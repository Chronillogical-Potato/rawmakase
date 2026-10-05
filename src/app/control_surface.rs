//! MIDI control surfaces such as the Loupedeck+: dials turn the Basic sliders and
//! buttons run named application actions. The listener reconnects when the
//! device is plugged in; `midi.json` in the data folder overrides the mapping.
//! The same commands also arrive from the `rawmakase-ctl` tool, over a local
//! socket (see [`socket`]).
mod settings;
mod socket;

use super::Editor;
use super::commands::{self, Command, Param};

use eframe::egui::{self, Key, Modifiers};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender as Sender},
    },
    time::{Duration, Instant},
};

/// A pause this long ends a turn of the photo dial.
const PHOTO_IDLE: Duration = Duration::from_millis(600);

/// Raw device input is translated here, never in the application command layer.
enum Msg {
    Cc(u8, u8),
    Note(u8, bool),
    Command(Command),
    Request(socket::Request),
    #[cfg(any(test, target_os = "macos", target_os = "windows"))]
    Midi(Arc<Mutex<Status>>, u64, Box<Msg>),
}

/// What a button does.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    Named(commands::Action),
    /// Legacy shortcut notation, resolved to a named action before execution.
    Key(Key, Modifiers),
    /// Acts as a modifier while held.
    Hold(Modifiers),
    /// Shows Hue, Saturation or Luminance (0..=2) in the Color Mixer, which is
    /// what the Band faders then turn.
    Mixer(usize),
    /// Black & White on or off, as the panel's B&W button does.
    ToggleMono,
}

