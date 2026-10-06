use super::Editor;
use super::before_after::{Compare, Transfer};
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
                    let before_only = self.view.compare.before_only();
                    if toolbar_action(ui, "Before", 70., before_only, true, 3)
                        .on_hover_text("Show Before · Backslash")
                        .clicked()
                    {
                        let view = self.view.compare.toggled(Compare::BeforeOnly);
                        self.set_compare(view);
                    }
                    self.before_after_menu(ui);
                    self.reference_controls(ui);
                    if toolbar_action(ui, "Clipping", 78., self.view.clipping.both_on(), true, 0)
                        .on_hover_text("Show clipped shadows and highlights · J")
                        .clicked()
                    {
                        self.view.clipping.toggle_both();
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
                            if menu_item(ui, "Copy Settings…", &copy, true, false) {
                                self.open_copy_dialog(super::settings_transfer::Transfer::Copy);
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
                            let previous = format!(
                                "{cmd}{}V",
                                if cfg!(target_os = "macos") {
                                    "⌥ "
                                } else {
                                    "Alt+"
                                }
                            );
                            if menu_item(
                                ui,
                                "Paste Settings from Previous",
                                &previous,
                                self.previous_settings.is_some(),
                                false,
                            ) {
                                self.paste_previous();
                                ui.close();
                            }
                            let sync = format!("{cmd}{shift}S");
                            let targets = self.sync_targets().len();
                            if menu_item(
                                ui,
                                "Sync Settings…",
                                &sync,
                                targets > 0 && !self.activity.is_busy(),
                                false,
                            ) {
                                self.open_copy_dialog(super::settings_transfer::Transfer::Sync);
                                ui.close();
                            }
                            // Lightroom's Match Total Exposures, for the photos selected
                            // with this one, from their aperture, shutter speed and ISO.
                            let matchable = targets > 0
                                && !self.activity.is_busy()
                                && self
                                    .document
                                    .metadata
                                    .as_ref()
                                    .and_then(super::sync::capture_stops)
                                    .is_some();
                            let match_keys = format!(
                                "{cmd}{shift}{}M",
                                if cfg!(target_os = "macos") {
                                    "⌥ "
                                } else {
                                    "Alt+"
                                }
                            );
                            if menu_item(ui, "Match Total Exposures", &match_keys, matchable, false)
                            {
                                self.start_sync(super::sync::BatchChange::MatchTotalExposures);
                                ui.close();
                            }
                            let reset = format!("{cmd}{shift}R");
                            if menu_item(ui, "Reset All Settings", &reset, true, false) {
                                self.reset_settings();
                                ui.close();
                            }
                            menu_separator(ui);
                            let new_preset = format!("{cmd}{shift}N");
                            if menu_item(ui, "New Develop Preset…", &new_preset, true, false) {
                                self.open_copy_dialog(
                                    super::settings_transfer::Transfer::NewPreset,
                                );
                                ui.close();
                            }
                            if menu_item(ui, "Save Preset File…", "", true, false) {
                                self.dialog(FileDialog::SavePreset, &ctx);
                                ui.close();
                            }
                            if menu_item(ui, "Load Preset File…", "", true, false) {
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
    /// Reference View's button, and while it shows, its layout and lock, as in
    /// Lightroom's toolbar.
    fn reference_controls(&mut self, ui: &mut egui::Ui) {
        use super::before_after::Axis;
        let shown = self.reference_view();
        if toolbar_action(ui, "Reference", 84., shown, true, 0)
            .on_hover_text("Reference View · Shift+R")
            .clicked()
        {
            self.toggle_reference_view();
        }
        if !shown {
            return;
        }
        for (layout, label, width) in [
            (Axis::LeftRight, "Left / Right", 92.),
            (Axis::TopBottom, "Top / Bottom", 98.),
        ] {
            let chosen = self.view.compare == Compare::Reference(layout);
            if toolbar_action(ui, label, width, chosen, true, 0).clicked() {
                self.set_reference_layout(layout);
            }
        }
        let locked = self.reference.locked;
        if toolbar_action(ui, "Lock", 54., locked, self.reference.photo.is_some(), 0)
            .on_hover_text(if locked {
                "The reference photo stays when you leave Develop"
            } else {
                "Keep the reference photo when you leave Develop"
            })
            .clicked()
        {
            self.reference.locked = !locked;
        }
    }
    /// Lightroom's Before/After menu: the views, with their keys, and copying or
    /// swapping settings between the two sides.
    fn before_after_menu(&mut self, ui: &mut egui::Ui) {
        let two_up = self.view.compare.two_up() && self.view.compare.shows_before();
        let button = toolbar_action(ui, "Before / After", 104., two_up, true, 0)
            .on_hover_text("Before/After views · Y, Option+Y, Shift+Y");
        let mac = cfg!(target_os = "macos");
        let alt = if mac { "⌥ " } else { "Alt+" };
        let transfer = if mac { "⌥⇧⌘ " } else { "Ctrl+Alt+Shift+" };
        egui::Popup::menu(&button).show(|ui| {
            ui.set_width(300.);
            ui.spacing_mut().item_spacing.y = 0.;
            for (view, name) in super::before_after::VIEWS {
                let keys = match view {
                    Compare::BeforeOnly => "\\".to_string(),
                    Compare::SideBySide(super::before_after::Axis::LeftRight) => "Y".into(),
                    Compare::SideBySide(_) => format!("{alt}Y"),
                    Compare::Split(super::before_after::Axis::LeftRight) => "Shift+Y".into(),
                    _ => String::new(),
                };
                if menu_item(ui, name, &keys, true, self.view.compare == view) {
                    let view = self.view.compare.toggled(view);
                    self.set_compare(view);
                    ui.close();
                }
            }
            menu_separator(ui);
            for (transfer_kind, name, key) in [
                (
                    Transfer::BeforeToAfter,
                    "Copy Before's Settings to After",
                    "→",
                ),
                (
                    Transfer::AfterToBefore,
                    "Copy After's Settings to Before",
                    "←",
                ),
                (Transfer::Swap, "Swap Before and After Settings", "↑"),
            ] {
                let keys = format!("{transfer}{key}");
                if menu_item(ui, name, &keys, self.document.full().is_some(), false) {
                    self.transfer(transfer_kind);
                    ui.close();
                }
            }
        });
    }
    /// Back to the raw defaults, like Lightroom's Reset.
    pub(super) fn reset_settings(&mut self) {
        self.document
            .edit
            .history
            .label(super::history::Step::new("Reset Settings", ""));
        let Some(defaults) = self.photo_defaults() else {
            self.document.edit.recipe = Recipe::default();
            return;
        };
        if let Some(note) = defaults.note {
            self.status = note;
        }
        self.document.edit.recipe = defaults.recipe;
    }
}
