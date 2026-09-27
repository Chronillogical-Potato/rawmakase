//! Lightroom-compatible catalog metadata and keyboard commands.
use crate::app::theme;
use crate::catalog::Photo;
use eframe::egui::{self, Color32, Key};

pub const LABELS: [&str; 5] = ["Red", "Yellow", "Green", "Blue", "Purple"];
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    Rating(i32),
    RatingDelta(i32),
    Flag(i32),
    TogglePick,
    ToggleLabel(String),
    Label(String),
}
impl Edit {
    pub fn values(&self, photo: &Photo) -> (i32, i32, String) {
        let (mut rating, mut flag, mut label) = (photo.rating, photo.flag, photo.label.clone());
        match self {
            Self::Rating(v) => rating = *v,
            Self::RatingDelta(v) => rating = (rating + v).clamp(0, 5),
            Self::Flag(v) => flag = *v,
            Self::TogglePick => flag = if flag == 1 { 0 } else { 1 },
            Self::ToggleLabel(v) => {
                label = if label == *v {
                    String::new()
                } else {
                    v.clone()
                }
            }
            Self::Label(v) => label = v.clone(),
        }
        (rating, flag, label)
    }
}

pub fn shortcut(ctx: &egui::Context) -> Option<(Edit, bool)> {
    if ctx.text_edit_focused() {
        return None;
    }
    ctx.input(|i| {
        i.events.iter().find_map(|event| {
            let egui::Event::Key {
                key,
                physical_key,
                pressed: true,
                repeat: false,
                modifiers,
            } = event
            else {
                return None;
            };
            if modifiers.command || modifiers.ctrl || modifiers.alt || modifiers.mac_cmd {
                return None;
            }
            let map = |key| {
                Some(match key {
                    Key::Num0 => Edit::Rating(0),
                    Key::Num1 => Edit::Rating(1),
                    Key::Num2 => Edit::Rating(2),
                    Key::Num3 => Edit::Rating(3),
                    Key::Num4 => Edit::Rating(4),
                    Key::Num5 => Edit::Rating(5),
                    Key::Num6 => Edit::ToggleLabel("Red".into()),
                    Key::Num7 => Edit::ToggleLabel("Yellow".into()),
                    Key::Num8 => Edit::ToggleLabel("Green".into()),
                    Key::Num9 => Edit::ToggleLabel("Blue".into()),
                    Key::P => Edit::Flag(1),
                    Key::U => Edit::Flag(0),
                    Key::X => Edit::Flag(-1),
                    Key::Backtick => Edit::TogglePick,
                    Key::OpenBracket => Edit::RatingDelta(-1),
                    Key::CloseBracket => Edit::RatingDelta(1),
                    _ => return None,
                })
            };
            // Shift-number may arrive as punctuation; retain the physical number key.
            map(*key)
                .or_else(|| physical_key.and_then(map))
                .map(|edit| (edit, modifiers.shift))
        })
    })
}

pub fn label_color(label: &str) -> Option<Color32> {
    Some(match label {
        "" => return None,
        "Red" => Color32::from_rgb(206, 86, 83),
        "Yellow" => Color32::from_rgb(220, 188, 77),
        "Green" => Color32::from_rgb(91, 171, 112),
        "Blue" => Color32::from_rgb(87, 143, 207),
        "Purple" => Color32::from_rgb(164, 111, 194),
        // Lightroom stores label text; custom label-set colors cannot be inferred.
        _ => theme::gray(220),
    })
}