#[derive(Clone, Debug, PartialEq)]
struct Config {
    /// Part of the MIDI port's name.
    port: String,
    dials: HashMap<u8, Param>,
    buttons: HashMap<u8, Action>,
    /// The dial that moves to the previous / next photo in Develop and the Loupe.
    photo_dial: Option<u8>,
    /// Ticks of that dial per photo.
    photo_detent: i32,
    /// Whether to listen for `rawmakase-ctl` on a local socket.
    socket: bool,
}
const SHIFT: Modifiers = Modifiers {
    shift: true,
    ..Modifiers::NONE
};
fn command() -> Modifiers {
    Modifiers {
        command: true,
        mac_cmd: cfg!(target_os = "macos"),
        ctrl: !cfg!(target_os = "macos"),
        ..Modifiers::NONE
    }
}
/// A key by name, in any case: "Backslash", "z", "left" or "ArrowLeft".
fn key_named(name: &str) -> Option<Key> {
    Key::from_name(name).or_else(|| {
        Key::ALL.iter().copied().find(|k| {
            k.name().eq_ignore_ascii_case(name) || format!("{k:?}").eq_ignore_ascii_case(name)
        })
    })
}
fn parse_action(text: &str) -> Option<Action> {
    if !text.starts_with("mixer:")
        && text != "toggle:bw"
        && let Some(action) = commands::Action::parse(text)
    {
        return Some(Action::Named(action));
    }
    if text.eq_ignore_ascii_case("toggle:bw") {
        return Some(Action::ToggleMono);
    }
    if let Some(channel) = text.strip_prefix("mixer:") {
        return match channel.trim().to_ascii_lowercase().as_str() {
            "hue" => Some(Action::Mixer(0)),
            "sat" | "saturation" => Some(Action::Mixer(1)),
            "lum" | "luminance" => Some(Action::Mixer(2)),
            _ => None,
        };
    }
    let (hold, text) = match text.strip_prefix("hold:") {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let mut modifiers = Modifiers::NONE;
    let mut key = None;
    for part in text.split('+').map(str::trim) {
        match part.to_ascii_lowercase().as_str() {
            "cmd" | "command" | "ctrl" | "control" | "primary" => modifiers |= command(),
            "control-key" => modifiers.ctrl = true,
            "shift" => modifiers.shift = true,
            "alt" | "option" => modifiers.alt = true,
            _ => key = Some(key_named(part)?),
        }
    }
    match (hold, key) {
        (true, None) => Some(Action::Hold(modifiers)),
        (false, Some(key)) => Some(Action::Key(key, modifiers)),
        _ => None,
    }
}
/// Only bindings with an executable application meaning are offered in Preferences.
fn supported_action(text: &str) -> Option<Action> {
    let action = parse_action(text)?;
    if let Action::Key(key, modifiers) = action
        && shortcut_action(key, modifiers).is_none()
    {
        return None;
    }
    Some(action)
}
/// The text `parse_action` reads back: `cmd+shift+z`, `hold:shift`, `mixer:hue`.
fn action_spec(action: Action) -> String {
    let modifiers = |m: Modifiers| {
        let mut parts = Vec::new();
        if m.command || m.mac_cmd {
            parts.push("cmd");
        } else if m.ctrl {
            parts.push("control-key");
        }
        if m.shift {
            parts.push("shift");
        }
        if m.alt {
            parts.push("alt");
        }
        parts
    };
    match action {
        Action::Named(a) => a.name().into(),
        Action::ToggleMono => "toggle:bw".into(),
        Action::Mixer(c) => format!("mixer:{}", ["hue", "sat", "lum"][c]),
        Action::Hold(m) => format!("hold:{}", modifiers(m).join("+")),
        Action::Key(key, m) => {
            let mut parts = modifiers(m);
            parts.push(key.name());
            parts.join("+")
        }
    }
}
impl Config {
    fn path() -> std::path::PathBuf {
        crate::storage::data_dir().join("midi.json")
    }
    /// Writes `midi.json` as what differs from the defaults, so a later release's
    /// better defaults still reach whatever was left alone.
    fn save(&self) -> anyhow::Result<()> {
        let mut midi = self.to_json();
        midi.as_object_mut()
            .expect("config object")
            .remove("socket");
        crate::storage::atomic_json(&Self::path(), &midi)?;
        crate::storage::atomic_json(
            &crate::storage::data_dir().join("automation.json"),
            &serde_json::json!({"protocol":commands::PROTOCOL,"socket":self.socket}),
        )
    }
    fn to_json(&self) -> serde_json::Value {
        use serde_json::{Map, Value};
        let defaults = Self::defaults();
        fn changes<T: PartialEq + Copy>(
            now: &HashMap<u8, T>,
            was: &HashMap<u8, T>,
            spec: impl Fn(T) -> String,
        ) -> Map<String, Value> {
            let mut numbers: Vec<u8> = now.keys().chain(was.keys()).copied().collect();
            numbers.sort_unstable();
            numbers.dedup();
            numbers
                .into_iter()
                .filter(|n| now.get(n) != was.get(n))
                .map(|n| {
                    let value = now.get(&n).map_or(Value::Null, |v| spec(*v).into());
                    (n.to_string(), value)
                })
                .collect()
        }
        serde_json::json!({
            "port": self.port,
            "socket": self.socket,
            "photo_dial": self.photo_dial,
            "photo_detent": self.photo_detent,
            "dials": changes(&self.dials, &defaults.dials, Param::spec),
            "buttons": changes(&self.buttons, &defaults.buttons, action_spec),
        })
    }
    /// The Loupedeck+ layout, as mapped with its default Lightroom profile.
    fn defaults() -> Self {
        let faders = (0..8).map(|i| (17 + i as u8, Param::Band(i)));
        let dials = [
            (33, Param::Exposure),
            (34, Param::Blacks),
            (35, Param::Whites),
            (36, Param::Saturation),
            (37, Param::Vibrance),
            (38, Param::Temperature),
            (39, Param::Tint),
            (40, Param::Highlights),
            (44, Param::Shadows),
            (45, Param::Clarity),
            (46, Param::Contrast),
        ]
        .into_iter()
        .chain(faders);
        let buttons = [
            (66, Action::Hold(SHIFT)),
            (67, Action::Hold(Modifiers::CTRL)),
            (68, Action::Hold(command())),
            (69, Action::Hold(Modifiers::ALT)),
            (76, Action::Key(Key::ArrowUp, Modifiers::NONE)),
            (77, Action::Key(Key::ArrowDown, Modifiers::NONE)),
            (78, Action::Key(Key::ArrowLeft, Modifiers::NONE)),
            (79, Action::Key(Key::ArrowRight, Modifiers::NONE)),
            // P1-P5 rate 1-5 stars, P6 clears the rating, P7 picks and P8 rejects,
            // as the keys do.
            (80, Action::Key(Key::Num1, Modifiers::NONE)),
            (81, Action::Key(Key::Num2, Modifiers::NONE)),
            (82, Action::Key(Key::Num3, Modifiers::NONE)),
            (83, Action::Key(Key::Num4, Modifiers::NONE)),
            (84, Action::Key(Key::Num5, Modifiers::NONE)),
            (85, Action::Key(Key::Num0, Modifiers::NONE)),
            (86, Action::Key(Key::P, Modifiers::NONE)),
            (87, Action::Key(Key::X, Modifiers::NONE)),
            (88, Action::Key(Key::E, command() | SHIFT)),
            (92, Action::Key(Key::C, command() | SHIFT)),
            (93, Action::Key(Key::V, command() | SHIFT)),
            (95, Action::Key(Key::Z, command())),
            (96, Action::Key(Key::Z, command() | SHIFT)),
            // C1 toggles zoom, as Z does.
            (49, Action::Key(Key::Z, Modifiers::NONE)),
            // C3-C6 are the colour labels red, yellow, green and blue.
            (51, Action::Key(Key::Num6, Modifiers::NONE)),
            (52, Action::Key(Key::Num7, Modifiers::NONE)),
            (53, Action::Key(Key::Num8, Modifiers::NONE)),
            (54, Action::Key(Key::Num9, Modifiers::NONE)),
            // Screen Mode shows the clipping overlay.
            (97, Action::Key(Key::J, Modifiers::NONE)),
            (101, Action::ToggleMono),
            (98, Action::Mixer(0)),
            (99, Action::Mixer(1)),
            (100, Action::Mixer(2)),
            (102, Action::Key(Key::Backslash, Modifiers::NONE)),
        ];
        Self {
            port: "Loupedeck".into(),
            photo_dial: Some(48),
            photo_detent: 2,
            socket: false,
            dials: dials.collect(),
            buttons: buttons.into_iter().collect(),
        }
    }
    /// The defaults, changed by `midi.json`:
    /// `{"port": "Loupedeck", "socket": true, "dials": {"41": "contrast"}, "buttons": {"98": "h", "95": null}}`
    /// where `null` removes a default and a button is `"cmd+shift+z"` or `"hold:shift"`.
    fn load() -> Self {
        let mut config = Self::defaults();
        let path = Self::path();
        if let Ok(text) = std::fs::read_to_string(&path) {
            match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(json) => config.apply(&json),
                Err(e) => eprintln!("{}: {e}", path.display()),
            }
        }
        if let Ok(text) =
            std::fs::read_to_string(crate::storage::data_dir().join("automation.json"))
        {
            match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(json) => {
                    config.socket = json["protocol"].as_u64() == Some(u64::from(commands::PROTOCOL))
                        && json["socket"].as_bool().unwrap_or(false)
                }
                Err(e) => {
                    config.socket = false;
                    eprintln!("automation.json: {e}");
                }
            }
        }
        config
    }
    fn apply(&mut self, json: &serde_json::Value) {
        if let Some(port) = json["port"].as_str() {
            self.port = port.into();
        }
        if let Some(dial) = json.get("photo_dial") {
            self.photo_dial = dial.as_u64().and_then(|n| u8::try_from(n).ok());
        }
        if let Some(on) = json["socket"].as_bool() {
            self.socket = on;
        }
        if let Some(n) = json["photo_detent"].as_i64() {
            self.photo_detent = n.clamp(1, 64) as i32;
        }
        for (number, value) in json["dials"].as_object().into_iter().flatten() {
            let Ok(cc) = number.parse() else { continue };
            match value.as_str().map(Param::parse) {
                Some(Some(param)) => self.dials.insert(cc, param),
                _ => self.dials.remove(&cc),
            };
        }
        for (number, value) in json["buttons"].as_object().into_iter().flatten() {
            let Ok(note) = number.parse() else { continue };
            match value.as_str().map(parse_action) {
                Some(Some(action)) => self.buttons.insert(note, action),
                _ => self.buttons.remove(&note),
            };
        }
    }
}

