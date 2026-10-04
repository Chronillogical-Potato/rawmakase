//! Preferences > Automation: what each Loupedeck+ dial and button does, and
//! whether `rawmakase-ctl` may connect. Changes apply at once and are saved
//! to `midi.json` (see [`Config::save`]).
use super::{Config, Last, Param, parse_action};
use crate::app::{
    Editor,
    inspector::BANDS,
    preferences::{gap, group},
    theme,
    widgets::form_row,
};
use eframe::egui;
use std::time::Duration;

/// The device's dials and faders: name and CC (tools/loupedeck/controls.json).
pub(super) const DIALS: [(&str, u8); 22] = [
    ("Fader P1", 17),
    ("Fader P2", 18),
    ("Fader P3", 19),
    ("Fader P4", 20),
    ("Fader P5", 21),
    ("Fader P6", 22),
    ("Fader P7", 23),
    ("Fader P8", 24),
    ("Exposure", 33),
    ("Blacks", 34),
    ("Whites", 35),
    ("Saturation", 36),
    ("Vibrance", 37),
    ("Temperature", 38),
    ("Tint", 39),
    ("Highlights", 40),
    ("D1", 41),
    ("D2", 42),
    ("Shadows", 44),
    ("Clarity", 45),
    ("Contrast", 46),
    ("Control Dial", 48),
];

/// The device's buttons: name and note.
pub(super) const BUTTONS: [(&str, u8); 40] = [
    ("C1", 49),
    ("C2", 50),
    ("C3", 51),
    ("C4", 52),
    ("C5", 53),
    ("C6", 54),
    ("Col", 65),
    ("Shift", 66),
    ("Ctrl", 67),
    ("Command", 68),
    ("Alt", 69),
    ("Tab", 70),
    ("Up Arrow", 76),
    ("Bottom Arrow", 77),
    ("Left Arrow", 78),
    ("Right Arrow", 79),
    ("P1", 80),
    ("P2", 81),
    ("P3", 82),
    ("P4", 83),
    ("P5", 84),
    ("P6", 85),
    ("P7", 86),
    ("P8", 87),
    ("Export", 88),
    ("Copy", 92),
    ("Paste", 93),
    ("Undo", 95),
    ("Redo", 96),
    ("Screen Mode", 97),
    ("Hue", 98),
    ("Sat", 99),
    ("Lum", 100),
    ("Clr/BW", 101),
    ("Before After", 102),
    ("Fn", 110),
    ("L1", 114),
    ("L2", 115),
    ("L3", 116),
    ("Custom mode", 117),
];

/// What a button can be set to from the list: its name and the key it presses,
/// spelled as in `midi.json`. Anything else is typed in the field beside it.
pub(super) const PRESETS: [(&str, &str); 45] = [
    ("Do nothing", ""),
    ("Undo", "cmd+z"),
    ("Redo", "cmd+shift+z"),
    ("Previous photo", "left"),
    ("Next photo", "right"),
    ("Arrow up", "arrowup"),
    ("Arrow down", "arrowdown"),
    ("Star rating 0", "0"),
    ("Star rating 1", "1"),
    ("Star rating 2", "2"),
    ("Star rating 3", "3"),
    ("Star rating 4", "4"),
    ("Star rating 5", "5"),
    ("Pick", "p"),
    ("Reject", "x"),
    ("Unflag", "u"),
    ("Label red", "6"),
    ("Label yellow", "7"),
    ("Label green", "8"),
    ("Label blue", "9"),
    ("Before / after", "backslash"),
    ("Show clipping", "j"),
    ("Toggle zoom", "z"),
    ("Crop & Straighten", "r"),
    ("Spot removal", "q"),
    ("White balance selector", "w"),
    ("Masking", "shift+w"),
    ("Convert to black & white or color", "toggle:bw"),
    ("Color Mixer: Hue", "mixer:hue"),
    ("Color Mixer: Saturation", "mixer:sat"),
    ("Color Mixer: Luminance", "mixer:lum"),
    ("Copy settings…", "cmd+shift+c"),
    ("Paste settings", "cmd+shift+v"),
    ("Paste settings from previous", "cmd+alt+v"),
    ("Sync settings…", "cmd+shift+s"),
    ("Reset all settings", "cmd+shift+r"),
    ("Auto tone", "cmd+shift+u"),
    ("Export…", "cmd+shift+e"),
    ("Export with previous", "cmd+alt+shift+e"),
    ("Library grid", "g"),
    ("Develop", "d"),
    ("Hold Shift", "hold:shift"),
    ("Hold Command", "hold:cmd"),
    ("Hold Alt", "hold:alt"),
    ("Show in Loupe", "e"),
];

