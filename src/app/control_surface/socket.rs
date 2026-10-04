//! A local control socket, so a program can do what the Loupedeck does:
//! `rawmakase-ctl` (tools/rawmakase-ctl) turns dials, presses buttons and sets
//! sliders by sending the same messages the MIDI thread does.
//!
//! The app listens on a loopback TCP port (the same on every platform) and
//! writes `control.json` in the data folder with the port and a random token;
//! only a program that can read that file can send commands. A request is one
//! line of JSON and so is the reply:
//!
//! ```text
//! {"token": "...", "cmd": "turn", "param": "exposure", "ticks": 5}
//! {"ok": true, "state": {"mode": "develop", "values": {"exposure": 0.1, ...}}}
//! ```
//!
//! Commands that act on the Library (`open`, `search`, `module`) put their outcome
//! in `state.result`: `{"ok": false, "error": "..."}` when they fail.
//!
//! A reply is sent once a frame has handled the command, with the state after it.
use super::{Action, Job, Msg, Param, Target, parse_action};
use eframe::egui;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    sync::{
        Condvar, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
        mpsc::Sender,
    },
    time::Duration,
};

/// How long a request waits for the app to draw a frame.
const ANSWER: Duration = Duration::from_secs(3);

/// A request is one short line.
const MAX_REQUEST: u64 = 64 * 1024;

/// Hands out a ticket per command and tells whoever holds one when a frame has
/// read it.
#[derive(Default)]
pub(super) struct Shared {
    /// Tickets issued.
    sent: AtomicU64,
    /// The highest ticket a published frame had read, and its state.
    published: Mutex<(u64, Value)>,
    changed: Condvar,
}
impl Shared {
    /// How many tickets are issued; every command up to that is in the queue.
    pub(super) fn sent(&self) -> u64 {
        self.sent.load(Ordering::SeqCst)
    }
    fn ticket(&self) -> u64 {
        self.sent.fetch_add(1, Ordering::SeqCst) + 1
    }
    /// Answers the tickets up to `consumed`; `state` is only built when there
    /// are some not yet answered.
    pub(super) fn publish(&self, consumed: u64, state: impl FnOnce() -> Value) {
        let mut published = self
            .published
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if consumed > published.0 {
            *published = (consumed, state());
            self.changed.notify_all();
        }
    }
    fn wait(&self, ticket: u64, timeout: Duration) -> Option<Value> {
        let published = self
            .published
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (published, _) = self
            .changed
            .wait_timeout_while(published, timeout, |p| p.0 < ticket)
            .unwrap_or_else(PoisonError::into_inner);
        (published.0 >= ticket).then(|| published.1.clone())
    }
}

/// The messages a request stands for.
fn command(request: &Value) -> Result<Vec<Msg>, String> {
    let text = |key: &str| {
        request[key]
            .as_str()
            .ok_or_else(|| format!("\"{key}\" is missing"))
    };
    let number = |key: &str| {
        request[key]
            .as_f64()
            .filter(|n| n.is_finite())
            .ok_or_else(|| format!("\"{key}\" is missing or not a number"))
    };
    let midi = |key: &str| {
        let n = number(key)?;
        (0. ..=127.)
            .contains(&n)
            .then_some(n as u8)
            .ok_or_else(|| format!("\"{key}\" must be 0 to 127"))
    };
    let param = || {
        let name = text("param")?;
        Param::parse(name).ok_or_else(|| format!("unknown slider \"{name}\""))
    };
    Ok(match text("cmd")? {
        "state" => vec![Msg::Ping],
        "cc" => vec![Msg::Cc(midi("cc")?, midi("value")?)],
        "note" => {
            let note = midi("note")?;
            match request["press"].as_str().unwrap_or("click") {
                "click" => vec![Msg::Note(note, true), Msg::Note(note, false)],
                "down" => vec![Msg::Note(note, true)],
                "up" => vec![Msg::Note(note, false)],
                other => return Err(format!("\"press\" is click, down or up, not \"{other}\"")),
            }
        }
        "turn" => {
            let ticks = number("ticks")?;
            if ticks.abs() > 1000. {
                return Err("\"ticks\" must be within 1000".into());
            }
            vec![Msg::Turn(param()?, ticks as i32)]
        }
        "set" => vec![Msg::Set(param()?, number("value")? as f32)],
        "action" => {
            let name = text("action")?;
            match parse_action(name) {
                Some(Action::Hold(_)) => return Err("a held modifier is not an action".into()),
                Some(action) => vec![Msg::Action(action)],
                None => return Err(format!("unknown action \"{name}\"")),
            }
        }
        "open" => {
            let target = match (request["id"].as_i64(), request["name"].as_str()) {
                (Some(id), _) => Target::Id(id),
                (None, Some(name)) => Target::Name(name.into()),
                _ => return Err("\"name\" or \"id\" is missing".into()),
            };
            vec![Msg::Job(Job::Open(target))]
        }
        "search" => vec![Msg::Job(Job::Search(text("text")?.into()))],
        "module" => match text("module")? {
            "develop" => vec![Msg::Job(Job::Module(true))],
            "library" => vec![Msg::Job(Job::Module(false))],
            other => return Err(format!("\"module\" is develop or library, not \"{other}\"")),
        },
        "photo" => match number("step")? as i32 {
            step @ (-1 | 1) => vec![Msg::Photo(step)],
            _ => return Err("\"step\" is -1 or 1".into()),
        },
        other => return Err(format!("unknown command \"{other}\"")),
    })
}

