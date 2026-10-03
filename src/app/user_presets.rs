//! Presets made here: New Develop Preset, and Update, Rename and Delete in a preset's
//! menu, for presets in the user library's "User Presets" folder only.
use super::widgets::{modal_frame, primary_button};
use super::{Editor, settings_transfer::PresetForm, theme};
use crate::develop::settings_groups::GroupSelection;
use crate::presets::user::UserPresets;
use crate::xmp::preset_write::PresetInfo;
use eframe::egui::{self, Color32, Vec2};

/// A change asked for in a preset's menu.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum PresetAction {
    Update(usize),
    StartRename(usize),
    Delete(usize),
}

/// A preset being renamed: its file, which stays put while the library reloads,
/// and the name being typed.
pub(super) struct PresetRename {
    path: std::path::PathBuf,
    name: String,
}

/// What the rename window asked for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RenameChoice {
    Rename,
    Cancel,
}

impl Editor {
    /// Saves the open photo's `groups` as a new preset in the user library.
    pub(super) fn create_preset(&mut self, form: &PresetForm, groups: &GroupSelection) {
        self.create_preset_in(&UserPresets::default(), form, groups);
    }
    /// A photo is open and loaded, so its settings are the ones shown, not the
    /// defaults a document holds while the next photo loads.
    fn settings_ready(&self) -> bool {
        self.document.full().is_some() && !self.load.is_running()
    }
    fn create_preset_in(&mut self, user: &UserPresets, form: &PresetForm, groups: &GroupSelection) {
        if !self.settings_ready() {
            self.status = "Preset not created: no photo is open".into();
            return;
        }
        let info = PresetInfo::new(&form.name, &form.group);
        match user.create(&self.document.recipe, &info, groups) {
            Ok(_) => {
                self.status = format!("Preset {} created in {}", info.name, info.group);
                self.reload_presets(&self.context.clone());
            }
            Err(e) => self.status = format!("Preset not created: {e:#}"),
        }
    }
    /// Carries out a preset menu's choice.
    pub(super) fn preset_action(&mut self, action: PresetAction) {
        let library = self.presets.library.clone();
        let user = UserPresets::default();
        let result = match action {
            PresetAction::Update(_) if !self.settings_ready() => {
                self.status = "Preset not updated: no photo is open".into();
                return;
            }
            PresetAction::Update(i) => library.presets.get(i).map(|p| {
                user.update(p, &self.document.recipe)
                    .map(|()| format!("Preset {} updated", p.name))
            }),
            PresetAction::Delete(i) => library.presets.get(i).map(|p| {
                user.delete(p).map(|()| {
                    // A new preset later saved at the same place starts unfavored.
                    if self.presets.favorites.remove(&p.id) {
                        self.presets.revision += 1;
                        let _ = crate::presets::save_favorites(&self.presets.favorites);
                    }
                    format!("Preset {} deleted", p.name)
                })
            }),
            PresetAction::StartRename(i) => {
                if let Some(p) = library.presets.get(i) {
                    self.preset_rename = Some(PresetRename {
                        path: p.path.clone(),
                        name: p.name.clone(),
                    });
                }
                return;
            }
        };
        match result {
            Some(Ok(status)) => {
                self.status = status;
                self.reload_presets(&self.context.clone());
            }
            Some(Err(e)) => self.status = format!("Preset not changed: {e:#}"),
            None => {}
        }
    }
    /// The Rename Preset window, while one is being renamed.
    pub(super) fn preset_rename_window(&mut self, ctx: &egui::Context) {
        let Some(rename) = &mut self.preset_rename else {
            return;
        };
        let mut choice = None;
        let response = egui::Modal::new(egui::Id::new("rename-preset"))
            .backdrop_color(Color32::from_black_alpha(140))
            .frame(modal_frame().inner_margin(egui::Margin::symmetric(28, 22)))
            .show(ctx, |ui| {
                ui.set_width(380.);
                ui.label(
                    egui::RichText::new("Rename Preset")
                        .size(17.)
                        .color(theme::gray(235)),
                );
                ui.add_space(12.);
                let field = ui
                    .add(egui::TextEdit::singleline(&mut rename.name).desired_width(f32::INFINITY));
                if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    choice = Some(RenameChoice::Rename);
                }
                ui.add_space(16.);
                ui.horizontal(|ui| {
                    ui.spacing_mut().button_padding = Vec2::new(14., 6.);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if primary_button(ui, "Rename").clicked() {
                            choice = Some(RenameChoice::Rename);
                        }
                        if ui.button("Cancel").clicked() {
                            choice = Some(RenameChoice::Cancel);
                        }
                    });
                });
            });
        if response.should_close() {
            choice = Some(RenameChoice::Cancel);
        }
        let Some(choice) = choice else {
            return;
        };
        let rename = self.preset_rename.take().expect("open above");
        if choice == RenameChoice::Cancel {
            return;
        }
        let library = self.presets.library.clone();
        let Some(preset) = library.presets.iter().find(|p| p.path == rename.path) else {
            return;
        };
        match UserPresets::default().rename(preset, &rename.name) {
            Ok(path) => {
                // A favorite follows the preset to its new file.
                let old = preset.id.clone();
                if self.presets.favorites.remove(&old) {
                    self.presets
                        .favorites
                        .insert(path.to_string_lossy().to_string());
                    self.presets.revision += 1;
                    let _ = crate::presets::save_favorites(&self.presets.favorites);
                }
                self.status = format!("Preset renamed to {}", rename.name.trim());
                self.reload_presets(ctx);
            }
            Err(e) => self.status = format!("Preset not renamed: {e:#}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop::settings_groups::{GroupInclusion, SettingGroup};

    #[test]
    fn new_develop_preset_writes_the_chosen_settings_to_the_user_library() {
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
        let d = tempfile::tempdir().unwrap();
        let user = UserPresets {
            dir: d.path().join("User Presets"),
        };
        // Without a loaded photo there is nothing to save.
        let form = PresetForm {
            name: "Bright".into(),
            group: "Mine".into(),
        };
        e.create_preset_in(&user, &form, &GroupSelection::all());
        assert!(e.status.contains("no photo"), "{}", e.status);
        e.document
            .set_image(std::sync::Arc::new(crate::raw::CameraImage {
                recovered: Default::default(),
                width: 4,
                height: 4,
                pixels: vec![[0.1; 3]; 16],
                metadata: Default::default(),
                fast: false,
                scale_factor: 1.,
                scale_clipped: 0,
            }));
        e.document.recipe.exposure = 0.7;
        let mut groups = GroupSelection::none();
        groups.set(SettingGroup::Exposure, GroupInclusion::Included);
        e.create_preset_in(&user, &form, &groups);
        let path = user.dir.join("Mine/Bright.xmp");
        let preset = crate::xmp::parse(&path, &std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(preset.settings["Exposure2012"], "+0.70");
        assert!(e.status.contains("created"), "{}", e.status);
        // A second one with the same name says why it wasn't made.
        e.create_preset_in(&user, &form, &groups);
        assert!(e.status.starts_with("Preset not created"), "{}", e.status);
    }
}
