//! The naming templates a dialog offers (Export's Rename To, Rename
//! Photos' File Naming): Lightroom's presets, RAWmakase's own templates, the
//! template made in the editor, and Edit… to make one.
use crate::export_settings::{BUILTIN, Naming, builtin};
use eframe::egui;

/// What was picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Choice {
    Preset(Naming),
    /// A template in Lightroom's form.
    Tokens(&'static str),
    Edit,
}

/// What the menu shows for `naming`, or for `custom` (Lightroom's form)
/// when a template names files in its place.
pub(in crate::app) fn shown(naming: Naming, custom: Option<&str>) -> &'static str {
    match custom {
        Some(tokens) => builtin(tokens).map_or("Custom Settings", |b| b.name),
        None => naming.label(),
    }
}

/// The menu, `width` wide: what was picked, if anything.
pub(in crate::app) fn combo(
    ui: &mut egui::Ui,
    id_salt: &str,
    width: f32,
    naming: Naming,
    custom: Option<&str>,
) -> Option<Choice> {
    let mut picked = None;
    egui::ComboBox::from_id_salt(id_salt)
        .width(width)
        .selected_text(shown(naming, custom))
        .show_ui(ui, |ui| {
            for n in Naming::ALL {
                if ui
                    .selectable_label(custom.is_none() && naming == n, n.label())
                    .clicked()
                {
                    picked = Some(Choice::Preset(n));
                }
            }
            ui.separator();
            for b in &BUILTIN {
                if ui
                    .selectable_label(custom == Some(b.tokens), b.name)
                    .clicked()
                {
                    picked = Some(Choice::Tokens(b.tokens));
                }
            }
            if custom.is_some_and(|t| builtin(t).is_none()) {
                let _ = ui.selectable_label(true, "Custom Settings");
            }
            ui.separator();
            if ui.selectable_label(false, "Edit…").clicked() {
                picked = Some(Choice::Edit);
            }
        });
    picked
}
