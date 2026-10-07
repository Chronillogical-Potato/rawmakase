//! Versioned, bounded local control transport. Every request carries its own
//! reply channel; a timed-out request still in the queue is cancelled atomically.
use super::{Action, Msg, Sender, parse_action, shortcut_action};
use crate::app::commands::{
    self, Command, Error, Operation, Param, PhotoTarget, PresetTarget, Target, Until,
};
use crate::catalog::PhotoId;
use eframe::egui;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const ANSWER: Duration = Duration::from_secs(3);
const MAX_REQUEST: u64 = 64 * 1024;
const MAX_CLIENTS: usize = 16;

pub(super) struct Request {
    pub messages: Vec<Msg>,
    pub reply: mpsc::SyncSender<commands::Result<commands::Reply>>,
    phase: Arc<AtomicU8>, // pending, executing, cancelled
    deadline: Instant,
    stop: Arc<AtomicBool>,
}
impl Request {
    pub fn begin(&self) -> bool {
        if self.stop.load(Ordering::SeqCst) || Instant::now() >= self.deadline {
            let _ = self
                .phase
                .compare_exchange(0, 2, Ordering::SeqCst, Ordering::SeqCst);
            return false;
        }
        self.phase
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }
}
fn invalid(message: impl Into<String>) -> Error {
    Error::new("invalid_request", message)
}

fn command(request: &Value) -> commands::Result<Vec<Msg>> {
    if request
        .get("protocol")
        .is_some_and(|v| v.as_u64() != Some(u64::from(commands::PROTOCOL)))
    {
        return Err(Error::new(
            "unsupported_protocol",
            "Supported protocol version is 1",
        ));
    }
    let text = |key: &str| {
        request[key]
            .as_str()
            .ok_or_else(|| invalid(format!("{key} must be a string")))
    };
    let integer = |key: &str, min: i64, max: i64| {
        request[key]
            .as_i64()
            .filter(|n| (min..=max).contains(n))
            .ok_or_else(|| invalid(format!("{key} must be an integer from {min} to {max}")))
    };
    let param = || Param::parse(text("param")?).ok_or_else(|| invalid("Unknown parameter"));
    let target: Target = match request.get("target") {
        Some(v) => serde_json::from_value::<rawmakase_protocol::Target>(v.clone())
            .map_err(|e| invalid(e.to_string()))?
            .into(),
        None => Target::default(),
    };
    let operation = match text("cmd")? {
        "state" => Operation::State,
        "capabilities" => Operation::Capabilities,
        "photos" => Operation::Photos {
            query: request["query"].as_str().unwrap_or("").into(),
            offset: request["offset"]
                .as_u64()
                .unwrap_or(0)
                .min(usize::MAX as u64) as usize,
            limit: request["limit"].as_u64().unwrap_or(100).clamp(1, 500) as usize,
        },
        "presets" => Operation::Presets {
            group: request["group"].as_str().map(Into::into),
        },
        "preset" => Operation::Preset(match (request["id"].as_str(), request["name"].as_str()) {
            (Some(id), _) => PresetTarget::Id(id.into()),
            (None, Some(name)) => PresetTarget::Name {
                name: name.into(),
                group: request["group"].as_str().map(Into::into),
            },
            _ => return Err(invalid("preset requires id or name")),
        }),
        "curve" => {
            let channel = serde_json::from_value(request["channel"].clone())
                .map_err(|_| invalid("channel must be rgb, red, green or blue"))?;
            let points = serde_json::from_value(request["points"].clone())
                .map_err(|_| invalid("points must be an array of [input, output] pairs"))?;
            Operation::Curve(channel, points)
        }
        "set" => {
            let value = request["value"]
                .as_f64()
                .filter(|n| n.is_finite() && (*n as f32).is_finite())
                .ok_or_else(|| invalid("value must be a finite float"))?;
            let p = param()?;
            if matches!(p, Param::Band(_)) {
                return Err(invalid("Use an explicit band channel, such as band3.sat"));
            }
            Operation::Set(p, value as f32)
        }
        "turn" => {
            let p = param()?;
            if matches!(p, Param::Band(_)) {
                return Err(invalid("Use an explicit band channel, such as band3.sat"));
            }
            Operation::Adjust(p, integer("ticks", -1000, 1000)? as i32)
        }
        "action" => {
            let name = text("action")?;
            let action = commands::Action::parse(name)
                .or_else(|| match parse_action(name)? {
                    Action::Key(k, m) => shortcut_action(k, m),
                    Action::Mixer(c) => Some(commands::Action::Mixer(c)),
                    Action::ToggleMono => Some(commands::Action::ToggleMono),
                    Action::Named(a) => Some(a),
                    Action::Hold(_) => None,
                })
                .ok_or_else(|| {
                    invalid("Unknown action; use capabilities to list supported actions")
                })?;
            Operation::Action(action)
        }
        "open" => Operation::Open(match (request["id"].as_i64(), request["name"].as_str()) {
            (Some(id), _) => PhotoTarget::Id(PhotoId(id)),
            (None, Some(name)) => PhotoTarget::Name(name.into()),
            _ => return Err(invalid("open requires id or name")),
        }),
        "search" => Operation::Search(text("text")?.into()),
        "module" => Operation::Module(match text("module")? {
            "develop" => true,
            "library" => false,
            _ => return Err(invalid("module must be develop or library")),
        }),
        "photo" => {
            let step = integer("step", -1, 1)?;
            if step == 0 {
                return Err(invalid("step must be -1 or 1"));
            }
            Operation::Navigate(step as i32)
        }
        "save" => Operation::Save,
        "export" | "preview" => {
            let preview = text("cmd")? == "preview";
            let max_edge = match request.get("max_edge") {
                Some(_) => integer("max_edge", 1, 16384)? as u32,
                None => {
                    if preview {
                        1600
                    } else {
                        0
                    }
                }
            };
            Operation::Output {
                path: text("path")?.into(),
                max_edge,
            }
        }
        "job" => Operation::Job(integer("job_id", 1, i64::MAX)? as u64),
        // Answered once the photo is loaded or the job finished, or after
        // timeout_ms (1 s by default, at most 5 s, within clients' read timeout).
        "wait" => {
            let until = match text("until")? {
                "loaded" => Until::Loaded(PhotoId(integer("photo_id", 1, i64::MAX)?)),
                "job" => Until::Job(integer("job_id", 1, i64::MAX)? as u64),
                _ => return Err(invalid("until must be loaded or job")),
            };
            let millis = match request.get("timeout_ms") {
                None => 1000,
                Some(_) => integer("timeout_ms", 1, 5000)?,
            };
            Operation::Wait(until, Duration::from_millis(millis as u64))
        }
        // Legacy device-level commands remain an adapter. Explicit targets
        // belong to semantic commands, never to mutable device mappings.
        "cc" | "note" if request.get("target").is_some() => {
            return Err(invalid("Use set/turn/action for explicit targets"));
        }
        "cc" => {
            return Ok(vec![Msg::Cc(
                integer("cc", 0, 127)? as u8,
                integer("value", 0, 127)? as u8,
            )]);
        }
        "note" => {
            let note = integer("note", 0, 127)? as u8;
            return Ok(match request["press"].as_str().unwrap_or("click") {
                "click" => vec![Msg::Note(note, true), Msg::Note(note, false)],
                "down" => vec![Msg::Note(note, true)],
                "up" => vec![Msg::Note(note, false)],
                _ => return Err(invalid("press must be click, down or up")),
            });
        }
        _ => return Err(invalid("Unknown command")),
    };
    Ok(vec![Msg::Command(Command { operation, target })])
}