/// What a dial does.
#[derive(Clone, Copy, PartialEq)]
enum DialUse {
    None,
    Slider(Param),
    /// Previous / next photo.
    Photos,
}
impl DialUse {
    fn label(self) -> String {
        match self {
            DialUse::None => "Do nothing".into(),
            DialUse::Photos => "Previous / next photo".into(),
            DialUse::Slider(Param::Band(i)) => format!("Color Mixer: {}", BANDS[i]),
            DialUse::Slider(p) => {
                let name = p.spec();
                let mut chars = name.chars();
                chars
                    .next()
                    .map(|c| c.to_uppercase().chain(chars).collect())
                    .unwrap_or_default()
            }
        }
    }
    /// Every choice, in the order of the list.
    fn all() -> Vec<DialUse> {
        let sliders = Param::NAMED.iter().map(|(_, p)| DialUse::Slider(*p));
        let bands = (0..8).map(|i| DialUse::Slider(Param::Band(i)));
        [DialUse::None]
            .into_iter()
            .chain(sliders)
            .chain(bands)
            .chain([DialUse::Photos])
            .collect()
    }
}

fn dial_use(config: &Config, cc: u8) -> DialUse {
    if config.photo_dial == Some(cc) {
        DialUse::Photos
    } else {
        config
            .dials
            .get(&cc)
            .map_or(DialUse::None, |p| DialUse::Slider(*p))
    }
}
fn set_dial(config: &mut Config, cc: u8, choice: DialUse) {
    config.dials.remove(&cc);
    if config.photo_dial == Some(cc) {
        config.photo_dial = None;
    }
    match choice {
        DialUse::None => {}
        DialUse::Slider(p) => {
            config.dials.insert(cc, p);
        }
        // Only one dial moves between photos.
        DialUse::Photos => config.photo_dial = Some(cc),
    }
}

/// A message as the page shows it: "Exposure (CC 33) ▲".
fn describe(last: Last) -> String {
    match last {
        Last::Dial(cc, t) => {
            let name = DIALS
                .iter()
                .find(|(_, n)| *n == cc)
                .map_or("Unknown", |d| d.0);
            format!("{name} (CC {cc}) {}", if t > 0 { "▲" } else { "▼" })
        }
        Last::Button(note) => {
            let name = BUTTONS
                .iter()
                .find(|(_, n)| *n == note)
                .map_or("Unknown", |b| b.0);
            format!("{name} (Note {note})")
        }
    }
}

fn small(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).size(12.).color(theme::gray(135)));
}
fn hint(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).size(12.).color(theme::gray(135))).wrap());
}

