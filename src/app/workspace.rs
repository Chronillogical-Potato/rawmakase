use super::Editor;
use super::dialogs::{CatalogDialog, FolderAction};
use super::state::Tool;
use super::widgets::{TOP_BAR_SEGMENTS, segment_bar};
use crate::app::theme;
use eframe::egui::{self, Color32, Vec2};
use std::time::Duration;

impl Editor {
    pub(super) fn metadata_shortcuts(&mut self, ctx: &egui::Context) {
        if self.activity.is_busy() {
            return;
        }
        // With a brush tool open, [ and ] size the brush instead of rating the photo.
        let brushing = !self.library_mode && matches!(self.view.tool, Tool::Remove | Tool::Mask);
        let auto_advance = self.auto_advance;
        let shortcut = crate::app::photo_metadata::shortcut(ctx)
            .filter(|(e, _)| {
                !(brushing && matches!(e, crate::app::photo_metadata::Edit::RatingDelta(_)))
            })
            // Photo > Auto Advance: every key moves on, as Shift does.
            .map(|(edit, shift)| (edit, shift || auto_advance));
        let Some(library) = &mut self.library else {
            return;
        };
        if self.library_mode {
            // The Grid applies a key to every selected photo, as Lightroom
            // does; Loupe to the photo it shows.
            let result = |library: &mut crate::app::library::Library, edit, advance| match library
                .selected()
                .filter(|_| library.loupe_open())
            {
                Some(id) => library.edit_metadata(id, edit, advance),
                None => library.edit_selection(edit, advance),
            };
            match shortcut {
                Some((edit, advance)) => {
                    match result(library, edit, advance) {
                        Ok(_) => self.status = library.message.clone(),
                        Err(e) => self.status = format!("Metadata could not be saved: {e}"),
                    }
                    // Logged now, as a Library change, whatever this frame does next.
                    self.sync_undo();
                }
                None => library.selection_keys(ctx),
            }
            return;
        }
        let (Some(id), Some((edit, advance))) = (self.document.catalog_photo, shortcut) else {
            return;
        };
        match library.edit_metadata(id, edit, advance) {
            Ok(next) => {
                self.status = library.message.clone();
                // Logged now, while this photo is still the one in Develop.
                self.sync_undo();
                if let Some(next) = next {
                    self.develop_catalog_photo(next);
                    if self.document.catalog_photo != Some(next)
                        && let Some(library) = &mut self.library
                    {
                        library.make_active(id);
                    }
                }
            }
            Err(e) => self.status = format!("Metadata could not be saved: {e}"),
        }
    }
    pub(super) fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.events(&ctx);
        self.poll_updates(&ctx);
        self.themes.poll(&ctx);
        if let Some(library) = &mut self.library {
            library.publish_shown();
            library.poll_previews(&ctx);
        }
        // Preferences is modal: keys go to it, not to the photo behind.
        let modal = self.preferences.open || self.export_modal() || self.remove_copy.is_some();
        if !modal {
            self.metadata_shortcuts(&ctx);
            self.workspace_shortcuts(&ctx);
            self.preferences_shortcut(&ctx);
        }
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
            if !modal {
                self.develop_shortcuts(&ctx);
            }
            // The Navigator column runs full height; the toolbar sits over
            // the photo and the adjustments only.
            self.status_bar(ui);
            self.filmstrip(ui);
            self.develop_left_panel(ui);
            self.toolbar(ui);
            self.develop_panels(ui);
            self.finish_edit_frame(frame, &ctx);
        }
        if let Some(request) = self.library.as_mut().and_then(|l| l.take_copy_request()) {
            self.virtual_copy(request);
        }
        self.remove_copy_window(&ctx);
        self.shortcuts_window(&ctx);
        self.preferences_window(&ctx);
        self.export_windows(&ctx);
        self.update_notice(&ctx, modal || self.view.shortcuts);
        #[cfg(feature = "telemetry")]
        self.usage_stats_notice(&ctx, modal || self.view.shortcuts);
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
        self.sync_undo();
        let place = self.current_place();
        if self.library.is_some() && place != self.saved_place {
            self.saved_place = place;
            let _ = self.save_session();
        }
    }
    fn workspace_shortcuts(&mut self, ctx: &egui::Context) {
        if !self.activity.is_busy() && !ctx.text_edit_focused() {
            // Cmd+G and Cmd+D are other commands (Stack, Select None).
            let plain = |key| {
                // The modifiers held for that key, not at the end of the frame.
                ctx.input(|i| {
                    i.events.iter().any(|e| {
                        matches!(e, egui::Event::Key { key: k, pressed: true, modifiers, .. }
                            if *k == key && !(modifiers.command || modifiers.ctrl || modifiers.alt))
                    })
                })
            };
            if self.library_mode
                && self
                    .library
                    .as_ref()
                    .and_then(|l| l.loupe_develops())
                    .is_some()
            {
                self.zoom_keys(ctx);
            }
            // Develop has its own keys; the log is the same.
            if self.library_mode {
                // Consumed, with the modifiers held for the key, so an undo
                // that opens Develop is not run again by Develop's keys.
                use egui::{Key, Modifiers};
                let (undo, redo) = ctx.input_mut(|i| {
                    let undo = i.consume_key(Modifiers::COMMAND, Key::Z);
                    let redo = i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z)
                        || (!cfg!(target_os = "macos")
                            && i.consume_key(Modifiers::COMMAND, Key::Y));
                    (undo, redo)
                });
                if undo {
                    self.undo();
                } else if redo {
                    self.redo();
                }
            }
            if plain(egui::Key::G) && self.flush() {
                self.library_mode = true;
                if let Some(library) = &mut self.library {
                    library.close_loupe();
                }
            }
            // E from Develop: the photo in the Library's Loupe.
            if !self.library_mode
                && plain(egui::Key::E)
                && self.flush()
                && let (Some(library), Some(id)) = (&mut self.library, self.document.catalog_photo)
            {
                self.library_mode = true;
                library.reveal(id);
                library.open_loupe();
            }
            // Lightroom's Create Virtual Copy, in Library and Develop.
            // Only the first key-down: a held key must not make copy after copy.
            let create = ctx.input(|i| {
                i.modifiers.command
                    && i.events.iter().any(|e| {
                        matches!(
                            e,
                            egui::Event::Key {
                                key: egui::Key::Quote,
                                pressed: true,
                                repeat: false,
                                ..
                            }
                        )
                    })
            });
            if create {
                let id = if self.library_mode {
                    self.library.as_ref().and_then(|l| l.selected())
                } else {
                    self.document.catalog_photo
                };
                if let Some(id) = id {
                    self.virtual_copy(crate::app::library::CopyAction::Create(id));
                }
            }
            if plain(egui::Key::D) {
                if self.library_mode {
                    if let Some(id) = self.library.as_mut().and_then(|l| l.selected_or_first()) {
                        self.develop_catalog_photo(id);
                    }
                } else {
                    self.library_mode = false;
                }
            }
        }
    }

    /// Lightroom's top panel: catalog menu on the left, module picker on the
    /// right. On macOS it is also the title bar, beside the traffic lights.
    fn workspace_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Panel::top("workspace-modes")
            .exact_size(BAR_HEIGHT)
            .frame(
                egui::Frame::new()
                    .fill(theme::gray(26))
                    .inner_margin(egui::Margin::symmetric(18, 0)),
            )
            .show(ui, |ui| {
                title_bar_drag(ui);
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.;
                    ui.add_space(fastframe_macos::traffic_light_inset(ui.ctx()));
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
                    let name = ui.painter().layout_no_wrap(
                        catalog,
                        egui::FontId::proportional(13.),
                        theme::gray(255),
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
                        ui.painter().rect_filled(rect, 4., theme::gray(38));
                    }
                    let color = theme::gray(if response.hovered() || open { 235 } else { 175 });
                    ui.painter()
                        .with_clip_rect(rect.shrink2(Vec2::new(10., 0.)))
                        .galley(
                            rect.left_center() + Vec2::new(10., -name.size().y / 2.),
                            name,
                            color,
                        );
                    super::icons::paint_at(
                        ui.painter(),
                        super::icons::Icon::ChevronDown,
                        rect.right_center() - Vec2::new(14., 0.),
                        11.,
                        color,
                    );
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
                        if ui
                            .add(egui::Button::new("Catalog Settings…").frame(false))
                            .clicked()
                        {
                            self.open_preferences(super::preferences::Tab::Catalog);
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
                    self.export_progress(ui);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 0.;
                        ui.add_enabled_ui(!self.activity.is_busy(), |ui| {
                            let setup = self.onboarding.visible;
                            // The setup assistant shows neither module as active.
                            let selected =
                                (!setup).then_some(if self.library_mode { 0 } else { 1 });
                            let [library, develop] = segment_bar(
                                ui,
                                ["Library", "Develop"],
                                selected,
                                &TOP_BAR_SEGMENTS,
                            );
                            if develop.on_hover_text("Develop · D").clicked() {
                                self.onboarding.visible = false;
                                if self.library_mode
                                    && let Some(id) = self
                                        .library
                                        .as_mut()
                                        .and_then(|library| library.selected_or_first())
                                {
                                    self.develop_catalog_photo(id);
                                } else {
                                    self.library_mode = false;
                                }
                            }
                            if library.on_hover_text("Library · G").clicked() && self.flush() {
                                self.onboarding.visible = false;
                                self.library_mode = true;
                            }
                            ui.add_space(12.);
                            let shortcut = if cfg!(target_os = "macos") {
                                "⌘,"
                            } else {
                                "Ctrl+,"
                            };
                            if super::preferences::gear_button(ui)
                                .on_hover_text(format!("Preferences · {shortcut}"))
                                .clicked()
                            {
                                self.open_preferences(super::preferences::Tab::General);
                            }
                        });
                    });
                });
            });
        egui::Panel::top("workspace-modes-rule")
            .exact_size(1.)
            .frame(egui::Frame::new().fill(theme::gray(16)))
            .show(ui, |_| {});
    }

    /// A RAW in the Library's Loupe: loaded as the document, as Develop
    /// does, and drawn by Develop's viewport without its tools, so it zooms
    /// the same way and D shows it in Develop at once.
    fn loupe_viewport(&mut self, ui: &mut egui::Ui, id: i64) {
        // A photo that failed to open is tried again the next time the
        // Loupe shows it, not every frame.
        let failed = self.document.catalog_photo == Some(id)
            && self.document.path.is_none()
            && !self.load.is_running();
        if self.document.catalog_photo != Some(id) || (failed && self.loupe_tried != Some(id)) {
            let Some(path) = self
                .library
                .as_ref()
                .and_then(|l| l.photo(id))
                .map(|p| p.path.clone())
            else {
                return;
            };
            // Moving on keeps the zoom, so the next photo is compared as it was.
            let zoom = (self.view.zoom100, self.view.zoom_level, self.view.pan);
            if !self.load_raw(path, Some(id)) {
                return;
            }
            (self.view.zoom100, self.view.zoom_level, self.view.pan) = zoom;
            self.loupe_tried = Some(id);
        }
        // Develop's tools and Before view stay in Develop.
        if self.view.tool != Tool::None || self.view.compare {
            self.view.tool = Tool::None;
            self.view.compare = false;
            self.schedule();
        }
        self.viewport_ui(ui);
    }
    fn library_workspace(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Panel::bottom("library-status").show(ui, |ui| {
            ui.horizontal(|ui| {
                if let Some(work) = &self.catalog_work {
                    ui.add(egui::Spinner::new().size(11.));
                    ui.small(work);
                } else {
                    ui.small(
                        self.library
                            .as_ref()
                            .filter(|l| !l.message.is_empty())
                            .map_or(self.status.as_str(), |l| l.message.as_str()),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let toggle = ui
                        .checkbox(
                            &mut self.auto_advance,
                            egui::RichText::new("Auto Advance").small(),
                        )
                        .on_hover_text(
                            "Photo > Auto Advance: a rating, flag or label moves on to the next photo",
                        );
                    if toggle.changed() {
                        let _ = self.save_session();
                    }
                    if let Some(library) = &self.library
                        && library.preview_progress_active()
                    {
                        library.preview_progress(ui);
                    }
                });
            });
        });
        let mut action = crate::app::library::Action::None;
        let develops = self.library.as_ref().and_then(|l| l.loupe_develops());
        if develops != self.loupe_tried {
            self.loupe_tried = None;
        }
        egui::Panel::left("library-sidebar")
            .default_size(260.)
            .min_size(180.)
            .max_size(500.)
            .show(ui, |ui| {
                // A RAW in the Loupe gets Develop's Navigator and zoom levels.
                if develops.is_some() {
                    self.navigator_ui(ui);
                }
                if let Some(library) = &mut self.library {
                    action = library.sidebar(ui, develops.is_none());
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
                    if let Some(id) = l.loupe_develops() {
                        self.loupe_viewport(ui, id);
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

    /// Develop's zoom keys, shared with the Library's Loupe: Cmd+= and Cmd+-
    /// step through the zoom levels, Z toggles Fit and the last zoom, F fits.
    pub(super) fn zoom_keys(&mut self, ctx: &egui::Context) {
        let zoom_step = ctx.input(|i| {
            if i.modifiers.command
                && (i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals))
            {
                1
            } else if i.modifiers.command && i.key_pressed(egui::Key::Minus) {
                -1
            } else {
                0
            }
        });
        if zoom_step != 0 {
            self.step_zoom(zoom_step);
        }
        ctx.input(|i| {
            // Once per press: a held Z must not flicker the zoom.
            let z = i.events.iter().any(|e| {
                matches!(e, egui::Event::Key { key: egui::Key::Z, pressed: true, repeat: false, modifiers, .. }
                    if !modifiers.any())
            });
            if z {
                self.view.zoom100 = !self.view.zoom100;
            }
            if i.key_pressed(egui::Key::F) && !i.modifiers.any() {
                self.view.zoom100 = false;
            }
        });
    }
    pub(super) fn develop_shortcuts(&mut self, ctx: &egui::Context) {
        if !self.activity.is_busy() && !ctx.text_edit_focused() {
            self.zoom_keys(ctx);
            let (mut copy, mut paste, mut reset) = (false, false, false);
            let mut auto = false;
            let mut export = None;
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
                if i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::R) {
                    reset = true;
                }
                // Lightroom's Auto Settings.
                if i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::U) {
                    auto = true;
                }
                // Shift+Cmd+E exports, Option+Shift+Cmd+E exports with the previous
                // choices. Option changes the typed letter on macOS, so match the
                // physical key too.
                let e = i.events.iter().any(|event| {
                    matches!(event, egui::Event::Key { key, physical_key, pressed: true, repeat: false, .. }
                        if *key == egui::Key::E || *physical_key == Some(egui::Key::E))
                });
                if e && i.modifiers.command && i.modifiers.shift {
                    export = Some(i.modifiers.alt);
                }
                // Cmd+Z / Cmd+Shift+Z on macOS, Ctrl+Z / Ctrl+Shift+Z or Ctrl+Y elsewhere.
                if i.modifiers.command && i.key_pressed(egui::Key::Z) {
                    if i.modifiers.shift {
                        self.redo();
                    } else {
                        self.undo();
                    }
                }
                if !cfg!(target_os = "macos")
                    && i.modifiers.command
                    && !i.modifiers.shift
                    && i.key_pressed(egui::Key::Y)
                {
                    self.redo();
                }
                if (i.key_pressed(egui::Key::C) || i.key_pressed(egui::Key::R))
                    && !i.modifiers.command
                {
                    self.view.toggle(Tool::Crop);
                }
                if i.key_pressed(egui::Key::J) && !i.modifiers.shift {
                    self.view.clipping = !self.view.clipping;
                }
                if i.key_pressed(egui::Key::Backslash) {
                    self.view.compare = !self.view.compare;
                }
                if i.key_pressed(egui::Key::Enter) && self.view.is(Tool::Crop) {
                    self.view.tool = Tool::None;
                }
                if i.key_pressed(egui::Key::W) && !i.modifiers.any() {
                    self.view.toggle(Tool::WhiteBalance);
                }
                if i.key_pressed(egui::Key::Q) && !i.modifiers.any() {
                    self.view.toggle(Tool::Remove);
                }
                if i.key_pressed(egui::Key::W) && i.modifiers.shift && !i.modifiers.command {
                    self.view.toggle(Tool::Mask);
                }
                if i.key_pressed(egui::Key::Escape) {
                    self.view.tool = Tool::None;
                }
                if self.view.is(Tool::Remove) {
                    self.retouch_keys(i);
                }
                if self.view.is(Tool::Mask) {
                    self.mask_keys(i);
                }
                // New masks: K brush, M linear, Shift+M radial, Shift+J colour range.
                if !i.modifiers.command && !i.modifiers.alt {
                    use super::mask_tool::Kind;
                    let kind = if i.key_pressed(egui::Key::K) && !i.modifiers.shift {
                        Some(Kind::Brush)
                    } else if i.key_pressed(egui::Key::M) {
                        Some(if i.modifiers.shift { Kind::Radial } else { Kind::Linear })
                    } else if i.key_pressed(egui::Key::J) && i.modifiers.shift {
                        Some(Kind::Color)
                    } else {
                        None
                    };
                    if let Some(kind) = kind {
                        self.create_mask(kind, None);
                    }
                }
            });
            if copy {
                self.copy_settings();
            }
            if reset {
                self.reset_settings();
            }
            if auto && !self.auto_in_effect() {
                self.start_auto(super::worker::AutoKind::Settings);
            }
            match export {
                Some(true) => self.export_with_previous(),
                Some(false) => self.open_export_dialog(),
                None => {}
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
                .frame(egui::Frame::new().fill(theme::gray(26)))
                .show(ui, |ui| {
                    let (next, changed) = library.filmstrip(ui, current);
                    target = next;
                    if changed {
                        self.status = library.message.clone();
                    }
                });
            // In Develop both a click and Open in Develop show the photo.
            if let Some(
                crate::app::library::Pick::Show(id) | crate::app::library::Pick::Develop(id),
            ) = target
                && id != current
                && !self.activity.is_busy()
            {
                self.develop_catalog_photo(id);
            }
        }
    }

    fn develop_left_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("presets")
            .default_size(245.)
            .min_size(180.)
            .max_size(400.)
            .show(ui, |ui| {
                self.navigator_ui(ui);
                self.presets_ui(ui);
            });
    }
    fn develop_panels(&mut self, ui: &mut egui::Ui) {
        egui::Panel::right("adjustments")
            .default_size(330.)
            .min_size(300.)
            .max_size(400.)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add_enabled_ui(self.document.full().is_some() && !self.view.compare, |ui| {
                        self.controls(ui)
                    });
                });
            });
        egui::CentralPanel::default().show(ui, |ui| self.viewport_ui(ui));
    }

    fn pending_work(&mut self, ctx: &egui::Context) {
        self.autosave(ctx);
        if self.document.save.needs_save() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        if ctx.input(|i| i.viewport().close_requested()) && (self.exporting() || !self.flush()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.close_confirm = true;
        }
        if self.close_confirm {
            egui::Window::new("Work still pending").show(ctx, |ui| {
                ui.label(if self.exporting() {
                    "Wait for the export to finish before closing."
                } else {
                    "Edits could not be saved. Retry or save a preset before closing."
                });
                if ui.button("Keep editing").clicked() {
                    self.close_confirm = false;
                }
                if !self.exporting() && ui.button("Close without saving").clicked() {
                    self.document.save.saved();
                    if let Some(library) = &mut self.library {
                        library.discard_copy_name();
                    }
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

/// The workspace bar's height; on macOS the traffic lights sit on its centre.
pub(super) const BAR_HEIGHT: f32 = 44.;

/// The bar's empty space moves the window, and a double click does what
/// System Settings says, as a title bar does. Controls drawn later take their
/// own clicks. Only macOS hides the system title bar.
fn title_bar_drag(ui: &mut egui::Ui) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let bar = ui.max_rect().expand2(Vec2::new(18., 0.));
    let response = ui.interact(
        bar,
        ui.id().with("title-bar"),
        egui::Sense::click_and_drag(),
    );
    let ctx = ui.ctx();
    if response.double_clicked() {
        match fastframe_macos::double_click_action() {
            fastframe_macos::DoubleClick::Minimize => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
            fastframe_macos::DoubleClick::Zoom => {
                let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            }
            // AppKit fills the screen itself, from the drag started below.
            fastframe_macos::DoubleClick::Fill | fastframe_macos::DoubleClick::Nothing => {}
        }
    } else if response.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_pressed()) {
        // AppKit only starts a drag during the original mouse-down.
        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
}