/// A relative encoder value: 1..=63 clockwise ticks, 65..=127 counter-clockwise
/// (127 is one tick back).
fn ticks(value: u8) -> i32 {
    if value < 64 {
        i32::from(value)
    } else {
        i32::from(value) - 128
    }
}

#[cfg(any(test, target_os = "macos", target_os = "windows"))]
fn parse(bytes: &[u8]) -> Option<Msg> {
    match *bytes {
        [status, d1, d2] if status & 0xF0 == 0xB0 => Some(Msg::Cc(d1, d2)),
        [status, d1, d2] if status & 0xF0 == 0x90 && d2 > 0 => Some(Msg::Note(d1, true)),
        [status, d1, _] if status & 0xF0 == 0x80 => Some(Msg::Note(d1, false)),
        [status, d1, 0] if status & 0xF0 == 0x90 => Some(Msg::Note(d1, false)),
        _ => None,
    }
}

/// The latest message from the device, for Preferences.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Last {
    Dial(u8, i32),
    Button(u8),
}

/// What the listener tells Preferences.
#[derive(Default)]
struct Status {
    /// The MIDI port while it is connected.
    connected: Option<String>,
    epoch: u64,
    last: Option<Last>,
}

fn locked(status: &Mutex<Status>) -> std::sync::MutexGuard<'_, Status> {
    status.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The epoch is shared outside the bounded input queue, so disconnects cannot
/// lose their reset signal and queued input from old connections is discarded.
#[cfg(any(test, target_os = "macos", target_os = "windows"))]
fn reset_connection(status: &Mutex<Status>, ctx: &egui::Context) -> u64 {
    let mut status = locked(status);
    status.epoch = status.epoch.wrapping_add(1);
    ctx.request_repaint();
    status.epoch
}

