//! MIDI control surfaces such as the Loupedeck+: dials turn the Basic sliders and
//! buttons press the keys the keyboard would. The listener reconnects when the
//! device is plugged in; `midi.json` in the data folder overrides the mapping.
//! The same commands also arrive from the `rawmakase-ctl` tool, over a local
//! socket (see [`socket`]).
mod socket;

use super::Editor;
use super::inspector::BANDS;
use super::widgets::{history_step_id, slider_text};
use crate::catalog::Photo;
use crate::develop::{Recipe, TEMPERATURE_MAX, TEMPERATURE_MIN, TINT_LIMIT};
use eframe::egui::{self, Event, Key, Modifiers};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
    time::{Duration, Instant},
};

/// Dial ticks closer together than this are one edit in History, like a drag.
const GESTURE: Duration = Duration::from_millis(400);

/// A pause this long ends a turn of the photo dial.
const PHOTO_IDLE: Duration = Duration::from_millis(600);

/// A raw message from the device, or a command from the control socket.
enum Msg {
    Cc(u8, u8),
    Note(u8, bool),
    /// A dial turned by `ticks`, as if its CC had arrived.
    Turn(Param, i32),
    /// A slider set to a value, in the units the slider shows.
    Set(Param, f32),
    /// A button's action, pressed once.
    Action(Action),
    /// The photo dial moved one photo (-1 or +1).
    Photo(i32),
    /// Nothing, but answered with the next frame's state.
    Ping,
    /// Something only the app itself can do, with the result in the state.
    Job(Job),
}

/// A photo to open: by name, or by its catalog id.
#[derive(Debug, PartialEq)]
enum Target {
    Name(String),
    Id(i64),
}

/// A command that acts on the Library rather than on a slider or a key.
#[derive(Debug, PartialEq)]
enum Job {
    /// Open the photo in Develop.
    Open(Target),
    /// Put this in the Library's search box ("" clears it).
    Search(String),
    /// Switch to Develop (true) or the Library grid (false).
    Module(bool),
}

/// A change to a slider, in the order it was asked for.
enum Edit {
    Turn(Param, i32),
    Set(Param, f32),
}

/// The Color Mixer's channels, in the order of `Recipe::hsl`.
const MIXER_CHANNELS: [&str; 3] = ["Hue", "Saturation", "Luminance"];