/// Rating, flag and label as fixed-size painted controls, so hovering never
/// changes the layout.
pub fn controls(ui: &mut egui::Ui, photo: &Photo, labels: &[String]) -> Option<Edit> {
    use egui::{Align2, FontId, Rect, Sense, Stroke, StrokeKind, Vec2};
    let mut edit = None;
    ui.push_id(("photo-metadata", photo.id), |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.;
            let stars: Vec<_> = (1..=5)
                .map(|star| {
                    let (rect, response) =
                        ui.allocate_exact_size(Vec2::new(17., 22.), Sense::click());
                    let response = response
                        .on_hover_text(format!("{star} stars · {star} key · click again to clear"));
                    if response.clicked() {
                        edit = Some(Edit::Rating(if photo.rating == star { 0 } else { star }));
                    }
                    (rect, response.hovered())
                })
                .collect();
            // Hovering previews the rating up to the pointer.
            let hovered = stars.iter().position(|(_, h)| *h).map(|i| i as i32 + 1);
            for (i, (rect, _)) in stars.iter().enumerate() {
                let lit = (i as i32) < hovered.unwrap_or(photo.rating);
                ui.painter().text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    "★",
                    FontId::proportional(14.),
                    theme::gray(match (lit, hovered.is_some()) {
                        (true, false) => 230,
                        (true, true) => 175,
                        _ => 72,
                    }),
                );
            }
            ui.add_space(12.);
            for (flag, tip) in [(1, "Pick · P"), (-1, "Reject · X")] {
                let (rect, response) = ui.allocate_exact_size(Vec2::new(24., 22.), Sense::click());
                let active = photo.flag == flag;
                if active || response.hovered() {
                    ui.painter().rect_filled(
                        rect.shrink2(Vec2::new(2., 2.)),
                        3.,
                        theme::gray(if active { 70 } else { 50 }),
                    );
                }
                flag_icon(ui.painter(), rect.center(), flag, active);
                if response.on_hover_text(tip).clicked() {
                    edit = Some(Edit::Flag(if active { 0 } else { flag }));
                }
            }
            ui.add_space(12.);
            for label in LABELS {
                let (rect, response) = ui.allocate_exact_size(Vec2::new(18., 22.), Sense::click());
                let active = photo.label == label;
                let chip = Rect::from_center_size(rect.center(), Vec2::splat(11.));
                ui.painter()
                    .rect_filled(chip, 2., label_color(label).unwrap_or_default());
                if active || response.hovered() {
                    ui.painter().rect_stroke(
                        chip.expand(2.),
                        3.,
                        Stroke::new(1.2, theme::gray(if active { 235 } else { 140 })),
                        StrokeKind::Outside,
                    );
                }
                if response.on_hover_text(label).clicked() {
                    edit = Some(Edit::Label(if active {
                        String::new()
                    } else {
                        label.into()
                    }));
                }
            }
            let custom: Vec<_> = labels
                .iter()
                .filter(|l| !LABELS.contains(&l.as_str()))
                .collect();
            if !custom.is_empty() || !LABELS.contains(&photo.label.as_str()) {
                ui.add_space(4.);
                ui.menu_button(egui::RichText::new("⋯").size(12.), |ui| {
                    for label in custom {
                        if ui.selectable_label(photo.label == *label, label).clicked() {
                            edit = Some(Edit::Label(label.clone()));
                            ui.close();
                        }
                    }
                    if ui.button("No label").clicked() {
                        edit = Some(Edit::Label(String::new()));
                        ui.close();
                    }
                })
                .response
                .on_hover_text(if photo.label.is_empty() {
                    "Other labels".to_string()
                } else {
                    format!("Label: {}", photo.label)
                });
            }
        });
    });
    edit
}
/// Lightroom's pick (white) and reject (black with a red cross) flags.
pub fn flag_icon(painter: &egui::Painter, at: egui::Pos2, flag: i32, strong: bool) {
    use egui::{Stroke, Vec2};
    let color = if flag > 0 {
        theme::gray(if strong { 245 } else { 200 })
    } else {
        theme::gray(if strong { 15 } else { 30 })
    };
    let outline = Stroke::new(1., theme::gray(if flag > 0 { 90 } else { 150 }));
    painter.line_segment(
        [at + Vec2::new(-3., -5.), at + Vec2::new(-3., 6.)],
        Stroke::new(1.3, if flag > 0 { color } else { outline.color }),
    );
    painter.add(egui::Shape::convex_polygon(
        vec![
            at + Vec2::new(-3., -5.),
            at + Vec2::new(5., -2.5),
            at + Vec2::new(-3., 0.5),
        ],
        color,
        outline,
    ));
    if flag < 0 {
        let red = Stroke::new(1.3, Color32::from_rgb(214, 78, 66));
        let c = at + Vec2::new(4., 4.);
        painter.line_segment([c + Vec2::new(-2., -2.), c + Vec2::new(2., 2.)], red);
        painter.line_segment([c + Vec2::new(-2., 2.), c + Vec2::new(2., -2.)], red);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shortcuts_respect_modifiers_repeat_and_text_focus() {
        let ctx = egui::Context::default();
        let run = |event: egui::Event, expected| {
            let repeated = matches!(event, egui::Event::Key { repeat: true, .. });
            let mut release = event.clone();
            if let egui::Event::Key {
                pressed, repeat, ..
            } = &mut release
            {
                *pressed = false;
                *repeat = false;
            }
            let mut actual = None;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events: if repeated {
                        vec![event]
                    } else {
                        vec![release, event]
                    },
                    ..Default::default()
                },
                |ui| {
                    actual = shortcut(ui.ctx());
                },
            );
            output.textures_delta.clear();
            assert_eq!(actual, expected);
        };
        let key = |key, modifiers, repeat| egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat,
            modifiers,
        };
        for (key_value, edit) in [
            (Key::Num0, Edit::Rating(0)),
            (Key::Num5, Edit::Rating(5)),
            (Key::Num6, Edit::ToggleLabel("Red".into())),
            (Key::Num9, Edit::ToggleLabel("Blue".into())),
            (Key::P, Edit::Flag(1)),
            (Key::X, Edit::Flag(-1)),
            (Key::U, Edit::Flag(0)),
            (Key::OpenBracket, Edit::RatingDelta(-1)),
            (Key::CloseBracket, Edit::RatingDelta(1)),
            (Key::Backtick, Edit::TogglePick),
        ] {
            run(
                key(key_value, egui::Modifiers::NONE, false),
                Some((edit, false)),
            );
        }
        run(
            key(Key::Num5, egui::Modifiers::SHIFT, false),
            Some((Edit::Rating(5), true)),
        );
        for modifiers in [
            egui::Modifiers::COMMAND,
            egui::Modifiers::CTRL,
            egui::Modifiers::ALT,
        ] {
            run(key(Key::Num5, modifiers, false), None);
        }
        run(key(Key::Num6, egui::Modifiers::NONE, true), None);
        let mut text = String::new();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.text_edit_singleline(&mut text).request_focus();
        });
        output.textures_delta.clear();
        assert!(ctx.text_edit_focused());
        run(key(Key::Num5, egui::Modifiers::NONE, false), None);
    }
    #[test]
    fn unknown_label_is_visible_without_guessing_a_color() {
        assert_eq!(label_color("Client approved"), Some(theme::gray(220)));
        assert_eq!(label_color(""), None);
        assert_ne!(label_color("Red"), label_color("Green"));
    }
}
