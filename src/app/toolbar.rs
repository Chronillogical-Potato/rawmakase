use super::Editor;
use super::dialogs::FileDialog;
use super::widgets::{menu_item, menu_separator, toolbar_action, toolbar_divider};
use crate::develop::Recipe;
use eframe::egui::{self, Color32, Stroke, Vec2};

impl Editor {
    pub(super) fn toolbar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Panel::top("toolbar")
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_gray(29))
                    .inner_margin(egui::Margin::symmetric(14, 10)),
            )
            .show(ui, |ui| {
                ui.spacing_mut().button_padding = Vec2::new(12., 8.);
                ui.spacing_mut().interact_size.y = 32.;
                ui.spacing_mut().item_spacing.x = 8.;
                ui.visuals_mut().button_frame = true;
                ui.visuals_mut().widgets.inactive.bg_fill = Color32::from_gray(38);
                ui.visuals_mut().widgets.inactive.weak_bg_fill = Color32::from_gray(38);
                ui.visuals_mut().widgets.inactive.bg_stroke =
                    Stroke::new(1., Color32::from_gray(53));
                ui.visuals_mut().widgets.hovered.bg_fill = Color32::from_gray(52);
                ui.visuals_mut().widgets.hovered.weak_bg_fill = Color32::from_gray(52);
                ui.horizontal(|ui| {
                    ui.menu_button("Open", |ui| {
                        if ui.button("Open RAW file…").clicked() {
                            self.dialog(FileDialog::OpenRaw, &ctx);
                            ui.close();
                        }
                        if ui.button("Open folder…").clicked() {
                            self.dialog(FileDialog::OpenFolder, &ctx);
                            ui.close();
                        }
                    });
                    toolbar_divider(ui);
                    if toolbar_action(ui, "", 32., false, self.document.history.can_undo(), 1)
                        .on_hover_text("Undo · Ctrl+Z")
                        .clicked()
                    {
                        self.undo();
                    }
                    if toolbar_action(ui, "", 32., false, self.document.history.can_redo(), 2)
                        .on_hover_text("Redo · Ctrl+Shift+Z")
                        .clicked()
                    {
                        self.redo();
                    }
                    toolbar_divider(ui);
                    egui::Frame::new()
                        .fill(Color32::from_gray(20))
                        .corner_radius(6.)
                        .inner_margin(3)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 2.;
                                if toolbar_action(ui, "Fit", 44., !self.view.zoom100, true, 0)
                                    .on_hover_text("Fit image · F")
                                    .clicked()
                                {
                                    self.view.zoom100 = false;
                                }
                                if toolbar_action(ui, "100%", 52., self.view.zoom100, true, 0)
                                    .on_hover_text("Actual pixels · 1")
                                    .clicked()
                                {
                                    self.set_zoom(1.);
                                }
                            });
                        });
                    ui.add_space(4.);
                    if toolbar_action(ui, "Before", 70., self.view.compare, true, 3)
                        .on_hover_text("Show original · Backslash")
                        .clicked()
                    {
                        self.view.compare = !self.view.compare;
                    }
                    if toolbar_action(ui, "Clipping", 78., self.view.clipping, true, 0)
                        .on_hover_text("Highlight clipped shadows and highlights")
                        .clicked()
                    {
                        self.view.clipping = !self.view.clipping;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let export = toolbar_action(
                            ui,
                            if self.activity.is_exporting() {
                                "Exporting…"
                            } else {
                                "Export"
                            },
                            94.,
                            true,
                            // Export waits for the full-resolution decode.
                            self.document.full().is_some_and(|im| !im.fast)
                                && !self.activity.is_busy(),
                            4,
                        );
                        egui::Popup::from_toggle_button_response(&export)
                            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                            .show(|ui| {
                                ui.set_min_width(220.);
                                ui.add(
                                    egui::Slider::new(&mut self.document.export.quality, 1..=100)
                                        .text("JPEG quality"),
                                );
                                ui.horizontal(|ui| {
                                    ui.label("Long edge");
                                    ui.add(
                                        egui::DragValue::new(&mut self.document.export.max_edge)
                                            .range(0..=30000)
                                            .suffix(" px"),
                                    );
                                });
                                ui.small("0 = original size. TIFF is 16-bit sRGB.");
                                ui.separator();
                                if ui.button("Export…").clicked() {
                                    self.dialog(FileDialog::Export, &ctx);
                                    ui.close();
                                }
                            });
                        ui.menu_button("Settings", |ui| {
                            ui.set_width(250.);
                            ui.spacing_mut().item_spacing.y = 0.;
                            let (cmd, shift) = if cfg!(target_os = "macos") {
                                ("⌘ ", "Shift ")
                            } else {
                                ("Ctrl+", "Shift+")
                            };
                            let copy = format!("{cmd}{shift}C");
                            let paste = format!("{cmd}{shift}V");
                            if menu_item(ui, "Copy Settings", &copy, true, false) {
                                self.copy_settings();
                                ui.close();
                            }
                            if menu_item(
                                ui,
                                "Paste Settings",
                                &paste,
                                self.clipboard.is_some(),
                                false,
                            ) {
                                self.paste_settings();
                                ui.close();
                            }
                            let reset = format!("{cmd}{shift}R");
                            if menu_item(ui, "Reset All Settings", &reset, true, false) {
                                self.reset_settings();
                                ui.close();
                            }
                            menu_separator(ui);
                            if menu_item(ui, "Save Preset…", "", true, false) {
                                self.dialog(FileDialog::SavePreset, &ctx);
                                ui.close();
                            }
                            if menu_item(ui, "Load Preset…", "", true, false) {
                                self.dialog(FileDialog::LoadPreset, &ctx);
                                ui.close();
                            }
                            menu_separator(ui);
                            let prefs = if cfg!(target_os = "macos") {
                                "⌘ ,"
                            } else {
                                "Ctrl+,"
                            };
                            // Profiles, display and engine choices apply to every photo.
                            if menu_item(ui, "Preferences…", prefs, true, false) {
                                self.open_preferences(super::preferences::Tab::General);
                                ui.close();
                            }
                        });
                        if self.load.is_running() || self.preview.task.is_running() {
                            ui.spinner();
                        }
                    });
                });
            });
    }
    pub(super) fn copy_settings(&mut self) {
        self.clipboard = Some(self.document.recipe.clone());
        self.status = "Settings copied".into();
    }
    /// Back to the camera defaults, like Lightroom's Reset.
    pub(super) fn reset_settings(&mut self) {
        self.document
            .history
            .label(super::history::Step::new("Reset Settings", ""));
        self.document.recipe = self
            .document
            .metadata
            .as_ref()
            .map(|m| Recipe::with_profiles(m, &self.document.profiles))
            .unwrap_or_default();
    }
    pub(super) fn paste_settings(&mut self) {
        if let Some(recipe) = self.clipboard.clone() {
            self.document
                .history
                .label(super::history::Step::new("Paste Settings", ""));
            self.document.recipe = recipe;
            self.status = "Settings pasted".into();
        }
    }
    /// A reference card of every shortcut, grouped like Lightroom's.
    pub(super) fn shortcuts_window(&mut self, ctx: &egui::Context) {
        let (cmd, shift) = if cfg!(target_os = "macos") {
            ("⌘ ", "Shift ")
        } else {
            ("Ctrl+", "Shift+")
        };
        let groups: [(&str, Vec<(String, &str)>); 3] = [
            (
                "Develop",
                vec![
                    ("R".into(), "Crop & Straighten"),
                    ("W".into(), "White balance selector"),
                    ("Enter".into(), "Finish crop"),
                    ("\\".into(), "Before / after"),
                    ("J".into(), "Show clipping"),
                    ("F".into(), "Fit to window"),
                    ("Z".into(), "Toggle 100%"),
                    ("Left / Right arrow".into(), "Previous / next photo"),
                    (format!("{cmd}Z"), "Undo"),
                    (format!("{cmd}{shift}Z"), "Redo"),
                    (format!("{cmd}{shift}C"), "Copy settings"),
                    (format!("{cmd}{shift}V"), "Paste settings"),
                    (format!("{cmd}{shift}R"), "Reset all settings"),
                    ("Double-click slider".into(), "Reset slider"),
                ],
            ),
            (
                "Rating and flags",
                vec![
                    ("0 – 5".into(), "Set star rating"),
                    ("[  ]".into(), "Lower / raise rating"),
                    ("6 – 9".into(), "Red, yellow, green, blue label"),
                    ("P / X / U".into(), "Pick / reject / unflag"),
                    ("`".into(), "Toggle pick"),
                    ("Shift + key".into(), "Apply and go to next photo"),
                ],
            ),
            (
                "Modules",
                vec![("G".into(), "Library"), ("D".into(), "Develop")],
            ),
        ];
        let mut open = self.view.shortcuts;
        egui::Window::new("Keyboard Shortcuts")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(360.)
            .show(ctx, |ui| {
                for (title, rows) in groups {
                    ui.add_space(6.);
                    ui.label(
                        egui::RichText::new(title)
                            .size(12.)
                            .color(Color32::from_gray(160)),
                    );
                    ui.add_space(2.);
                    egui::Grid::new(title)
                        .num_columns(2)
                        .spacing([16., 4.])
                        .show(ui, |ui| {
                            for (key, action) in rows {
                                ui.label(
                                    egui::RichText::new(key)
                                        .monospace()
                                        .color(Color32::from_gray(230)),
                                );
                                ui.label(action);
                                ui.end_row();
                            }
                        });
                }
            });
        self.view.shortcuts = open;
    }
}