fn handle(
    line: &str,
    token: &str,
    tx: &Sender<Msg>,
    ctx: &egui::Context,
    stop: &Arc<AtomicBool>,
    timeout: Duration,
) -> commands::Result<Value> {
    let request: Value = serde_json::from_str(line).map_err(|e| invalid(e.to_string()))?;
    if request["token"].as_str() != Some(token) {
        return Err(Error::new("unauthorized", "Wrong token; read control.json"));
    }
    if stop.load(Ordering::SeqCst) {
        return Err(Error::new("disabled", "External control is disabled"));
    }
    let messages = command(&request)?;
    // A wait is answered by its own time, not the usual answer's.
    let timeout = match messages[..] {
        [
            Msg::Command(Command {
                operation: Operation::Wait(_, wait),
                ..
            }),
        ] => timeout + wait,
        _ => timeout,
    };
    let (reply, rx) = mpsc::sync_channel(1);
    let phase = Arc::new(AtomicU8::new(0));
    let queued = Request {
        messages,
        reply,
        phase: phase.clone(),
        deadline: Instant::now() + timeout,
        stop: stop.clone(),
    };
    tx.try_send(Msg::Request(queued)).map_err(|e| match e {
        mpsc::TrySendError::Full(_) => Error::new("busy", "The command queue is full"),
        mpsc::TrySendError::Disconnected(_) => Error::new("closed", "RAWmakase is closing"),
    })?;
    ctx.request_repaint();
    match rx.recv_timeout(timeout) {
        Ok(result) => {
            result.map(|reply| serde_json::to_value(reply).expect("serializable command reply"))
        }
        Err(_) => {
            if phase
                .compare_exchange(0, 2, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
                || phase.load(Ordering::SeqCst) == 2
            {
                Err(Error::new(
                    "cancelled",
                    "The command did not start in time and was cancelled",
                ))
            } else {
                Err(Error::new(
                    "outcome_unknown",
                    "Execution started but the reply timed out; inspect state before retrying",
                ))
            }
        }
    }
}
fn serve(
    mut stream: TcpStream,
    token: &str,
    tx: &Sender<Msg>,
    ctx: &egui::Context,
    stop: &Arc<AtomicBool>,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let mut line = String::new();
    let read = stream
        .try_clone()
        .and_then(|s| BufReader::new(s.take(MAX_REQUEST + 1)).read_line(&mut line));
    let result = match read {
        Ok(0) => return,
        Ok(_) if line.len() as u64 > MAX_REQUEST || !line.ends_with('\n') => Err(invalid(
            "Request must be a newline-terminated JSON object of at most 64 KiB",
        )),
        Ok(_) => handle(&line, token, tx, ctx, stop, ANSWER),
        Err(e) => Err(invalid(e.to_string())),
    };
    let id = serde_json::from_str::<Value>(&line)
        .ok()
        .and_then(|v| v.get("request_id").cloned());
    let mut reply = match result {
        Ok(mut body) => {
            body["ok"] = true.into();
            body
        }
        Err(error) => json!({"ok":false,"error":error.message,"code":error.code}),
    };
    reply["protocol"] = commands::PROTOCOL.into();
    reply["request_id"] = id.into();
    let _ = writeln!(stream, "{reply}");
}
fn spawn(
    listener: TcpListener,
    token: String,
    tx: Sender<Msg>,
    ctx: egui::Context,
    stop: Arc<AtomicBool>,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("control-socket".into())
        .spawn(move || {
            let clients = Arc::new(AtomicUsize::new(0));
            for stream in listener.incoming().flatten() {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                if clients.fetch_add(1, Ordering::SeqCst) >= MAX_CLIENTS {
                    clients.fetch_sub(1, Ordering::SeqCst);
                    continue;
                }
                let (token, tx, ctx, stop, clients) = (
                    token.clone(),
                    tx.clone(),
                    ctx.clone(),
                    stop.clone(),
                    clients.clone(),
                );
                let count = clients.clone();
                let spawned = std::thread::Builder::new()
                    .name("control-request".into())
                    .spawn(move || {
                        serve(stream, &token, &tx, &ctx, &stop);
                        clients.fetch_sub(1, Ordering::SeqCst);
                    });
                if spawned.is_err() {
                    count.fetch_sub(1, Ordering::SeqCst);
                }
            }
        })
        .map(drop)
}
fn new_token() -> std::io::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(std::io::Error::other)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
fn write_connection(path: &std::path::Path, port: u16, token: &str) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("Missing data directory"))?;
    std::fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    let endpoint = rawmakase_protocol::Endpoint {
        protocol: commands::PROTOCOL,
        port,
        token: token.into(),
        pid: Some(std::process::id()),
    };
    serde_json::to_writer(&mut file, &endpoint)?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}
