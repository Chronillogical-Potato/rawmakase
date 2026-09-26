use super::Editor;
use super::dialogs::FileDialog;
use super::dialogs::{CatalogDialog, FolderAction};
use super::widgets::workspace_tab;
use eframe::egui::{self, Color32, Vec2};
use std::time::Duration;

impl Editor {
    pub(super) fn metadata_shortcuts(&mut self, ctx: &egui::Context) {
        if self.activity.is_busy() {
            return;
        }
        let id = if self.library_mode {
            self.library.as_ref().and_then(|l| l.selected)
        } else {
            self.document.catalog_photo
        };
        let Some(id) = id else {
            return;
        };
        if let Some((edit, advance)) = crate::app::photo_metadata::shortcut(ctx) {
            let Some(library) = &mut self.library else {
                return;
            };
            match library.edit_metadata(id, edit, advance) {
                Ok(next) => {
                    self.status = library.message.clone();
                    if !self.library_mode
                        && let Some(next) = next
                    {
                        self.develop_catalog_photo(next);
                        if self.document.catalog_photo != Some(next)
                            && let Some(library) = &mut self.library
                        {
                            library.selected = Some(id);
                        }
                    }
                }
                Err(e) => self.status = format!("Metadata could not be saved: {e}"),
            }
        } else if self.library_mode && !ctx.egui_wants_keyboard_input() {
            let delta = ctx.input(|i| {
                if i.modifiers.any() {
                    0
                } else if i.key_pressed(egui::Key::ArrowRight) {
                    1
                } else if i.key_pressed(egui::Key::ArrowLeft) {
                    -1
                } else {
                    0
                }
            });
            if delta != 0
                && let Some(library) = &mut self.library
            {
                library.selected = library.navigate(id, delta);
            }
        }
    }
    pub(super) fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.events(&ctx);
        if let Some(library) = &mut self.library {
            library.poll_previews(&ctx);
        }
        self.metadata_shortcuts(&ctx);
        self.workspace_shortcuts(&ctx);
        self.workspace_bar(ui);
        if self.activity.is_dialog() {
            ui.disable();
        }
        if self.onboarding.visible {
            self.onboarding_ui(ui);
        } else if self.library_mode {
            self.library_workspace(ui);
        } else {
            let frame = self.begin_edit_frame();
            self.develop_shortcuts(&ctx);
            self.toolbar(ui);
            self.status_bar(ui);
            self.filmstrip(ui);
            self.develop_panels(ui);
            self.finish_edit_frame(frame, &ctx);
        }
        self.shortcuts_window(&ctx);
        self.pending_work(&ctx);
        let collapsed = ctx.data(|d| {
            d.get_temp::<std::collections::BTreeSet<String>>(
                super::widgets::collapsed_sections_id(),
            )
        });
        if let Some(collapsed) = collapsed
            && collapsed != self.collapsed
        {
            self.collapsed = collapsed;
            let _ = self.save_session();
        }
    }
    fn workspace_shortcuts(&mut self, ctx: &egui::Context) {
        if !self.activity.is_busy() && !ctx.egui_wants_keyboard_input() {
            if ctx.input(|i| i.key_pressed(egui::Key::G)) && self.flush() {
                self.library_mode = true;
            }
            if ctx.input(|i| i.key_pressed(egui::Key::D)) {
                if self.library_mode {
                    if let Some(id) = self.library.as_ref().and_then(|l| l.selected) {
                        self.develop_catalog_photo(id);
                    }
                } else {
                    self.library_mode = false;
                }
            }
        }
    }

    /// Lightroom's top panel: identity plate on the left, module picker on the right.
    fn workspace_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Panel::top("workspace-modes")
            .exact_size(44.)
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_gray(26))
                    .inner_margin(egui::Margin::symmetric(18, 0)),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.;
                    let catalog = self
                        .library
                        .as_ref()
                        .map(|library| {
                            library
                                .catalog
                                .path
                                .file_stem()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string()
                        })
                        .unwrap_or_else(|| "No catalog".into());
                    // Every element is painted in a 28 px slot so all centers line up.
                    let wordmark = ui.painter().layout_no_wrap(
                        "rawmakase".into(),
                        egui::FontId::proportional(17.),
                        Color32::from_gray(232),
                    );
                    let (rect, _) = ui.allocate_exact_size(
                        Vec2::new(wordmark.size().x, 28.),
                        egui::Sense::hover(),
                    );
                    ui.painter().galley(
                        rect.left_center() - Vec2::new(0., wordmark.size().y / 2.),
                        wordmark,
                        Color32::from_gray(232),
                    );
                    let (rect, _) =
                        ui.allocate_exact_size(Vec2::new(29., 28.), egui::Sense::hover());
                    ui.painter().line_segment(
                        [
                            rect.center() - Vec2::new(0., 9.),
                            rect.center() + Vec2::new(0., 9.),
                        ],
                        egui::Stroke::new(1., Color32::from_gray(70)),
                    );
                    let name = ui.painter().layout_no_wrap(
                        catalog,
                        egui::FontId::proportional(13.),
                        Color32::WHITE,
                    );
                    let busy = self.activity.is_busy();
                    let (rect, response) = ui.allocate_exact_size(
                        Vec2::new(name.size().x.min(320.) + 36., 28.),
                        if busy {
                            egui::Sense::hover()
                        } else {
                            egui::Sense::click()
                        },
                    );
                    let open =
                        egui::Popup::is_id_open(&ctx, egui::Popup::default_response_id(&response));
                    if response.hovered() || open {
                        ui.painter().rect_filled(rect, 4., Color32::from_gray(38));
                    }
                    let color =
                        Color32::from_gray(if response.hovered() || open { 235 } else { 175 });
                    ui.painter()
                        .with_clip_rect(rect.shrink2(Vec2::new(10., 0.)))
                        .galley(
                            rect.left_center() + Vec2::new(10., -name.size().y / 2.),
                            name,
                            color,
                        );
                    let c = rect.right_center() - Vec2::new(14., 0.);
                    ui.painter().add(egui::Shape::line(
                        vec![
                            c + Vec2::new(-3.5, -1.5),
                            c + Vec2::new(0., 2.),
                            c + Vec2::new(3.5, -1.5),
                        ],
                        egui::Stroke::new(1.3, color),
                    ));
                    let response = response
                        .on_hover_text("Catalog: open, create or import")
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    egui::Popup::menu(&response).show(|ui| {
                        ui.set_min_width(210.);
                        if ui
                            .add(egui::Button::new("Setup assistant…").frame(false))
                            .clicked()
                        {
                            self.open_onboarding();
                            ui.close();
                        }
                        ui.separator();
                        for (kind, label) in [
                            (CatalogDialog::Open, "Open catalog…"),
                            (CatalogDialog::Create, "New catalog…"),
                            (CatalogDialog::ImportLightroom, "Import Lightroom catalog…"),
                            (
                                CatalogDialog::Folder(FolderAction::Add),
                                "Add photo folder…",
                            ),
                        ] {
                            if ui
                                .add_enabled(
                                    !matches!(kind, CatalogDialog::Folder(_))
                                        || self.library.is_some(),
                                    egui::Button::new(label).frame(false),
                                )
                                .clicked()
                            {
                                self.catalog_dialog(kind, &ctx);
                                ui.close();
                            }
                        }
                    });
                    ui.add_space(8.);
                    if self.activity.is_dialog() {
                        ui.spinner();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 0.;
                        ui.add_enabled_ui(!self.activity.is_busy(), |ui| {
                            let setup = self.onboarding.visible;
                            if workspace_tab(ui, "Develop", !setup && !self.library_mode)
                                .on_hover_text("Develop · D")
                                .clicked()
                            {
                                self.onboarding.visible = false;
                                if self.library_mode
                                    && let Some(id) =
                                        self.library.as_ref().and_then(|library| library.selected)
                                {
                                    self.develop_catalog_photo(id);
                                } else {
                                    self.library_mode = false;
                                }
                            }
                            let (rect, _) =
                                ui.allocate_exact_size(Vec2::new(1., 28.), egui::Sense::hover());
                            ui.painter().line_segment(
                                [
                                    rect.center() - Vec2::new(0., 8.),
                                    rect.center() + Vec2::new(0., 8.),
                                ],
                                egui::Stroke::new(1., Color32::from_gray(70)),
                            );
                            if workspace_tab(ui, "Library", !setup && self.library_mode)
                                .on_hover_text("Library · G")
                                .clicked()
                                && self.flush()
                            {
                                self.onboarding.visible = false;
                                self.library_mode = true;
                            }
                        });
                    });
                });
            });
        egui::Panel::top("workspace-modes-rule")
            .exact_size(1.)
            .frame(egui::Frame::new().fill(Color32::from_gray(16)))
            .show(ui, |_| {});
    }

    fn library_workspace(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Panel::bottom("library-status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.small(
                    self.library
                        .as_ref()
                        .filter(|l| !l.message.is_empty())
                        .map_or(self.status.as_str(), |l| l.message.as_str()),
                );
                self.preview_progress(ui);
            });
        });
        let mut action = crate::app::library::Action::None;
        egui::Panel::left("library-sidebar")
            .default_size(260.)
            .min_size(180.)
            .max_size(500.)
            .show(ui, |ui| {
                if let Some(library) = &mut self.library {
                    action = library.sidebar(ui);
                } else {
                    ui.heading("Library");
                    ui.label("Create an RAWmakase catalog or import a Lightroom catalog from the Catalog menu.");
                }
            });
        egui::Panel::right("library-info")
            .default_size(270.)
            .min_size(220.)
            .max_size(420.)
            .show(ui, |ui| {
                if let Some(library) = &mut self.library {
                    let a = library.info_panel(ui);
                    if !matches!(a, crate::app::library::Action::None) {
                        action = a
                    }
                }
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::new())
            .show(ui, |ui| {
                if let Some(l) = &mut self.library {
                    let a = l.grid(ui);
                    if !matches!(a, crate::app::library::Action::None) {
                        action = a
                    }
                } else {
                    ui.centered_and_justified(|ui| {
                        ui.label("Your photographs, folders and collections");
                    });
                }
            });
        if !self.activity.is_busy() {
            match action {
                crate::app::library::Action::Develop(id) => self.develop_catalog_photo(id),
                crate::app::library::Action::RelinkRoot(id) => {
                    self.catalog_dialog(CatalogDialog::Folder(FolderAction::RelinkRoot(id)), &ctx)
                }
                crate::app::library::Action::RelinkFolder(id) => {
                    self.catalog_dialog(CatalogDialog::Folder(FolderAction::RelinkFolder(id)), &ctx)
                }
                crate::app::library::Action::AddFolder => {
                    self.catalog_dialog(CatalogDialog::Folder(FolderAction::Add), &ctx)
                }
                crate::app::library::Action::None => {}
            }
        }
    }

    fn develop_shortcuts(&mut self, ctx: &egui::Context) {
        if !self.activity.is_busy() && !ctx.egui_wants_keyboard_input() {
            if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::O)) {
                self.dialog(FileDialog::OpenRaw, ctx);
            }
            let (mut copy, mut paste, mut zoom_step) = (false, false, 0);
            ctx.input(|i| {
                if i.key_pressed(egui::Key::ArrowRight) {
                    self.navigate(1);
                }
                if i.key_pressed(egui::Key::ArrowLeft) {
                    self.navigate(-1);
                }
                if i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::C) {
                    copy = true;
                }
                if i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::V) {
                    paste = true;
                }
                if i.modifiers.command && i.key_pressed(egui::Key::Z) {
                    if i.modifiers.shift {
                        self.redo();
                    } else {
                        self.undo();
                    }
                }
                if i.modifiers.command
                    && (i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals))
                {
                    zoom_step = 1;
                }
                if i.modifiers.command && i.key_pressed(egui::Key::Minus) {
                    zoom_step = -1;
                }
                if i.key_pressed(egui::Key::Z) && !i.modifiers.any() {
                    self.view.zoom100 = !self.view.zoom100;
                }
                if i.key_pressed(egui::Key::F) {
                    self.view.zoom100 = false;
                }
                if (i.key_pressed(egui::Key::C) || i.key_pressed(egui::Key::R))
                    && !i.modifiers.command
                {
                    self.view.crop_mode = !self.view.crop_mode;
                    self.view.zoom100 = false;
                }
                if i.key_pressed(egui::Key::J) {
                    self.view.clipping = !self.view.clipping;
                }
                if i.key_pressed(egui::Key::Backslash) {
                    self.view.compare = !self.view.compare;
                }
                if i.key_pressed(egui::Key::Enter) && self.view.crop_mode {
                    self.view.crop_mode = false;
                }
                if i.key_pressed(egui::Key::W) && !i.modifiers.any() {
                    self.view.picker = !self.view.picker;
                    if self.view.picker {
                        self.view.crop_mode = false;
                    }
                }
                if i.key_pressed(egui::Key::Escape) {
                    self.view.picker = false;
                    self.view.crop_mode = false;
                }
            });
            if zoom_step != 0 {
                self.step_zoom(zoom_step);
            }
            if copy {
                self.copy_settings();
            }
            if paste {
                self.paste_settings();
            }
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.small(self.document.save.message().unwrap_or(&self.status))
                    .on_hover_text(if self.view.monitor.is_some() {
                        "Display: custom ICC (disable compositor ICC conversion)"
                    } else {
                        "Display: sRGB (compositor may manage the monitor)"
                    });
                if !self.preview.status.is_empty() {
                    ui.separator();
                    ui.small(&self.preview.status);
                }
                self.preview_progress(ui);
                if !self.document.lightroom_notice.is_empty() {
                    ui.separator();
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(
                                self.document.lightroom_notice.lines().next().unwrap_or(""),
                            )
                            .small(),
                        )
                        .truncate(),
                    )
                    .on_hover_text(&self.document.lightroom_notice);
                }
                if self.document.save.is_protected() {
                    ui.separator();
                    ui.colored_label(
                        Color32::YELLOW,
                        egui::RichText::new(
                            "Saved edits protected; editing is temporary. Export or save a preset.",
                        )
                        .small(),
                    );
                }
            });
        });
    }

    /// Library preview progress, right-aligned inside an existing status row
    /// so its appearance never changes the layout.
    fn preview_progress(&self, ui: &mut egui::Ui) {
        if let Some(library) = &self.library
            && library.preview_progress_active()
        {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                library.preview_progress(ui);
            });
        }
    }
    fn filmstrip(&mut self, ui: &mut egui::Ui) {
        if let (Some(library), Some(current)) = (&mut self.library, self.document.catalog_photo) {
            let mut target = None;
            egui::Panel::bottom("catalog-filmstrip")
                .exact_size(128.)
                .frame(egui::Frame::new().fill(Color32::from_gray(26)))
                .show(ui, |ui| {
                    let (next, changed) = library.filmstrip(ui, current);
                    target = next;
                    if changed {
                        self.status = library.message.clone();
                    }
                });
            if let Some(id) = target
                && !self.activity.is_busy()
            {
                self.develop_catalog_photo(id);
            }
        } else if self.document.catalog_photo.is_none() {
            egui::Panel::bottom("filmstrip")
                .exact_size(104.)
                .show(ui, |ui| {
                    egui::ScrollArea::horizontal().show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let mut selected = None;
                            for (i, p) in self.document.files.iter().enumerate() {
                                ui.vertical(|ui| {
                                    if let Some(t) = self.preview.thumbs.get(p) {
                                        let response = ui.add(
                                            egui::Button::image(
                                                egui::Image::new(t)
                                                    .fit_to_exact_size(Vec2::new(108., 64.)),
                                            )
                                            .selected(self.document.path.as_ref() == Some(p)),
                                        );
                                        if response.clicked() {
                                            selected = Some(i);
                                        }
                                    } else if ui
                                        .add_sized(
                                            [108., 64.],
                                            egui::Button::new("RAW")
                                                .selected(self.document.path.as_ref() == Some(p)),
                                        )
                                        .clicked()
                                    {
                                        selected = Some(i);
                                    }
                                    ui.small(p.file_name().unwrap_or_default().to_string_lossy());
                                });
                            }
                            if let Some(i) = selected {
                                self.open(self.document.files[i].clone());
                            }
                        });
                    });
                });
        }
    }

    fn develop_panels(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("presets")
            .default_size(245.)
            .min_size(180.)
            .max_size(400.)
            .show(ui, |ui| {
                self.navigator_ui(ui);
                self.presets_ui(ui);
            });
        egui::Panel::right("adjustments")
            .default_size(330.)
            .min_size(300.)
            .max_size(400.)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let has_lightroom_edits = self.document.catalog_photo
                        .and_then(|id| self.library.as_ref().and_then(|library| library.photo(id)))
                        .is_some_and(|photo| photo.has_lightroom_edits);
                    if has_lightroom_edits && ui
                        .add_enabled(self.document.full().is_some() && !self.document.save.is_protected(), egui::Button::new("Apply compatible Lightroom edits"))
                        .on_hover_text("Apply supported settings as one undo step. Unsupported settings stay preserved in the catalog and are listed below.")
                        .clicked()
                    {
                        self.apply_lightroom_edits();
                    }
                    ui.add_enabled_ui(self.document.full().is_some() && !self.view.compare, |ui| self.controls(ui));
                });
            });
        egui::CentralPanel::default().show(ui, |ui| self.viewport_ui(ui));
    }

    fn pending_work(&mut self, ctx: &egui::Context) {
        if self.document.save.ready() && !self.document.history.in_gesture() {
            self.flush();
        }
        if self.document.save.needs_save() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        if let Some(path) = self.activity.pending_export().cloned() {
            egui::Window::new("Replace exported file?")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(path.display().to_string());
                    ui.horizontal(|ui| {
                        if ui.button("Replace").clicked() {
                            self.activity.cancel_overwrite();
                            self.start_export(path.clone(), true, ctx);
                        }
                        if ui.button("Cancel").clicked() {
                            self.activity.cancel_overwrite();
                        }
                    });
                });
        }
        if ctx.input(|i| i.viewport().close_requested())
            && (self.activity.is_exporting() || !self.flush())
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.close_confirm = true;
        }
        if self.close_confirm {
            egui::Window::new("Work still pending").show(ctx, |ui| {
                ui.label(if self.activity.is_exporting() {
                    "Wait for the export to finish before closing."
                } else {
                    "Edits could not be saved. Retry or save a preset before closing."
                });
                if ui.button("Keep editing").clicked() {
                    self.close_confirm = false;
                }
                if !self.activity.is_exporting() && ui.button("Close without saving").clicked() {
                    self.document.save.saved();
                    self.close_confirm = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        }
        if let Some(file) = ctx.input(|i| i.raw.dropped_files.first().cloned()) {
            self.open(file.path().to_path_buf());
        }
    }
}