/// A slider a dial can turn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Param {
    Exposure,
    Contrast,
    Highlights,
    Shadows,
    Whites,
    Blacks,
    Texture,
    Clarity,
    Dehaze,
    Vibrance,
    Saturation,
    Temperature,
    Tint,
    /// A Color Mixer colour band (0 Red .. 7 Magenta), on the channel the
    /// panel's Hue / Sat / Lum selector shows.
    Band(usize),
    /// One channel (0 Hue, 1 Saturation, 2 Luminance) of a Color Mixer band,
    /// whichever channel the panel shows.
    Hsl(usize, usize),
    /// A band's gray mix, which Black & White uses in place of the mixer.
    Gray(usize),
}
impl Param {
    /// The sliders with a name of their own, as `midi.json` and the control
    /// socket spell them.
    const NAMED: [(&'static str, Self); 13] = [
        ("exposure", Self::Exposure),
        ("contrast", Self::Contrast),
        ("highlights", Self::Highlights),
        ("shadows", Self::Shadows),
        ("whites", Self::Whites),
        ("blacks", Self::Blacks),
        ("texture", Self::Texture),
        ("clarity", Self::Clarity),
        ("dehaze", Self::Dehaze),
        ("vibrance", Self::Vibrance),
        ("saturation", Self::Saturation),
        ("temperature", Self::Temperature),
        ("tint", Self::Tint),
    ];
    /// `exposure`, `band3` (the channel the panel shows), `band3.sat` or
    /// `band3.gray`; bands count from 1 (Red).
    fn parse(name: &str) -> Option<Self> {
        let name = name.to_ascii_lowercase();
        if name == "temp" {
            return Some(Self::Temperature);
        }
        if let Some((_, param)) = Self::NAMED.iter().find(|(n, _)| *n == name) {
            return Some(*param);
        }
        let (band, channel) = match name.strip_prefix("band")?.split_once('.') {
            Some((band, channel)) => (band, Some(channel)),
            None => (name.strip_prefix("band")?, None),
        };
        let i = match band.parse::<usize>() {
            Ok(n) if (1..=8).contains(&n) => n - 1,
            _ => return None,
        };
        Some(match channel {
            None => Self::Band(i),
            Some("hue") => Self::Hsl(i, 0),
            Some("sat" | "saturation") => Self::Hsl(i, 1),
            Some("lum" | "luminance") => Self::Hsl(i, 2),
            Some("gray" | "grey") => Self::Gray(i),
            Some(_) => return None,
        })
    }
    /// The name the slider and its History step carry.
    fn label(self, channel: usize) -> String {
        match self {
            Self::Band(i) => return format!("{} {}", BANDS[i], MIXER_CHANNELS[channel]),
            Self::Hsl(i, c) => return format!("{} {}", BANDS[i], MIXER_CHANNELS[c]),
            Self::Gray(i) => return format!("{} Gray", BANDS[i]),
            _ => {}
        }
        match self {
            Self::Exposure => "Exposure",
            Self::Contrast => "Contrast",
            Self::Highlights => "Highlights",
            Self::Shadows => "Shadows",
            Self::Whites => "Whites",
            Self::Blacks => "Blacks",
            Self::Texture => "Texture",
            Self::Clarity => "Clarity",
            Self::Dehaze => "Dehaze",
            Self::Vibrance => "Vibrance",
            Self::Saturation => "Saturation",
            Self::Temperature => "Temp",
            Self::Tint => "Tint",
            Self::Band(_) | Self::Hsl(..) | Self::Gray(_) => unreachable!(),
        }
        .to_string()
    }
    fn value(self, r: &mut Recipe, channel: usize) -> &mut f32 {
        match self {
            // Black & White swaps the mixer for one gray mix per band.
            Self::Band(i) if r.effects.monochrome => &mut r.effects.gray_mix[i],
            Self::Band(i) => &mut r.hsl[i][channel],
            Self::Hsl(i, c) => &mut r.hsl[i][c],
            Self::Gray(i) => &mut r.effects.gray_mix[i],
            Self::Exposure => &mut r.exposure,
            Self::Contrast => &mut r.contrast,
            Self::Highlights => &mut r.highlights,
            Self::Shadows => &mut r.shadows,
            Self::Whites => &mut r.whites,
            Self::Blacks => &mut r.blacks,
            Self::Texture => &mut r.effects.texture,
            Self::Clarity => &mut r.effects.clarity,
            Self::Dehaze => &mut r.effects.dehaze,
            Self::Vibrance => &mut r.vibrance,
            Self::Saturation => &mut r.saturation,
            Self::Temperature => &mut r.temperature,
            Self::Tint => &mut r.tint,
        }
    }
    fn is_white_balance(self) -> bool {
        matches!(self, Self::Temperature | Self::Tint)
    }
    /// The slider's number as it shows: EV, kelvin or tint units, else -100..100.
    fn shown(self, r: &mut Recipe, channel: usize) -> f64 {
        let scale = match self {
            Self::Exposure | Self::Temperature | Self::Tint => 1.,
            _ => 100.,
        };
        (f64::from(*self.value(r, channel)) * scale * 1000.).round() / 1000.
    }
    /// Sets the slider to `shown`, a number as `shown` returns it, and returns
    /// the text for the History step.
    fn set(self, r: &mut Recipe, shown: f32, channel: usize) -> String {
        let v = self.value(r, channel);
        match self {
            Self::Exposure => {
                *v = shown.clamp(-5., 5.);
                slider_text(f64::from(*v), 2, true)
            }
            Self::Temperature => {
                *v = shown.clamp(TEMPERATURE_MIN, TEMPERATURE_MAX);
                slider_text(f64::from(*v), 0, false)
            }
            Self::Tint => {
                *v = shown.clamp(-TINT_LIMIT, TINT_LIMIT);
                slider_text(f64::from(*v), 0, true)
            }
            _ => {
                *v = (shown / 100.).clamp(-1., 1.);
                slider_text(f64::from(*v * 100.), 0, true)
            }
        }
    }
    /// Moves the slider by `ticks` (clockwise positive) and returns the value as
    /// the slider shows it, for the History step.
    fn turn(self, r: &mut Recipe, ticks: i32, channel: usize) -> String {
        let t = ticks as f32;
        let v = self.value(r, channel);
        match self {
            Self::Exposure => {
                *v = (*v + 0.02 * t).clamp(-5., 5.);
                slider_text(f64::from(*v), 2, true)
            }
            // Evenly in mireds, as the slider does; clockwise is warmer.
            Self::Temperature => {
                let mired = (1e6 / *v - 4. * t).max(1e6 / TEMPERATURE_MAX);
                *v = (1e6 / mired).clamp(TEMPERATURE_MIN, TEMPERATURE_MAX);
                slider_text(f64::from(*v), 0, false)
            }
            Self::Tint => {
                *v = (*v + t).clamp(-TINT_LIMIT, TINT_LIMIT);
                slider_text(f64::from(*v), 0, true)
            }
            _ => {
                *v = (*v + 0.01 * t).clamp(-1., 1.);
                slider_text(f64::from(*v * 100.), 0, true)
            }
        }
    }
}

/// What a button does.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    /// Presses a key, with the modifiers of any held modifier buttons.
    Key(Key, Modifiers),
    /// Acts as a modifier while held.
    Hold(Modifiers),
    /// Shows Hue, Saturation or Luminance (0..=2) in the Color Mixer, which is
    /// what the Band faders then turn.
    Mixer(usize),
    /// Black & White on or off, as the panel's B&W button does.
    ToggleMono,
}