fn handle(
    line: &str,
    token: &str,
    tx: &Sender<Msg>,
    ctx: &egui::Context,
    shared: &Shared,
) -> Result<Value, String> {
    let request: Value = serde_json::from_str(line).map_err(|e| format!("not JSON: {e}"))?;
    if request["token"].as_str() != Some(token) {
        return Err("wrong token; read it from control.json".into());
    }
    let messages = command(&request)?;
    for message in messages {
        tx.send(message).map_err(|_| "RAWmakase is closing")?;
    }
    let ticket = shared.ticket();
    ctx.request_repaint();
    shared.wait(ticket, ANSWER).ok_or_else(|| {
        "RAWmakase drew no frame in time (is its window minimized?); the command is queued".into()
    })
}

fn serve(
    mut stream: TcpStream,
    token: &str,
    tx: &Sender<Msg>,
    ctx: &egui::Context,
    shared: &Shared,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let mut line = String::new();
    let read = match stream.try_clone() {
        Ok(reader) => BufReader::new(reader.take(MAX_REQUEST)).read_line(&mut line),
        Err(e) => Err(e),
    };
    let reply = match read {
        Ok(0) => return,
        Ok(_) => match handle(&line, token, tx, ctx, shared) {
            Ok(state) => json!({"ok": true, "state": state}),
            Err(error) => json!({"ok": false, "error": error}),
        },
        Err(e) => json!({"ok": false, "error": e.to_string()}),
    };
    let _ = writeln!(stream, "{reply}");
}

fn spawn(
    listener: TcpListener,
    token: String,
    tx: Sender<Msg>,
    ctx: egui::Context,
    shared: std::sync::Arc<Shared>,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("control-socket".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let (token, tx, ctx, shared) =
                    (token.clone(), tx.clone(), ctx.clone(), shared.clone());
                // A client that stalls must not hold up the next.
                let _ = std::thread::Builder::new()
                    .name("control-request".into())
                    .spawn(move || serve(stream, &token, &tx, &ctx, &shared));
            }
        })
        .map(drop)
}

/// A token nobody can guess: the standard library's hasher is seeded by the OS.
fn new_token() -> String {
    use std::hash::{BuildHasher, Hasher, RandomState};
    (0..4)
        .map(|_| format!("{:016x}", RandomState::new().build_hasher().finish()))
        .collect()
}