impl Editor {
    pub(in crate::app) fn automation_page(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        // The status follows the device being plugged in.
        ctx.request_repaint_after(Duration::from_millis(500));
        let mut config = self.surface.config.clone();
        let (connected, last) = {
            let status = super::locked(&self.surface.status);
            (status.connected.clone(), status.last)
        };

        group(ui, "Control surface");
        form_row(ui, "Device name", |ui| {
            ui.add(egui::TextEdit::singleline(&mut config.port).desired_width(160.));
            small(ui, "Part of its MIDI port name");
        });
        form_row(ui, "Status", |ui| {
            let text = match &connected {
                Some(name) => format!("Connected to {name}"),
                None if cfg!(any(target_os = "macos", target_os = "windows")) => {
                    format!("{} not found; looking…", self.surface.config.port)
                }
                None => "MIDI is not available on this platform".into(),
            };
            ui.add(egui::Label::new(egui::RichText::new(text).color(theme::gray(225))).truncate());
        });
        form_row(ui, "Last message", |ui| {
            small(
                ui,
                &last.map_or("None yet: press or turn a control".into(), describe),
            );
        });
        form_row(ui, "", |ui| {
            if ui.button("Restore Default Actions").clicked() {
                let defaults = Config::defaults();
                config.dials = defaults.dials;
                config.buttons = defaults.buttons;
                config.photo_dial = defaults.photo_dial;
                config.photo_detent = defaults.photo_detent;
            }
        });
        gap(ui);

        group(ui, "Command line");
        form_row(ui, "Control socket", |ui| {
            ui.checkbox(&mut config.socket, "Let rawmakase-ctl connect");
        });
        form_row(ui, "Status", |ui| {
            let text = match &self.surface.socket {
                Some(socket) => format!("Listening on 127.0.0.1:{}", socket.port()),
                None => "Off".into(),
            };
            ui.add(egui::Label::new(egui::RichText::new(text).color(theme::gray(225))).truncate());
        });
        form_row(ui, "", |ui| {
            hint(
                ui,
                "RAWmakase listens on this computer only, and only a program that can read control.json in the data folder (it holds a random token) can send commands. Turning it off stops listening and removes that file.",
            );
        });
        gap(ui);

        group(ui, "Dials and faders");
        let choices = DialUse::all();
        for (name, cc) in DIALS {
            let now = dial_use(&config, cc);
            let mut choice = now;
            form_row(ui, name, |ui| {
                egui::ComboBox::from_id_salt(("automation-dial", cc))
                    .width(210.)
                    .selected_text(now.label())
                    .show_ui(ui, |ui| {
                        for option in &choices {
                            ui.selectable_value(&mut choice, *option, option.label());
                        }
                    });
                small(ui, &format!("CC {cc}"));
            });
            if choice != now {
                set_dial(&mut config, cc, choice);
            }
        }
        gap(ui);

        group(ui, "Buttons");
        for (name, note) in BUTTONS {
            let current = config.buttons.get(&note).copied();
            let preset = PRESETS
                .iter()
                .find(|(_, spec)| match current {
                    None => spec.is_empty(),
                    Some(action) => !spec.is_empty() && parse_action(spec) == Some(action),
                })
                .map(|(label, _)| *label);
            let mut picked = None;
            let mut typed = None;
            form_row(ui, name, |ui| {
                egui::ComboBox::from_id_salt(("automation-button", note))
                    .width(170.)
                    .selected_text(preset.unwrap_or("Custom"))
                    .show_ui(ui, |ui| {
                        for (label, spec) in PRESETS {
                            if ui.selectable_label(preset == Some(label), label).clicked() {
                                picked = Some(spec);
                            }
                        }
                    });
                // What is typed stays as typed while the field has focus, even
                // while it is not yet a key.
                let id = egui::Id::new(("automation-keys", note));
                let mut text = ui
                    .data(|d| d.get_temp::<String>(id))
                    .unwrap_or_else(|| current.map(super::action_spec).unwrap_or_default());
                let valid = text.trim().is_empty() || parse_action(text.trim()).is_some();
                let response = ui.add(
                    egui::TextEdit::singleline(&mut text)
                        .desired_width(100.)
                        .hint_text("key")
                        .text_color_opt((!valid).then(|| egui::Color32::from_rgb(230, 100, 90))),
                );
                if response.changed() {
                    ui.data_mut(|d| d.insert_temp(id, text.clone()));
                    typed = Some(text);
                }
                if response.lost_focus() {
                    ui.data_mut(|d| d.remove_temp::<String>(id));
                }
                small(ui, &format!("Note {note}"));
            });
            let spec = picked.or(typed.as_deref()).map(str::trim);
            if picked.is_some() {
                ctx.data_mut(|d| d.remove_temp::<String>(egui::Id::new(("automation-keys", note))));
            }
            match spec {
                Some("") => {
                    config.buttons.remove(&note);
                }
                Some(spec) => {
                    if let Some(action) = parse_action(spec) {
                        config.buttons.insert(note, action);
                    }
                }
                None => {}
            }
        }
        gap(ui);
        hint(
            ui,
            "A button presses the key of its action, with any held Shift, Command or Alt button added. Type a key such as cmd+shift+z, hold:shift or toggle:bw to set one that is not in the list.",
        );

        if config != self.surface.config {
            let socket = config.socket != self.surface.config.socket;
            self.surface.config = config;
            if let Err(e) = self.surface.config.save() {
                self.status = format!("Automation settings not saved: {e:#}");
            }
            if socket {
                self.surface.set_socket(self.surface.config.socket);
            }
        }
        // The device is looked for again once the name is final.
        if !ctx.text_edit_focused() && self.surface.config.port != self.surface.listening_port {
            self.surface.restart_midi();
        }
    }
}