#[derive(Debug)]
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
impl Config {
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
            socket: true,
            dials: dials.collect(),
            buttons: buttons.into_iter().collect(),
        }
    }
    /// The defaults, changed by `midi.json`:
    /// `{"port": "Loupedeck", "socket": true, "dials": {"41": "contrast"}, "buttons": {"98": "h", "95": null}}`
    /// where `null` removes a default and a button is `"cmd+shift+z"` or `"hold:shift"`.
    fn load() -> Self {
        let mut config = Self::defaults();
        let path = crate::storage::data_dir().join("midi.json");
        if let Ok(text) = std::fs::read_to_string(&path) {
            match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(json) => config.apply(&json),
                Err(e) => eprintln!("{}: {e}", path.display()),
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

fn parse(bytes: &[u8]) -> Option<Msg> {
    match *bytes {
        [status, d1, d2] if status & 0xF0 == 0xB0 => Some(Msg::Cc(d1, d2)),
        [status, d1, d2] if status & 0xF0 == 0x90 && d2 > 0 => Some(Msg::Note(d1, true)),
        [status, d1, _] if status & 0xF0 == 0x80 => Some(Msg::Note(d1, false)),
        [status, d1, 0] if status & 0xF0 == 0x90 => Some(Msg::Note(d1, false)),
        _ => None,
    }
}

/// Listens for the device on a thread of its own, finding it again when it is
/// plugged back in. Where there is no MIDI backend nothing is ever sent.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn listen(port: String, tx: Sender<Msg>, ctx: egui::Context) {
    use midir::{MidiInput, MidiInputConnection};
    let spawned = std::thread::Builder::new()
        .name("midi".into())
        .spawn(move || {
            let mut connected: Option<(String, String, MidiInputConnection<()>)> = None;
            let mut last = Instant::now();
            loop {
                // A gap far beyond the poll interval means the Mac slept; the
                // device comes back as a new endpoint behind the same name.
                if last.elapsed() > Duration::from_secs(10) {
                    connected = None;
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
                        }
                        Some(_) => {}
                        None => {
                            if let Some(p) = ports.iter().find(|p| named(p).contains(&port)) {
                                let (name, id) = (named(p), p.id());
                                let (tx, ctx) = (tx.clone(), ctx.clone());
                                connected = input
                                    .connect(
                                        p,
                                        "RAWmakase input",
                                        move |_, bytes, _| {
                                            if let Some(msg) = parse(bytes) {
                                                let _ = tx.send(msg);
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
                std::thread::sleep(Duration::from_secs(2));
            }
        });
    if let Err(e) = spawned {
        eprintln!("MIDI listener: {e}");
    }
}
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn listen(_: String, _: Sender<Msg>, _: egui::Context) {}

pub(super) struct Surface {
    rx: Receiver<Msg>,
    config: Config,
    held: Modifiers,
    /// Slider changes since the frame began editing, in order.
    pending: Vec<Edit>,
    /// What the control socket waits on, and the state it answers with.
    shared: Arc<socket::Shared>,
    /// How many socket commands the last `poll` is known to have read.
    consumed: u64,
    /// Library commands waiting for the start of the next frame.
    jobs: Vec<Job>,
    /// How the last of them went, for the next state.
    result: Option<serde_json::Value>,
    /// The Mixer channel a button asked for since the last frame.
    mixer: Option<usize>,
    /// How many times Black & White was asked for since the last frame.
    mono_toggles: u32,
    /// Ticks of the photo dial not yet worth a photo, and when it last turned.
    photo_ticks: i32,
    photo_dir: i32,
    last_photo: Option<Instant>,
    last_turn: Option<Instant>,
}
impl Surface {
    pub(super) fn start(ctx: &egui::Context) -> Self {
        let config = Config::load();
        let (tx, rx) = mpsc::channel();
        listen(config.port.clone(), tx.clone(), ctx.clone());
        let surface = Self::new(config, rx);
        if surface.config.socket {
            socket::start(tx, ctx.clone(), surface.shared.clone());
        }
        surface
    }
    fn new(config: Config, rx: Receiver<Msg>) -> Self {
        Self {
            rx,
            config,
            held: Modifiers::NONE,
            pending: Vec::new(),
            shared: Arc::default(),
            consumed: 0,
            jobs: Vec::new(),
            result: None,
            mixer: None,
            mono_toggles: 0,
            photo_ticks: 0,
            photo_dir: 0,
            last_photo: None,
            last_turn: None,
        }
    }
    /// The photo to move to, -1 or +1, once the photo dial has turned a detent;
    /// one at a time, so the photo is loaded before the next.
    fn photo_step(&mut self) -> i32 {
        if self
            .last_photo
            .is_none_or(|t| t.elapsed() > Duration::from_millis(600))
        {
            self.photo_ticks = 0;
        }
        if self.photo_ticks.abs() < self.config.photo_detent {
            return 0;
        }
        let step = self.photo_ticks.signum();
        self.photo_ticks -= step * self.config.photo_detent;
        step
    }
    /// A dial is still being turned, so its steps are one History entry.
    pub(super) fn turning(&self) -> bool {
        self.last_turn.is_some_and(|t| t.elapsed() < GESTURE)
    }
    /// A turn of the photo dial by `t` ticks.
    fn photo_turn(&mut self, t: i32) {
        let fresh = self.last_photo.is_none_or(|l| l.elapsed() > PHOTO_IDLE);
        // A new turn, or one the other way, moves at once; the dial
        // has no detents to wait for.
        if fresh || t.signum() != self.photo_dir {
            self.photo_ticks = t.signum() * self.config.photo_detent;
        } else {
            self.photo_ticks += t;
        }
        self.photo_dir = t.signum();
        self.last_photo = Some(Instant::now());
    }
    /// A button going down or up.
    fn act(&mut self, action: Action, down: bool, events: &mut Vec<Event>) {
        match action {
            Action::Mixer(channel) if down => self.mixer = Some(channel),
            Action::ToggleMono if down => self.mono_toggles += 1,
            Action::Hold(m) => {
                self.held = if down {
                    self.held | m
                } else {
                    without(self.held, m)
                };
            }
            Action::Key(key, modifiers) if down => {
                let modifiers = modifiers | self.held;
                for pressed in [true, false] {
                    events.push(Event::Key {
                        key,
                        physical_key: Some(key),
                        pressed,
                        repeat: false,
                        modifiers,
                    });
                }
            }
            _ => {}
        }
    }
    /// Reads what arrived: buttons become key presses now, dials wait for the
    /// edit frame.
    fn poll(&mut self) -> Vec<Event> {
        // Read before the queue, so every command it counts is in the queue.
        self.consumed = self.shared.sent();
        let mut events = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Cc(cc, value) if self.config.photo_dial == Some(cc) => {
                    self.photo_turn(ticks(value));
                }
                Msg::Cc(cc, value) => {
                    if let Some(&param) = self.config.dials.get(&cc) {
                        self.turn(param, ticks(value));
                    }
                }
                Msg::Note(note, down) => {
                    if let Some(&action) = self.config.buttons.get(&note) {
                        self.act(action, down, &mut events);
                    }
                }
                Msg::Turn(param, t) => self.turn(param, t),
                Msg::Set(param, v) => self.pending.push(Edit::Set(param, v)),
                Msg::Action(action) => self.act(action, true, &mut events),
                // Exactly one photo, however soon after the last.
                Msg::Photo(step) => {
                    self.photo_ticks += step * self.config.photo_detent;
                    self.photo_dir = step.signum();
                    self.last_photo = Some(Instant::now());
                }
                Msg::Ping => {}
                Msg::Job(job) => self.jobs.push(job),
            }
        }
        events
    }
    fn turn(&mut self, param: Param, t: i32) {
        match self.pending.last_mut() {
            Some(Edit::Turn(p, n)) if *p == param => *n += t,
            _ => self.pending.push(Edit::Turn(param, t)),
        }
    }
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

/// The photos a name stands for: the filename or the path, in any case, with or
/// without the extension; else the ones whose name contains it. A master wins
/// over its virtual copies, which share its filename.
fn find_photos<'a>(photos: &'a [Photo], name: &str) -> Vec<&'a Photo> {
    let wanted = name.trim().to_lowercase();
    let by_path = wanted.contains(['/', '\\']);
    let text = |p: &Photo| {
        if by_path {
            p.path.to_string_lossy().to_lowercase()
        } else {
            p.filename.to_lowercase()
        }
    };
    let stem = |p: &Photo| {
        std::path::Path::new(&text(p))
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
    };
    let pick = |matching: &dyn Fn(&Photo) -> bool| -> Vec<&'a Photo> {
        let all: Vec<&Photo> = photos.iter().filter(|p| matching(p)).collect();
        let masters: Vec<&Photo> = all.iter().copied().filter(|p| p.master.is_none()).collect();
        if masters.is_empty() { all } else { masters }
    };
    if wanted.is_empty() {
        return Vec::new();
    }
    let exact = pick(&|p| text(p) == wanted || (!by_path && stem(p).as_deref() == Some(&wanted)));
    if exact.is_empty() {
        pick(&|p| text(p).contains(&wanted))
    } else {
        exact
    }
}

impl Editor {
    /// Start of the frame: button presses reach the shortcut handlers as if typed.
    pub(super) fn surface_buttons(&mut self, ctx: &egui::Context) {
        let events = self.surface.poll();
        self.surface_jobs();
        if let Some(channel) = self.surface.mixer.take() {
            // The HSL view, where the selector is, so the screen shows what the faders turn.
            self.view.mixer_adjust = channel;
            self.view.mixer_color = false;
        }
        if self.library_mode {
            self.surface.pending.clear();
            self.surface.mono_toggles = 0;
            self.surface_publish();
        }
        // Only where one photo is shown: the grid, Compare and Survey use the
        // arrows for other things.
        let single = !self.library_mode || self.library.as_ref().is_some_and(|l| l.loupe_open());
        let step = if single { self.surface.photo_step() } else { 0 };
        if !single {
            self.surface.photo_ticks = 0;
        }
        let mut events = events;
        if step != 0 {
            let key = if step > 0 {
                Key::ArrowRight
            } else {
                Key::ArrowLeft
            };
            for pressed in [true, false] {
                events.push(Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                });
            }
            if self.surface.photo_ticks != 0 {
                // More detents waiting: another frame for the next photo.
                ctx.request_repaint();
            }
        }
        if !events.is_empty() {
            ctx.input_mut(|i| i.events.extend(events));
        }
    }
    /// Inside the edit frame: turns the dials' sliders. Dials do nothing without
    /// a photo open in Develop.
    pub(super) fn surface_dials(&mut self, ctx: &egui::Context) {
        self.apply_edits(ctx);
        self.surface_publish();
    }
    /// Library commands from the socket, at the start of the frame.
    fn surface_jobs(&mut self) {
        for job in std::mem::take(&mut self.surface.jobs) {
            let done = match job {
                Job::Search(text) => self.surface_search(&text),
                Job::Module(true) => self.surface_develop(),
                Job::Module(false) => self.surface_library(),
                Job::Open(target) => self.surface_open(&target),
            };
            self.surface.result = Some(match done {
                Ok(mut json) => {
                    json["ok"] = true.into();
                    json
                }
                Err(error) => serde_json::json!({"ok": false, "error": error}),
            });
        }
    }
    fn surface_search(&mut self, text: &str) -> Result<serde_json::Value, String> {
        let library = self.library.as_mut().ok_or("no catalog is open")?;
        let mut layout = library.layout();
        layout.query = text.to_string();
        layout.filters_off = false;
        library.apply_layout(&layout);
        Ok(serde_json::json!({"query": text}))
    }
    /// G: the Library's grid.
    fn surface_library(&mut self) -> Result<serde_json::Value, String> {
        if self.library.is_none() {
            return Err("no catalog is open".into());
        }
        if !self.flush() {
            return Err("an edit is still being saved".into());
        }
        self.library_mode = true;
        if let Some(library) = &mut self.library {
            library.show_grid();
        }
        Ok(serde_json::json!({}))
    }
    /// D: Develop on the selected photo, or the first one shown.
    fn surface_develop(&mut self) -> Result<serde_json::Value, String> {
        if !self.library_mode {
            return Ok(serde_json::json!({}));
        }
        let library = self.library.as_mut().ok_or("no catalog is open")?;
        let id = library
            .selected_or_first()
            .ok_or("the Library shows no photo to open")?;
        self.surface_develop_photo(id)
    }
    fn surface_open(&mut self, target: &Target) -> Result<serde_json::Value, String> {
        let library = self.library.as_mut().ok_or("no catalog is open")?;
        let id = match target {
            Target::Id(id) => library
                .photo(*id)
                .map(|p| p.id)
                .ok_or_else(|| format!("no photo has id {id}"))?,
            Target::Name(name) => match find_photos(&library.photos, name)[..] {
                [] => return Err(format!("no photo matches \"{name}\"")),
                [p] => p.id,
                ref many => {
                    let list: Vec<String> = many
                        .iter()
                        .take(10)
                        .map(|p| format!("{} {} ({})", p.id, p.filename, p.path.display()))
                        .collect();
                    return Err(format!(
                        "{} photos match \"{name}\"; open one with --id: {}",
                        many.len(),
                        list.join("; ")
                    ));
                }
            },
        };
        // Leaves a filter that hides it, as clicking it elsewhere would.
        library.reveal(id);
        self.surface_develop_photo(id)
    }
    fn surface_develop_photo(&mut self, id: i64) -> Result<serde_json::Value, String> {
        self.develop_catalog_photo(id);
        if self.library_mode {
            return Err(match &self.not_editable {
                Some((_, reason)) => reason.clone(),
                None => "RAWmakase is busy or has an edit to save first".into(),
            });
        }
        let photo = self.library.as_ref().and_then(|l| l.photo(id));
        Ok(
            serde_json::json!({"opened": photo.map(|p| serde_json::json!({
                "id": p.id,
                "filename": p.filename,
                "path": p.path.display().to_string(),
            }))}),
        )
    }
    fn apply_edits(&mut self, ctx: &egui::Context) {
        let pending = std::mem::take(&mut self.surface.pending);
        let toggles = std::mem::take(&mut self.surface.mono_toggles);
        if self.document.metadata.is_none() {
            return;
        }
        if toggles % 2 == 1 {
            let mono = &mut self.document.recipe.effects.monochrome;
            *mono = !*mono;
        }
        if pending.is_empty() {
            return;
        }
        // "All" shows three channels at once; the faders then turn Hue.
        let channel = self.view.mixer_adjust.min(2);
        let mut step = None;
        let mut turned = false;
        for edit in pending {
            let r = &mut self.document.recipe;
            let (param, before, shown) = match edit {
                Edit::Turn(_, 0) => continue,
                Edit::Turn(param, ticks) => {
                    turned = true;
                    let before = *param.value(r, channel);
                    (param, before, param.turn(r, ticks, channel))
                }
                Edit::Set(param, value) => {
                    let before = *param.value(r, channel);
                    (param, before, param.set(r, value, channel))
                }
            };
            if param.is_white_balance()
                && before != *param.value(r, channel)
                && let Some(m) = &self.document.metadata
            {
                r.update_wb(m);
                r.auto_white_balance = None;
            }
            step = Some((param.label(channel), shown));
        }
        if let Some(step) = step {
            ctx.data_mut(|d| d.insert_temp(history_step_id(), step));
        }
        if turned {
            self.surface.last_turn = Some(Instant::now());
            // One more frame once the turning stops, to close the History entry.
            ctx.request_repaint_after(GESTURE + Duration::from_millis(50));
        }
    }
    /// Tells the control socket what the app looks like now, if it asked.
    fn surface_publish(&mut self) {
        let result = self.surface.result.take();
        let surface = &self.surface;
        surface.shared.publish(surface.consumed, || {
            let develop = !self.library_mode && self.document.metadata.is_some();
            let mut recipe = self.document.recipe.clone();
            let channel = self.view.mixer_adjust.min(2);
            let mut values = serde_json::Map::new();
            if develop {
                let named = Param::NAMED.iter().map(|&(name, p)| (name.to_string(), p));
                let bands = (0..8).flat_map(|i| {
                    let band = i + 1;
                    ["hue", "sat", "lum", "gray"].into_iter().enumerate().map(
                        move |(c, name)| {
                            let p = if c < 3 { Param::Hsl(i, c) } else { Param::Gray(i) };
                            (format!("band{band}.{name}"), p)
                        },
                    )
                });
                for (name, param) in named.chain(bands) {
                    values.insert(name, param.shown(&mut recipe, channel).into());
                }
            }
            let mixer_channel = ["hue", "sat", "lum"][channel];
            serde_json::json!({
                "mode": if self.library_mode { "library" } else { "develop" },
                "photo": develop.then(|| self.document.path.as_ref().map(|p| p.display().to_string())).flatten(),
                "black_and_white": develop && recipe.effects.monochrome,
                "mixer_channel": mixer_channel,
                "photo_id": develop.then_some(self.document.catalog_photo).flatten(),
                "loaded": develop && self.document.full().is_some() && !self.load.is_running(),
                "values": values,
                "result": result,
            })
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn commands_apply_in_order_and_photo_steps_are_exact() {
        let (tx, rx) = mpsc::channel();
        let mut surface = Surface::new(Config::defaults(), rx);
        tx.send(Msg::Turn(Param::Tint, 1)).unwrap();
        tx.send(Msg::Set(Param::Tint, 10.)).unwrap();
        tx.send(Msg::Turn(Param::Tint, 1)).unwrap();
        surface.poll();
        assert!(matches!(
            surface.pending[..],
            [Edit::Turn(..), Edit::Set(..), Edit::Turn(..)]
        ));
        // Two photos in a row, however close together.
        tx.send(Msg::Photo(1)).unwrap();
        tx.send(Msg::Photo(1)).unwrap();
        surface.poll();
        assert_eq!(surface.photo_step(), 1);
        assert_eq!(surface.photo_step(), 1);
        assert_eq!(surface.photo_step(), 0);
    }

    fn photo(id: i64, path: &str, master: Option<i64>) -> Photo {
        let path = std::path::PathBuf::from(path);
        Photo {
            id,
            folder: 1,
            filename: path.file_name().unwrap().to_string_lossy().into_owned(),
            path,
            captured: String::new(),
            rating: 0,
            flag: 0,
            label: String::new(),
            format: "RAF".into(),
            copy_name: String::new(),
            master,
            keywords: String::new(),
            has_lightroom_edits: false,
        }
    }
    fn ids(photos: Vec<&Photo>) -> Vec<i64> {
        photos.into_iter().map(|p| p.id).collect()
    }

    #[test]
    fn photos_are_found_by_name() {
        let photos = [
            photo(1, "/a/DSCF0042.RAF", None),
            photo(2, "/a/DSCF0042.RAF", Some(1)),
            photo(3, "/a/DSCF0043.RAF", None),
            photo(4, "/b/DSCF0042.RAF", None),
            photo(5, "/b/portrait.jpg", None),
        ];
        // Case, extension and stem; a master wins over its copy.
        assert_eq!(ids(find_photos(&photos, "dscf0043")), [3]);
        assert_eq!(ids(find_photos(&photos, "PORTRAIT.JPG")), [5]);
        assert_eq!(ids(find_photos(&photos, "portrait")), [5]);
        // The same name in two folders is ambiguous, but copies are not extra.
        assert_eq!(ids(find_photos(&photos, "DSCF0042")), [1, 4]);
        // A path picks one.
        assert_eq!(ids(find_photos(&photos, "/b/dscf0042.raf")), [4]);
        // Else a part of the name, if it fits.
        assert_eq!(ids(find_photos(&photos, "0043")), [3]);
        assert_eq!(ids(find_photos(&photos, "dscf")), [1, 3, 4]);
        assert!(find_photos(&photos, "nothing").is_empty());
        assert!(find_photos(&photos, "  ").is_empty());
    }

    #[test]
    fn photo_dial_moves_at_once_then_once_per_detent() {
        let (tx, rx) = mpsc::channel();
        let mut surface = Surface::new(Config::defaults(), rx);
        tx.send(Msg::Cc(48, 1)).unwrap();
        surface.poll();
        assert_eq!(surface.photo_step(), 1, "the first tick moves");
        assert_eq!(surface.photo_step(), 0);
        tx.send(Msg::Cc(48, 1)).unwrap();
        surface.poll();
        assert_eq!(surface.photo_step(), 0, "one more tick is half a detent");
        tx.send(Msg::Cc(48, 1)).unwrap();
        surface.poll();
        assert_eq!(surface.photo_step(), 1);
        // The other way moves at once too.
        tx.send(Msg::Cc(48, 127)).unwrap();
        surface.poll();
        assert_eq!(surface.photo_step(), -1);
        // After a pause the leftover is forgotten.
        tx.send(Msg::Cc(48, 127)).unwrap();
        surface.poll();
        surface.last_photo = Some(Instant::now() - Duration::from_secs(1));
        assert_eq!(surface.photo_step(), 0);
        assert_eq!(surface.photo_ticks, 0);
    }

    #[test]
    fn p_buttons_rate_pick_and_reject() {
        let (tx, rx) = mpsc::channel();
        let mut surface = Surface::new(Config::defaults(), rx);
        for note in 80..=87 {
            tx.send(Msg::Note(note, true)).unwrap();
        }
        let keys: Vec<Key> = surface
            .poll()
            .into_iter()
            .filter_map(|e| match e {
                Event::Key {
                    key, pressed: true, ..
                } => Some(key),
                _ => None,
            })
            .collect();
        let expected = [
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num0,
            Key::P,
            Key::X,
        ];
        assert_eq!(keys, expected);
    }

    #[test]
    fn labels_clipping_and_black_and_white_buttons() {
        let (tx, rx) = mpsc::channel();
        let mut surface = Surface::new(Config::defaults(), rx);
        for note in [51, 52, 53, 54, 97, 49] {
            tx.send(Msg::Note(note, true)).unwrap();
        }
        let keys: Vec<Key> = surface
            .poll()
            .into_iter()
            .filter_map(|e| match e {
                Event::Key {
                    key, pressed: true, ..
                } => Some(key),
                _ => None,
            })
            .collect();
        assert_eq!(
            keys,
            [Key::Num6, Key::Num7, Key::Num8, Key::Num9, Key::J, Key::Z]
        );
        tx.send(Msg::Note(101, true)).unwrap();
        tx.send(Msg::Note(101, false)).unwrap();
        assert!(surface.poll().is_empty());
        assert_eq!(surface.mono_toggles, 1, "only the press toggles");
        assert_eq!(parse_action("toggle:bw"), Some(Action::ToggleMono));
    }

    #[test]
    fn default_faders_and_mixer_buttons() {
        let config = Config::defaults();
        assert_eq!(config.dials.get(&17), Some(&Param::Band(0)));
        assert_eq!(config.dials.get(&24), Some(&Param::Band(7)));
        assert_eq!(config.buttons.get(&99), Some(&Action::Mixer(1)));
        assert_eq!(Param::parse("band8"), Some(Param::Band(7)));
        assert_eq!(Param::parse("band9"), None);
        assert_eq!(parse_action("mixer:lum"), Some(Action::Mixer(2)));
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
    fn held_modifiers_join_and_leave() {
        let held = Modifiers::NONE | SHIFT;
        assert!(held.shift);
        assert_eq!(without(held, SHIFT), Modifiers::NONE);
        let both = held | command();
        assert!(both.shift && both.command);
        assert_eq!(without(both, command()), SHIFT);
    }

    #[test]
    fn buttons_become_key_events() {
        let (tx, rx) = mpsc::channel();
        let mut surface = Surface::new(Config::defaults(), rx);
        tx.send(Msg::Note(95, true)).unwrap();
        tx.send(Msg::Note(95, false)).unwrap();
        let events = surface.poll();
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0],
            Event::Key {
                key: Key::Z,
                pressed: true,
                ..
            }
        ));
    }
}