/// Writes where the socket is, for `rawmakase-ctl`; only its owner may read it.
fn write_connection(port: u16, token: &str) -> std::io::Result<()> {
    let dir = crate::storage::data_dir();
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join("control.json.tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let body = json!({"port": port, "token": token, "pid": std::process::id()});
    options.open(&tmp)?.write_all(body.to_string().as_bytes())?;
    std::fs::rename(tmp, dir.join("control.json"))
}

/// Starts listening on a thread of its own. Without a socket nothing is ever sent.
pub(super) fn start(tx: Sender<Msg>, ctx: egui::Context, shared: std::sync::Arc<Shared>) {
    let token = new_token();
    let started = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).and_then(|listener| {
        write_connection(listener.local_addr()?.port(), &token)?;
        spawn(listener, token, tx, ctx, shared)
    });
    if let Err(e) = started {
        eprintln!("Control socket: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, mpsc};

    fn parsed(text: &str) -> Result<Vec<Msg>, String> {
        command(&serde_json::from_str(text).unwrap())
    }

    #[test]
    fn requests_become_the_messages_the_device_sends() {
        let note = parsed(r#"{"cmd":"note","note":87}"#).unwrap();
        assert!(matches!(
            note[..],
            [Msg::Note(87, true), Msg::Note(87, false)]
        ));
        let held = parsed(r#"{"cmd":"note","note":66,"press":"down"}"#).unwrap();
        assert!(matches!(held[..], [Msg::Note(66, true)]));
        let cc = parsed(r#"{"cmd":"cc","cc":33,"value":127}"#).unwrap();
        assert!(matches!(cc[..], [Msg::Cc(33, 127)]));
        let turn = parsed(r#"{"cmd":"turn","param":"Exposure","ticks":-5}"#).unwrap();
        assert!(matches!(turn[..], [Msg::Turn(Param::Exposure, -5)]));
        let set = parsed(r#"{"cmd":"set","param":"band3.sat","value":20}"#).unwrap();
        assert!(matches!(set[..], [Msg::Set(Param::Hsl(2, 1), v)] if v == 20.));
        let key = parsed(r#"{"cmd":"action","action":"cmd+shift+z"}"#).unwrap();
        assert!(matches!(key[..], [Msg::Action(Action::Key(..))]));
        let open = parsed(r#"{"cmd":"open","name":"DSCF0042"}"#).unwrap();
        assert!(matches!(&open[..], [Msg::Job(Job::Open(Target::Name(n)))] if n == "DSCF0042"));
        let open = parsed(r#"{"cmd":"open","id":7}"#).unwrap();
        assert!(matches!(open[..], [Msg::Job(Job::Open(Target::Id(7)))]));
        let search = parsed(r#"{"cmd":"search","text":""}"#).unwrap();
        assert!(matches!(&search[..], [Msg::Job(Job::Search(t))] if t.is_empty()));
        let module = parsed(r#"{"cmd":"module","module":"develop"}"#).unwrap();
        assert!(matches!(module[..], [Msg::Job(Job::Module(true))]));
        let photo = parsed(r#"{"cmd":"photo","step":-1}"#).unwrap();
        assert!(matches!(photo[..], [Msg::Photo(-1)]));
    }

    #[test]
    fn bad_requests_say_what_is_wrong() {
        for (text, wanted) in [
            (r#"{"cmd":"cc","cc":200,"value":1}"#, "0 to 127"),
            (
                r#"{"cmd":"turn","param":"nope","ticks":1}"#,
                "unknown slider",
            ),
            (
                r#"{"cmd":"turn","param":"tint","ticks":1e9}"#,
                "within 1000",
            ),
            (r#"{"cmd":"set","param":"tint"}"#, "\"value\""),
            (r#"{"cmd":"action","action":"hold:shift"}"#, "not an action"),
            (r#"{"cmd":"action","action":"nonsense"}"#, "unknown action"),
            (r#"{"cmd":"photo","step":5}"#, "-1 or 1"),
            (r#"{"cmd":"open"}"#, "\"name\" or \"id\""),
            (r#"{"cmd":"module","module":"grid"}"#, "develop or library"),
            (r#"{"cmd":"dance"}"#, "unknown command"),
            (r#"{}"#, "\"cmd\""),
        ] {
            let error = parsed(text).err().unwrap_or_default();
            assert!(error.contains(wanted), "{text}: {error}");
        }
    }

    #[test]
    fn a_reply_waits_for_the_frame_that_read_the_command() {
        let shared = Arc::new(Shared::default());
        let ticket = shared.ticket();
        assert!(shared.wait(ticket, Duration::from_millis(10)).is_none());
        shared.publish(0, || panic!("nothing to answer"));
        let waiting = {
            let shared = shared.clone();
            std::thread::spawn(move || shared.wait(ticket, Duration::from_secs(5)))
        };
        shared.publish(ticket, || json!({"mode": "develop"}));
        assert_eq!(waiting.join().unwrap(), Some(json!({"mode": "develop"})));
        // Later tickets are answered by later frames only.
        let next = shared.ticket();
        assert!(shared.wait(next, Duration::from_millis(10)).is_none());
    }

    #[test]
    fn the_socket_checks_the_token_and_answers_over_tcp() {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared::default());
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        spawn(
            listener,
            "secret".into(),
            tx,
            egui::Context::default(),
            shared.clone(),
        )
        .unwrap();
        // A stand-in for the app's frames.
        let frames = {
            let shared = shared.clone();
            std::thread::spawn(move || {
                let mut got = Vec::new();
                while got.is_empty() {
                    let consumed = shared.sent();
                    while let Ok(msg) = rx.try_recv() {
                        got.push(msg);
                    }
                    shared.publish(consumed, || json!({"mode": "develop"}));
                    std::thread::sleep(Duration::from_millis(5));
                }
                got
            })
        };
        let ask = |request: Value| -> Value {
            let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
            writeln!(stream, "{request}").unwrap();
            let mut reply = String::new();
            BufReader::new(stream).read_line(&mut reply).unwrap();
            serde_json::from_str(&reply).unwrap()
        };
        let denied = ask(json!({"token": "guess", "cmd": "state"}));
        assert_eq!(denied["ok"], false);
        let reply = ask(json!({"token": "secret", "cmd": "turn", "param": "tint", "ticks": 2}));
        assert_eq!(reply, json!({"ok": true, "state": {"mode": "develop"}}));
        let got = frames.join().unwrap();
        assert!(matches!(got[..], [Msg::Turn(Param::Tint, 2)]));
    }
}
