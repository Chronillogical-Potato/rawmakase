use super::Editor;
use super::dialogs::FileDialog;
use super::icons::Icon;
use super::widgets::{
    ButtonKind, action_button, menu_item, menu_separator, toolbar_action, toolbar_divider,
};
use crate::app::theme;
use crate::develop::Recipe;
use eframe::egui::{self, Stroke, Vec2};

impl Editor {
    pub(super) fn toolbar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        // The buttons follow the shared log, which also holds Library changes.
        self.sync_undo();
        let (can_undo, can_redo) = (self.undo_log.can_undo(), self.undo_log.can_redo());
        egui::Panel::top("toolbar")
            .frame(
                egui::Frame::new()
                    .fill(theme::gray(29))
                    .inner_margin(egui::Margin::symmetric(14, 10)),
            )
            .show(ui, |ui| {
                ui.spacing_mut().button_padding = Vec2::new(12., 8.);
                ui.spacing_mut().interact_size.y = 32.;
                ui.spacing_mut().item_spacing.x = 8.;
                ui.visuals_mut().button_frame = true;
                ui.visuals_mut().widgets.inactive.bg_fill = theme::gray(38);
                ui.visuals_mut().widgets.inactive.weak_bg_fill = theme::gray(38);
                ui.visuals_mut().widgets.inactive.bg_stroke = Stroke::new(1., theme::gray(53));
                ui.visuals_mut().widgets.hovered.bg_fill = theme::gray(52);
                ui.visuals_mut().widgets.hovered.weak_bg_fill = theme::gray(52);
                ui.horizontal(|ui| {
                    if toolbar_action(ui, "", 32., false, can_undo, 1)
                        .on_hover_text("Undo · Ctrl+Z")
                        .clicked()
                    {
                        self.undo();
                    }
                    if toolbar_action(ui, "", 32., false, can_redo, 2)
                        .on_hover_text("Redo · Ctrl+Shift+Z")
                        .clicked()
                    {
                        self.redo();
                    }
                    toolbar_divider(ui);
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
                        if action_button(
                            ui,
                            "Export",
                            Some(Icon::Export),
                            ButtonKind::Primary,
                            self.document.full().is_some(),
                        )
                        .on_hover_text(if cfg!(target_os = "macos") {
                            "Export… · ⇧⌘E"
                        } else {
                            "Export… · Ctrl+Shift+E"
                        })
                        .clicked()
                        {
                            self.open_export_dialog();
                        }
                        ui.add_space(8.);
                        let settings =
                            action_button(ui, "Settings", None, ButtonKind::Secondary, true);
                        egui::Popup::menu(&settings).show(|ui| {
                            ui.set_width(270.);
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
                            let (export, previous) = if cfg!(target_os = "macos") {
                                ("⇧⌘E", "⌥⇧⌘E")
                            } else {
                                ("Ctrl+Shift+E", "Ctrl+Alt+Shift+E")
                            };
                            let photo = self.document.full().is_some();
                            if menu_item(ui, "Export…", export, photo, false) {
                                self.open_export_dialog();
                                ui.close();
                            }
                            if menu_item(ui, "Export with Previous", previous, photo, false) {
                                self.export_with_previous();
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
    /// Pastes the copied settings. Spot removal and masks belong to their photo and
    /// stay as they were, as with Lightroom's default Paste Settings.
    pub(super) fn paste_settings(&mut self) {
        if let Some(mut recipe) = self.clipboard.clone() {
            self.document
                .history
                .label(super::history::Step::new("Paste Settings", ""));
            recipe.retouch = std::mem::take(&mut self.document.recipe.retouch);
            recipe.masks = std::mem::take(&mut self.document.recipe.masks);
            // Auto white balance was estimated for the copied photo; here its values
            // are just Custom.
            recipe.auto_white_balance = None;
            // Upright's corrections were analysed from the copied photo: analyse this one.
            recipe.upright.clear_analysis();
            self.document.recipe = recipe;
            self.ensure_upright();
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
        let groups: [(&str, Vec<(String, &str)>); 4] = [
            (
                "Develop",
                vec![
                    ("R".into(), "Crop & Straighten"),
                    ("Q".into(), "Spot removal (Heal / Clone)"),
                    ("Shift W".into(), "Masking"),
                    ("[ / ]".into(), "Brush size (Shift: feather)"),
                    ("/".into(), "New source for the selected spot"),
                    ("H".into(), "Hide spot pins"),
                    ("A".into(), "Visualize spots"),
                    ("Space".into(), "Pan while a tool is open"),
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
                    (format!("{cmd}{shift}U"), "Auto tone"),
                    (format!("{cmd}{shift}E"), "Export…"),
                    (
                        if cfg!(target_os = "macos") {
                            "⌥⇧⌘E".to_string()
                        } else {
                            "Ctrl+Alt+Shift+E".to_string()
                        },
                        "Export with previous",
                    ),
                    ("Double-click slider".into(), "Reset slider"),
                    (format!("{cmd}'"), "Create virtual copy"),
                ],
            ),
            (
                "Library grid",
                vec![
                    ("Arrow keys".into(), "Move the active photo"),
                    ("Home / End".into(), "First / last photo"),
                    (format!("{shift}arrow / click"), "Extend the selection"),
                    (format!("{cmd}click"), "Add or remove a photo"),
                    (format!("{cmd}A"), "Select all"),
                    (format!("{cmd}D"), "Select none"),
                    ("/".into(), "Deselect the active photo"),
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
                    ("In the grid".into(), "Applies to every selected photo"),
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
                    ui.label(egui::RichText::new(title).size(12.).color(theme::gray(160)));
                    ui.add_space(2.);
                    egui::Grid::new(title)
                        .num_columns(2)
                        .spacing([16., 4.])
                        .show(ui, |ui| {
                            for (key, action) in rows {
                                ui.label(
                                    egui::RichText::new(key).monospace().color(theme::gray(230)),
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