/// Listens for the device on a thread of its own, finding it again when it is
/// plugged back in, until `stop` is set. Where there is no MIDI backend nothing
/// is ever sent.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn listen(
    port: String,
    tx: Sender<Msg>,
    ctx: egui::Context,
    status: Arc<Mutex<Status>>,
    stop: Arc<AtomicBool>,
) {
    use midir::{MidiInput, MidiInputConnection};
    let spawned = std::thread::Builder::new()
        .name("midi".into())
        .spawn(move || {
            let mut connected: Option<(String, String, MidiInputConnection<()>)> = None;
            let mut last = Instant::now();
            while !stop.load(Ordering::Relaxed) {
                // A gap far beyond the poll interval means the Mac slept; the
                // device comes back as a new endpoint behind the same name.
                if last.elapsed() > Duration::from_secs(10) {
                    connected = None;
                    reset_connection(&status, &ctx);
                }
                last = Instant::now();
                if let Ok(input) = MidiInput::new("RAWmakase") {
                    let ports = input.ports();
                    let named = |p: &midir::MidiInputPort| input.port_name(p).unwrap_or_default();
                    match &connected {
                        Some((name, id, _))
                            if !ports.iter().any(|p| &named(p) == name && &p.id() == id) =>
                        {
                            connected = None;
                            reset_connection(&status, &ctx);
                        }
                        Some(_) => {}
                        None => {
                            if let Some(p) = ports.iter().find(|p| named(p).contains(&port)) {
                                let epoch = reset_connection(&status, &ctx);
                                let (name, id) = (named(p), p.id());
                                let (tx, ctx, status) = (tx.clone(), ctx.clone(), status.clone());
                                connected = input
                                    .connect(
                                        p,
                                        "RAWmakase input",
                                        move |_, bytes, _| {
                                            if let Some(msg) = parse(bytes) {
                                                match msg {
                                                    Msg::Cc(cc, v) => {
                                                        locked(&status).last =
                                                            Some(Last::Dial(cc, ticks(v)));
                                                    }
                                                    Msg::Note(n, true) => {
                                                        locked(&status).last =
                                                            Some(Last::Button(n));
                                                    }
                                                    _ => {}
                                                }
                                                let _ = tx.try_send(Msg::Midi(
                                                    status.clone(),
                                                    epoch,
                                                    Box::new(msg),
                                                ));
                                                ctx.request_repaint();
                                            }
                                        },
                                        (),
                                    )
                                    .ok()
                                    .map(|c| (name, id, c));
                            }
                        }
                    }
                }
                locked(&status).connected = connected.as_ref().map(|(name, ..)| name.clone());
                // Looked at again in two seconds; a stop is noticed sooner.
                for _ in 0..8 {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
            }
            locked(&status).connected = None;
        });
    if let Err(e) = spawned {
        eprintln!("MIDI listener: {e}");
    }
}
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn listen(_: String, _: Sender<Msg>, _: egui::Context, _: Arc<Mutex<Status>>, _: Arc<AtomicBool>) {}

pub(super) struct Surface {
    rx: Receiver<Msg>,
    config: Config,
    held: Modifiers,
    photo_ticks: i32,
    photo_dir: i32,
    last_photo: Option<Instant>,
    midi_epoch: u64,
    status: Arc<Mutex<Status>>,
    stop_midi: Arc<AtomicBool>,
    listening_port: String,
    link: Option<(Sender<Msg>, egui::Context)>,
    socket: Option<socket::Handle>,
}
impl Drop for Surface {
    fn drop(&mut self) {
        self.stop_midi.store(true, Ordering::Relaxed);
    }
}
impl Surface {
    pub(super) fn inactive() -> Self {
        let (_, rx) = mpsc::sync_channel(256);
        Self::new(Config::defaults(), rx)
    }
    pub(super) fn start(ctx: &egui::Context) -> Self {
        let (tx, rx) = mpsc::sync_channel(256);
        let mut surface = Self::new(Config::load(), rx);
        surface.link = Some((tx, ctx.clone()));
        surface.restart_midi();
        surface.set_socket(surface.config.socket);
        surface
    }
    fn new(config: Config, rx: Receiver<Msg>) -> Self {
        Self {
            listening_port: config.port.clone(),
            config,
            rx,
            held: Modifiers::NONE,
            photo_ticks: 0,
            photo_dir: 0,
            last_photo: None,
            midi_epoch: 0,
            status: Arc::default(),
            stop_midi: Arc::default(),
            link: None,
            socket: None,
        }
    }
    fn restart_midi(&mut self) {
        self.stop_midi.store(true, Ordering::Relaxed);
        self.reset_midi_state();
        self.status = Arc::default();
        self.midi_epoch = 0;
        let Some((tx, ctx)) = &self.link else { return };
        self.stop_midi = Arc::default();
        self.listening_port = self.config.port.clone();
        listen(
            self.config.port.clone(),
            tx.clone(),
            ctx.clone(),
            self.status.clone(),
            self.stop_midi.clone(),
        );
    }
    fn set_socket(&mut self, on: bool) {
        self.socket = None;
        if on && let Some((tx, ctx)) = &self.link {
            self.socket = socket::start(tx.clone(), ctx.clone());
        }
    }
    fn reset_midi_state(&mut self) {
        self.held = Modifiers::NONE;
        self.photo_ticks = 0;
        self.photo_dir = 0;
        self.last_photo = None;
    }
    fn sync_midi_epoch(&mut self) {
        let epoch = locked(&self.status).epoch;
        if epoch != self.midi_epoch {
            self.midi_epoch = epoch;
            self.reset_midi_state();
        }
    }
    fn photo_turn(&mut self, t: i32) -> i32 {
        if t == 0 {
            return 0;
        }
        let fresh = self.last_photo.is_none_or(|p| p.elapsed() > PHOTO_IDLE);
        if fresh || t.signum() != self.photo_dir {
            self.photo_ticks = t.signum() * self.config.photo_detent;
        } else {
            self.photo_ticks += t;
        }
        self.photo_dir = t.signum();
        self.last_photo = Some(Instant::now());
        let step = self.photo_ticks / self.config.photo_detent;
        self.photo_ticks %= self.config.photo_detent;
        step.signum()
    }
    fn translate(&mut self, msg: Msg) -> commands::Result<Option<Command>> {
        use commands::{Action as A, Error, Operation as O};
        match &msg {
            Msg::Cc(cc, value) => locked(&self.status).last = Some(Last::Dial(*cc, ticks(*value))),
            Msg::Note(note, true) => locked(&self.status).last = Some(Last::Button(*note)),
            _ => {}
        }
        let action = match msg {
            Msg::Command(command) => return Ok(Some(command)),
            Msg::Cc(cc, v) if self.config.photo_dial == Some(cc) => {
                let step = self.photo_turn(ticks(v));
                return Ok((step != 0).then(|| Command::new(O::DeviceNavigate(step))));
            }
            Msg::Cc(cc, v) => {
                let p = self.config.dials.get(&cc).copied().ok_or_else(|| {
                    Error::new("unmapped_control", "No action is mapped to this CC")
                })?;
                return Ok(Some(Command::new(O::Adjust(p, ticks(v)))));
            }
            Msg::Note(n, down) => {
                let a = self.config.buttons.get(&n).copied().ok_or_else(|| {
                    Error::new("unmapped_control", "No action is mapped to this note")
                })?;
                if let Action::Hold(m) = a {
                    self.held = if down {
                        self.held | m
                    } else {
                        without(self.held, m)
                    };
                    return Ok(None);
                }
                if !down {
                    return Ok(None);
                }
                a
            }
            #[cfg(any(test, target_os = "macos", target_os = "windows"))]
            Msg::Midi(..) => unreachable!("MIDI envelope removed before translation"),
            Msg::Request(_) => return Err(Error::new("invalid_request", "Nested request")),
        };
        let advance = self.held.shift || matches!(action,Action::Key(_,m) if m.shift);
        let action = match action {
            Action::Mixer(c) => A::Mixer(c),
            Action::ToggleMono => A::ToggleMono,
            Action::Named(a) => a,
            Action::Key(k, m) => shortcut_action(k, m | self.held).ok_or_else(|| {
                Error::new(
                    "unsupported_action",
                    "This shortcut has no application action",
                )
            })?,
            Action::Hold(_) => return Ok(None),
        };
        let operation = if let Some(edit) = action.metadata() {
            O::Metadata { edit, advance }
        } else {
            O::Action(action)
        };
        Ok(Some(Command::new(operation)))
    }
}

/// Device/legacy key notation resolves to semantics. Never inject egui events:
/// handlers read both per-event and frame modifiers, and focus changes meaning.
fn shortcut_action(key: Key, m: Modifiers) -> Option<commands::Action> {
    use commands::Action as A;
    Some(
        match (key, m.command || m.ctrl || m.mac_cmd, m.shift, m.alt) {
            (Key::Z, true, false, false) => A::Undo,
            (Key::Z, true, true, false) | (Key::Y, true, false, false) => A::Redo,
            (Key::C, true, true, false) => A::Copy,
            (Key::V, true, true, false) => A::Paste,
            (Key::V, true, false, true) => A::PastePrevious,
            (Key::S, true, true, false) => A::Sync,
            (Key::R, true, true, false) => A::Reset,
            (Key::U, true, true, false) => A::AutoTone,
            (Key::E, true, true, false) => A::ExportDialog,
            (Key::E, true, true, true) => A::ExportPrevious,
            (Key::W, false, true, false) => A::Mask,
            (key, false, _, false) => match key {
                Key::Num0 => A::Rating(0),
                Key::Num1 => A::Rating(1),
                Key::Num2 => A::Rating(2),
                Key::Num3 => A::Rating(3),
                Key::Num4 => A::Rating(4),
                Key::Num5 => A::Rating(5),
                Key::Num6 => A::ToggleLabel(0),
                Key::Num7 => A::ToggleLabel(1),
                Key::Num8 => A::ToggleLabel(2),
                Key::Num9 => A::ToggleLabel(3),
                Key::P => A::Flag(1),
                Key::X => A::Flag(-1),
                Key::U => A::Flag(0),
                Key::ArrowLeft | Key::ArrowUp => A::Previous,
                Key::ArrowRight | Key::ArrowDown => A::Next,
                Key::Backslash => A::Compare,
                Key::J => A::Clipping,
                Key::Z => A::Zoom,
                Key::F => A::Fit,
                Key::R | Key::C => A::Crop,
                Key::Q => A::Remove,
                Key::W => A::WhiteBalance,
                Key::V => A::ToggleMono,
                Key::G => A::Library,
                Key::D => A::Develop,
                Key::E => A::Loupe,
                _ => return None,
            },
            _ => return None,
        },
    )
}

/// `a` without the modifiers that are on in `b`.
fn without(a: Modifiers, b: Modifiers) -> Modifiers {
    Modifiers {
        alt: a.alt && !b.alt,
        ctrl: a.ctrl && !b.ctrl,
        shift: a.shift && !b.shift,
        mac_cmd: a.mac_cmd && !b.mac_cmd,
        command: a.command && !b.command,
    }
}

impl Editor {
    pub(super) fn control_commands(&mut self, ctx: &egui::Context) {
        self.surface.sync_midi_epoch();
        let messages: Vec<_> = self.surface.rx.try_iter().take(128).collect();
        let full = messages.len() == 128;
        for message in messages {
            match message {
                Msg::Request(request) => {
                    if !request.begin() {
                        continue;
                    }
                    let result = self.control_messages(request.messages, ctx);
                    let state = self.command_state();
                    let _ = request.reply.send(result.map(|result| commands::Reply {
                        state,
                        result,
                        status: "applied",
                    }));
                }
                message => {
                    if let Err(error) = self.control_messages(vec![message], ctx) {
                        self.status = format!("Control surface: {}", error.message);
                    }
                }
            }
        }
        if full {
            ctx.request_repaint();
        }
    }
    fn control_messages(
        &mut self,
        messages: Vec<Msg>,
        ctx: &egui::Context,
    ) -> commands::Result<commands::Outcome> {
        let mut result = commands::Outcome::Empty;
        for msg in messages {
            self.sync_command_revision();
            self.surface.sync_midi_epoch();
            let source = commands::Source::Socket;
            #[cfg(any(test, target_os = "macos", target_os = "windows"))]
            let source = if matches!(msg, Msg::Midi(..)) {
                commands::Source::Midi
            } else {
                source
            };
            #[cfg(any(test, target_os = "macos", target_os = "windows"))]
            let msg = match msg {
                Msg::Midi(source, epoch, msg)
                    if Arc::ptr_eq(&source, &self.surface.status)
                        && epoch == self.surface.midi_epoch =>
                {
                    *msg
                }
                Msg::Midi(..) => continue,
                msg => msg,
            };
            if matches!(msg, Msg::Cc(cc, _) if self.surface.config.photo_dial == Some(cc))
                && self.library_mode
                && !self.library.as_ref().is_some_and(|l| l.loupe_open())
            {
                continue;
            }
            let device = matches!(msg, Msg::Cc(..) | Msg::Note(..));
            if let Some(mut command) = self.surface.translate(msg)? {
                // Device adjustments follow the active mask; scripts use explicit
                // scope. Unsupported mask parameters fail rather than edit globally.
                if device
                    && matches!(command.operation, commands::Operation::Adjust(..))
                    && self.view.is(super::state::Tool::Mask)
                {
                    let index =
                        self.view.masking.selected.ok_or_else(|| {
                            commands::Error::new("no_mask", "Select a mask first")
                        })?;
                    command.target = commands::Target {
                        mask: Some(index),
                        generation: Some(self.load.id()),
                        revision: Some(self.automation.revision),
                        ..Default::default()
                    };
                }
                if device
                    && let commands::Operation::Metadata { advance, .. } = &mut command.operation
                {
                    *advance |= self.auto_advance;
                }
                result = self.execute_command_from(command, source, ctx)?;
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop::{Recipe, TEMPERATURE_MAX, TEMPERATURE_MIN, TINT_LIMIT};
    #[test]
    fn encoder_values_are_relative() {
        assert_eq!((ticks(1), ticks(127), ticks(3), ticks(125)), (1, -1, 3, -3));
    }
    #[test]
    fn parses_device_messages() {
        assert!(matches!(parse(&[0xB0, 48, 1]), Some(Msg::Cc(48, 1))));
        assert!(matches!(parse(&[0x90, 68, 64]), Some(Msg::Note(68, true))));
        assert!(matches!(parse(&[0x80, 68, 64]), Some(Msg::Note(68, false))));
        assert!(parse(&[0xF8]).is_none());
    }
    #[test]
    fn dials_clamp_and_label_like_sliders() {
        let mut r = Recipe::default();
        assert_eq!(Param::Contrast.turn(&mut r, 3, 0), "+3");
        assert_eq!(Param::Contrast.turn(&mut r, -5, 0), "-2");
        assert_eq!(Param::Exposure.turn(&mut r, 5, 0), "+0.10");
        Param::Highlights.turn(&mut r, 1000, 0);
        assert_eq!(r.highlights, 1.);
        Param::Tint.turn(&mut r, -1000, 0);
        assert_eq!(r.tint, -TINT_LIMIT);
    }
    #[test]
    fn temperature_clockwise_is_warmer_and_stays_in_range() {
        let mut r = Recipe {
            temperature: 5000.,
            ..Recipe::default()
        };
        Param::Temperature.turn(&mut r, 1, 0);
        assert!(r.temperature > 5000.);
        Param::Temperature.turn(&mut r, 100_000, 0);
        assert_eq!(r.temperature, TEMPERATURE_MAX);
        Param::Temperature.turn(&mut r, -100_000, 0);
        assert_eq!(r.temperature, TEMPERATURE_MIN);
    }
    #[test]
    fn faders_turn_the_selected_mixer_channel() {
        let mut r = Recipe::default();
        assert_eq!(Param::Band(2).turn(&mut r, 5, 1), "+5");
        assert!((r.hsl[2][1] - 0.05).abs() < 1e-6);
        assert_eq!((r.hsl[2][0], r.hsl[2][2]), (0., 0.));
        assert_eq!(Param::Band(2).label(1), "Yellow Saturation");
        r.effects.monochrome = true;
        Param::Band(7).turn(&mut r, -3, 1);
        assert!((r.effects.gray_mix[7] + 0.03).abs() < 1e-6);
        assert_eq!(r.hsl[7], [0.; 3]);
    }
    #[test]
    fn sliders_are_set_in_the_units_they_show() {
        let mut r = Recipe::default();
        assert_eq!(Param::Exposure.set(&mut r, 1.5, 0), "+1.50");
        assert_eq!(Param::Exposure.shown(&mut r, 0), 1.5);
        assert_eq!(Param::Contrast.set(&mut r, 35., 0), "+35");
        assert!((r.contrast - 0.35).abs() < 1e-6);
        assert_eq!(Param::Contrast.shown(&mut r, 0), 35.);
        Param::Exposure.set(&mut r, 99., 0);
        assert_eq!(r.exposure, 5.);
        Param::Temperature.set(&mut r, 1., 0);
        assert_eq!(r.temperature, TEMPERATURE_MIN);
        // A band addressed by channel ignores the Mixer's selector and B&W.
        r.effects.monochrome = true;
        Param::Hsl(2, 1).set(&mut r, -40., 0);
        Param::Gray(2).set(&mut r, 10., 0);
        assert!((r.hsl[2][1] + 0.4).abs() < 1e-6);
        assert!((r.effects.gray_mix[2] - 0.1).abs() < 1e-6);
        assert_eq!(Param::Gray(2).label(0), "Yellow Gray");
        assert_eq!(Param::Hsl(2, 1).label(0), "Yellow Saturation");
    }
    #[test]
    fn slider_names_parse_with_bands_and_channels() {
        assert_eq!(Param::parse("Temp"), Some(Param::Temperature));
        assert_eq!(Param::parse("band3.sat"), Some(Param::Hsl(2, 1)));
        assert_eq!(Param::parse("band8.gray"), Some(Param::Gray(7)));
        assert_eq!(Param::parse("band1"), Some(Param::Band(0)));
        assert_eq!(Param::parse("band1.alpha"), None);
        assert_eq!(Param::parse("band0.hue"), None);
        for (name, param) in Param::NAMED {
            assert_eq!(Param::parse(name), Some(param));
        }
    }
    #[test]
    fn config_overrides_defaults() {
        let mut config = Config::defaults();
        config.apply(&serde_json::json!({
            "dials": {"41": "contrast", "33": null},
            "buttons": {"98": "cmd+shift+u", "95": null, "99": "hold:shift"}
        }));
        assert_eq!(config.dials.get(&41), Some(&Param::Contrast));
        assert!(!config.dials.contains_key(&33));
        assert_eq!(
            config.buttons.get(&98),
            Some(&Action::Key(Key::U, command() | SHIFT))
        );
        assert!(!config.buttons.contains_key(&95));
        assert_eq!(config.buttons.get(&99), Some(&Action::Hold(SHIFT)));
    }
    #[test]
    fn default_keys_parse_the_way_the_names_say() {
        assert_eq!(
            parse_action("Cmd+Shift+Z"),
            Some(Action::Key(Key::Z, command() | SHIFT))
        );
        assert_eq!(
            parse_action("backslash"),
            Some(Action::Key(Key::Backslash, Modifiers::NONE))
        );
        assert_eq!(parse_action("hold:alt"), Some(Action::Hold(Modifiers::ALT)));
        assert_eq!(
            parse_action("alt+arrowleft"),
            Some(Action::Key(Key::ArrowLeft, Modifiers::ALT))
        );
        assert_eq!(
            parse_action("Left"),
            Some(Action::Key(Key::ArrowLeft, Modifiers::NONE))
        );
        assert_eq!(parse_action("nonsense"), None);
    }
    #[test]
    fn saved_config_is_the_changes_and_reads_back_the_same() {
        let defaults = Config::defaults();
        let saved = defaults.to_json();
        assert_eq!(saved["dials"], serde_json::json!({}));
        assert_eq!(saved["buttons"], serde_json::json!({}));
        let mut config = Config::defaults();
        config.dials.insert(41, Param::Hsl(2, 1));
        config.dials.insert(42, Param::Gray(7));
        config.dials.remove(&33);
        config
            .buttons
            .insert(50, Action::Key(Key::U, command() | SHIFT));
        config.buttons.insert(110, Action::Hold(Modifiers::ALT));
        config.buttons.insert(111, Action::Mixer(2));
        config.buttons.remove(&95);
        config.photo_dial = None;
        config.socket = false;
        let mut read = Config::defaults();
        read.apply(&config.to_json());
        assert_eq!(read, config);
    }
    #[test]
    fn every_default_spells_back_to_itself() {
        let config = Config::defaults();
        for (cc, param) in &config.dials {
            assert_eq!(Param::parse(&param.spec()), Some(*param), "CC {cc}");
        }
        for (note, action) in &config.buttons {
            assert_eq!(
                parse_action(&action_spec(*action)),
                Some(*action),
                "note {note}"
            );
        }
        for (label, spec) in settings::PRESETS {
            assert!(
                spec.is_empty() || parse_action(spec).is_some(),
                "{label}: {spec}"
            );
        }
    }
    #[test]
    fn every_device_message_is_listed_for_the_page() {
        // The page lists what the device sends; the defaults must be among it.
        let config = Config::defaults();
        for cc in config.dials.keys().chain(config.photo_dial.iter()) {
            assert!(settings::DIALS.iter().any(|(_, n)| n == cc), "CC {cc}");
        }
        for note in config.buttons.keys() {
            assert!(
                settings::BUTTONS.iter().any(|(_, n)| n == note),
                "note {note}"
            );
        }
    }
    #[test]
    fn held_modifiers_join_and_leave() {
        let held = Modifiers::NONE | SHIFT;
        assert!(held.shift);
        assert_eq!(without(held, SHIFT), Modifiers::NONE);
        let both = held | command();
        assert!(both.shift && both.command);
        assert_eq!(without(both, command()), SHIFT);
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    #[test]
    fn disconnect_clears_modifiers_even_with_old_input_queued() {
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
        let (tx, rx) = mpsc::sync_channel(1);
        e.surface = Surface::new(Config::defaults(), rx);
        let old = reset_connection(&e.surface.status, &ctx);
        e.control_messages(
            vec![Msg::Midi(
                e.surface.status.clone(),
                old,
                Box::new(Msg::Note(66, true)),
            )],
            &ctx,
        )
        .unwrap();
        assert!(e.surface.held.shift);
        tx.send(Msg::Midi(
            e.surface.status.clone(),
            old,
            Box::new(Msg::Note(68, true)),
        ))
        .unwrap();
        // The reset cannot be lost even though the bounded queue is full.
        let new = reset_connection(&e.surface.status, &ctx);
        e.control_commands(&ctx);
        assert_eq!(e.surface.held, Modifiers::NONE);
        let undo = e.surface.translate(Msg::Note(95, true)).unwrap().unwrap();
        assert!(matches!(
            undo.operation,
            commands::Operation::Action(commands::Action::Undo)
        ));
        e.control_messages(
            vec![Msg::Midi(
                e.surface.status.clone(),
                new,
                Box::new(Msg::Note(66, true)),
            )],
            &ctx,
        )
        .unwrap();
        assert!(e.surface.held.shift);
        reset_connection(&e.surface.status, &ctx);
        e.control_commands(&ctx);
        assert_eq!(e.surface.held, Modifiers::NONE);
        // Changing the configured port creates a separate listener identity.
        let old_source = e.surface.status.clone();
        let old_epoch = locked(&old_source).epoch;
        e.surface.restart_midi();
        e.control_messages(
            vec![Msg::Midi(
                old_source,
                old_epoch,
                Box::new(Msg::Note(66, true)),
            )],
            &ctx,
        )
        .unwrap();
        assert_eq!(e.surface.held, Modifiers::NONE);
    }

    #[test]
    fn photo_dial_ignores_grid_preserves_loupe_and_navigates_develop() -> anyhow::Result<()> {
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
        e.onboarding.visible = false;
        let dir = tempfile::tempdir()?;
        let photos = dir.path().join("photos");
        std::fs::create_dir(&photos)?;
        for name in ["a.DNG", "b.DNG"] {
            std::fs::write(photos.join(name), b"synthetic")?;
        }
        let db = dir.path().join("catalog.rawmakase");
        let mut catalog = crate::catalog::Catalog::create(&db)?;
        catalog.add_folder(&photos)?;
        drop(catalog);
        e.library = Some(Box::new(super::super::library::Library::load(
            &db,
            ctx.clone(),
        )?));
        let library = e.library.as_mut().unwrap();
        let first = library.photos[0].id;
        library.make_active(first);
        let second = library.navigate(first, 1).unwrap();
        e.library_mode = true;
        e.control_messages(vec![Msg::Cc(48, 1)], &ctx).unwrap();
        assert!(e.library_mode);
        assert_eq!(e.library.as_ref().unwrap().selected(), Some(first));
        e.library.as_mut().unwrap().open_loupe();
        e.control_messages(vec![Msg::Cc(48, 1)], &ctx).unwrap();
        assert!(e.library_mode);
        assert!(e.library.as_ref().unwrap().loupe_open());
        assert_eq!(e.library.as_ref().unwrap().selected(), Some(second));
        // Semantic API navigation and device arrows preserve Loupe too.
        e.control_messages(
            vec![Msg::Command(Command::new(commands::Operation::Navigate(
                -1,
            )))],
            &ctx,
        )
        .unwrap();
        assert!(e.library_mode && e.library.as_ref().unwrap().loupe_open());
        assert_eq!(e.library.as_ref().unwrap().selected(), Some(first));
        e.control_messages(
            vec![Msg::Command(Command::new(commands::Operation::Action(
                commands::Action::Next,
            )))],
            &ctx,
        )
        .unwrap();
        assert!(e.library_mode && e.library.as_ref().unwrap().loupe_open());
        assert_eq!(e.library.as_ref().unwrap().selected(), Some(second));
        e.library.as_mut().unwrap().show_grid();
        e.control_messages(
            vec![Msg::Command(Command::new(commands::Operation::Action(
                commands::Action::Previous,
            )))],
            &ctx,
        )
        .unwrap();
        assert!(e.library_mode && !e.library.as_ref().unwrap().loupe_open());
        assert_eq!(e.library.as_ref().unwrap().selected(), Some(first));
        e.library_mode = false;
        e.document.catalog_photo = Some(second);
        e.control_messages(vec![Msg::Cc(48, 127)], &ctx).unwrap();
        assert!(!e.library_mode);
        assert_eq!(e.document.catalog_photo, Some(first));
        Ok(())
    }

    #[test]
    fn modifier_bindings_become_actions_without_keyboard_state() {
        let mut surface = Surface::inactive();
        let command = surface.translate(Msg::Note(92, true)).unwrap().unwrap();
        assert!(matches!(
            command.operation,
            commands::Operation::Action(commands::Action::Copy)
        ));
        let command = surface.translate(Msg::Note(95, true)).unwrap().unwrap();
        assert!(matches!(
            command.operation,
            commands::Operation::Action(commands::Action::Undo)
        ));
        surface.translate(Msg::Note(66, true)).unwrap();
        let command = surface.translate(Msg::Note(95, true)).unwrap().unwrap();
        assert!(matches!(
            command.operation,
            commands::Operation::Action(commands::Action::Redo)
        ));
        surface.translate(Msg::Note(66, false)).unwrap();
        assert_eq!(surface.held, Modifiers::NONE);
        let command = surface.translate(Msg::Note(51, true)).unwrap().unwrap();
        assert!(
            matches!(command.operation,commands::Operation::Metadata {edit:crate::app::photo_metadata::Edit::ToggleLabel(ref label),advance:false} if label=="Red")
        );
        surface.translate(Msg::Note(66, true)).unwrap();
        let command = surface.translate(Msg::Note(80, true)).unwrap().unwrap();
        assert!(matches!(
            command.operation,
            commands::Operation::Metadata {
                edit: crate::app::photo_metadata::Edit::Rating(1),
                advance: true
            }
        ));
    }
    #[test]
    fn each_socket_request_runs_and_replies_before_the_next() {
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
        e.onboarding.visible = false;
        let (tx, rx) = mpsc::sync_channel(4);
        e.surface = Surface::new(Config::defaults(), rx);
        let (first, r1) = socket::test_request(vec![Msg::Command(Command::new(
            commands::Operation::Set(Param::Exposure, 1.),
        ))]);
        let (second, r2) =
            socket::test_request(vec![Msg::Command(Command::new(commands::Operation::State))]);
        tx.send(Msg::Request(first)).unwrap();
        tx.send(Msg::Request(second)).unwrap();
        e.control_commands(&ctx);
        assert_eq!(r1.recv().unwrap().unwrap_err().code, "no_document");
        assert!(r2.recv().unwrap().is_ok());
    }
}