pub(super) struct Handle {
    stop: Arc<AtomicBool>,
    port: u16,
    token: String,
    path: std::path::PathBuf,
}
impl Drop for Handle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect_timeout(
            &(Ipv4Addr::LOCALHOST, self.port).into(),
            Duration::from_millis(100),
        );
        let ours = std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .is_some_and(|v| v["token"].as_str() == Some(&self.token));
        if ours {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
pub(super) fn start(tx: Sender<Msg>, ctx: egui::Context) -> Option<Handle> {
    start_at(
        tx,
        ctx,
        rawmakase_protocol::Endpoint::path(&crate::storage::data_dir()),
    )
    .map_err(|e| eprintln!("Control socket: {e}"))
    .ok()
}
fn start_at(
    tx: Sender<Msg>,
    ctx: egui::Context,
    path: std::path::PathBuf,
) -> std::io::Result<Handle> {
    let token = new_token()?;
    let stop = Arc::new(AtomicBool::new(false));
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let port = listener.local_addr()?.port();
    write_connection(&path, port, &token)?;
    let handle = Handle {
        stop: stop.clone(),
        port,
        token: token.clone(),
        path,
    };
    spawn(listener, token, tx, ctx, stop)?;
    Ok(handle)
}

#[cfg(test)]
pub(super) fn test_request(
    messages: Vec<Msg>,
) -> (Request, mpsc::Receiver<commands::Result<commands::Reply>>) {
    let (reply, rx) = mpsc::sync_channel(1);
    (
        Request {
            messages,
            reply,
            phase: Arc::default(),
            deadline: Instant::now() + Duration::from_secs(30),
            stop: Arc::default(),
        },
        rx,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_numbers_scopes_and_versions() {
        for value in [
            json!({"cmd":"set","param":"exposure","value":1e100}),
            json!({"cmd":"turn","param":"exposure","ticks":1.5}),
            json!({"cmd":"cc","cc":1.5,"value":1}),
            json!({"cmd":"state","protocol":2}),
            json!({"cmd":"set","param":"band1","value":1}),
            json!({"cmd":"state","target":{"typo":1}}),
        ] {
            assert!(command(&value).is_err(), "{value}");
        }
    }
    #[test]
    fn timeout_cancels_queued_commands() {
        let (tx, rx) = mpsc::sync_channel(2);
        let result = handle(
            r#"{"token":"t","cmd":"set","param":"exposure","value":1}"#,
            "t",
            &tx,
            &egui::Context::default(),
            &Arc::default(),
            Duration::from_millis(5),
        );
        assert_eq!(result.unwrap_err().code, "cancelled");
        let Msg::Request(request) = rx.recv().unwrap() else {
            panic!()
        };
        assert!(!request.begin());
    }
    #[test]
    fn clients_receive_only_their_own_results() {
        let (tx, rx) = mpsc::sync_channel(4);
        let clients: Vec<_> = (1..=2)
            .map(|id| {
                let tx = tx.clone();
                std::thread::spawn(move || {
                    handle(
                        &json!({"token":"t","cmd":"open","id":id}).to_string(),
                        "t",
                        &tx,
                        &egui::Context::default(),
                        &Arc::default(),
                        Duration::from_secs(5),
                    )
                    .unwrap()
                })
            })
            .collect();
        for _ in 0..2 {
            let Msg::Request(request) = rx.recv().unwrap() else {
                panic!()
            };
            assert!(request.begin());
            let Msg::Command(Command {
                operation: Operation::Open(PhotoTarget::Id(id)),
                ..
            }) = &request.messages[0]
            else {
                panic!()
            };
            let ctx = egui::Context::default();
            let mut editor = super::super::Editor::with_context(
                &ctx,
                None,
                crate::app::session::Session::default(),
                None,
            );
            request
                .reply
                .send(Ok(commands::Reply {
                    state: editor.command_state(),
                    result: commands::Outcome::Search {
                        query: id.to_string(),
                    },
                    status: "applied",
                }))
                .unwrap();
        }
        for (index, client) in clients.into_iter().enumerate() {
            assert_eq!(
                client.join().unwrap()["result"]["query"],
                (index + 1).to_string()
            );
        }
    }
    #[test]
    fn real_tcp_reply_contains_the_executed_requests_identity() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::sync_channel(8);
        let ctx = egui::Context::default();
        let mut editor = crate::app::Editor::with_context(
            &ctx,
            None,
            crate::app::session::Session::default(),
            None,
        );
        editor.controls = super::super::Hub::new(super::super::Settings::default(), rx);
        let handle = start_at(tx, ctx.clone(), dir.path().join("control.json")).unwrap();
        let port = handle.port;
        let token = handle.token.clone();
        let client = std::thread::spawn(move || {
            let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            writeln!(
                stream,
                "{}",
                json!({"protocol":1,"request_id":"state-1","token":token,"cmd":"state"})
            )
            .unwrap();
            let mut line = String::new();
            BufReader::new(stream).read_line(&mut line).unwrap();
            serde_json::from_str::<Value>(&line).unwrap()
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !client.is_finished() {
            editor.control_commands(&ctx);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let reply = client.join().unwrap();
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["protocol"], 1);
        assert_eq!(reply["request_id"], "state-1");
        assert_eq!(
            reply["state"],
            serde_json::to_value(editor.command_state()).unwrap()
        );
        drop(handle);
    }
    #[test]
    fn disable_and_backpressure_prevent_execution() {
        let (request, _) = test_request(vec![]);
        request.stop.store(true, Ordering::SeqCst);
        assert!(!request.begin());
        let (tx, _rx) = mpsc::sync_channel(1);
        tx.try_send(Msg::Command(Command::new(Operation::State)))
            .unwrap();
        let result = handle(
            r#"{"token":"t","cmd":"state"}"#,
            "t",
            &tx,
            &egui::Context::default(),
            &Arc::default(),
            Duration::from_millis(5),
        );
        assert_eq!(result.unwrap_err().code, "busy");
    }

    #[test]
    fn token_disable_and_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.json");
        let (tx, rx) = mpsc::sync_channel(2);
        let handle = start_at(tx.clone(), egui::Context::default(), path.clone()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let denied = super::handle(
            r#"{"token":"wrong","cmd":"state"}"#,
            &handle.token,
            &tx,
            &egui::Context::default(),
            &handle.stop,
            Duration::from_millis(5),
        );
        assert_eq!(denied.unwrap_err().code, "unauthorized");
        assert!(rx.try_recv().is_err());
        let stop = handle.stop.clone();
        drop(handle);
        assert!(!path.exists());
        assert!(stop.load(Ordering::SeqCst));
    }
}
