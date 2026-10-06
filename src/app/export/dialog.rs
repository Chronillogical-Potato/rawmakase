//! Lightroom's Export dialog, and the question it asks when the file exists.
use super::super::widgets::{confirm_modal, form_row, modal_frame, pretty_path, primary_button};
use super::Editor;
use crate::app::theme;
use crate::export_settings::{
    AfterExport, Destination, Existing, Format, Include, NameContext, Naming,
};
use eframe::egui::{self, Color32, Sense, Stroke, Vec2};
use std::{path::Path, sync::atomic::Ordering};

const WIDTH: f32 = 700.;
const HEIGHT: f32 = 600.;
/// The name the built-in watermark is listed under.
const SIMPLE_COPYRIGHT_LABEL: &str = "Simple Copyright Watermark";

fn destination_label(d: Destination) -> &'static str {
    match d {
        Destination::SameFolder => "Same folder as original photo",
        Destination::Desktop => "Desktop",
        Destination::Pictures => "Pictures",
        Destination::Folder => "Specific folder",
    }
}
fn existing_label(e: Existing) -> &'static str {
    match e {
        Existing::Ask => "Ask what to do",
        Existing::Unique => "Choose a new name for the exported file",
        Existing::Overwrite => "Overwrite without warning",
        Existing::Skip => "Skip",
    }
}

impl Editor {
    pub(in crate::app) fn export_windows(&mut self, ctx: &egui::Context) {
        self.conflict_window(ctx);
        if self.exports.dialog {
            self.export_dialog(ctx);
        }
        self.watermark_editor(ctx);
        self.export_report(ctx);
    }

    fn export_dialog(&mut self, ctx: &egui::Context) {
        if let Some(folder) = self.exports.picked.lock().ok().and_then(|mut p| p.take()) {
            self.exports.draft.folder = Some(folder);
            self.exports.draft.destination = Destination::Folder;
        }
        // Names and folders are shown for the first photo.
        let (source, captured, place) = self
            .exports
            .scope
            .photos
            .first()
            .map(|p| (p.source.clone(), p.captured.clone(), p.place))
            .unwrap_or_default();
        let count = self.exports.scope.photos.len();
        let chosen = self.exports.scope.chosen.max(count);
        // As the export names it.
        let at = NameContext {
            index: place,
            total: chosen,
            captured: captured.as_deref(),
        };
        let left_out = self.exports.scope.left_out.clone();
        let mut confirmed = None;
        let response = egui::Modal::new(egui::Id::new("export-dialog"))
            .backdrop_color(Color32::from_black_alpha(140))
            .frame(modal_frame())
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(WIDTH, HEIGHT), Sense::hover());
                ui.painter().text(
                    rect.left_top() + Vec2::new(24., 26.),
                    egui::Align2::LEFT_CENTER,
                    if count == 1 {
                        "Export One File".to_string()
                    } else {
                        format!("Export {count} Files")
                    },
                    egui::FontId::proportional(16.),
                    theme::gray(236),
                );
                // Said before the export starts: what will not be exported, and why.
                if !left_out.is_empty() {
                    let mut reasons: Vec<(String, usize)> = Vec::new();
                    for (_, why) in &left_out {
                        match reasons.iter_mut().find(|(r, _)| r == why) {
                            Some((_, n)) => *n += 1,
                            None => reasons.push((why.clone(), 1)),
                        }
                    }
                    let reasons = reasons
                        .iter()
                        .map(|(why, n)| format!("{why} ({n})"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let line = format!(
                        "{} won't be exported: {reasons}",
                        crate::app::widgets::plural(left_out.len(), "photo", "photos")
                    );
                    let at = egui::Rect::from_min_size(
                        rect.left_top() + Vec2::new(200., 16.),
                        Vec2::new(WIDTH - 224., 20.),
                    );
                    ui.put(
                        at,
                        egui::Label::new(
                            egui::RichText::new(line)
                                .size(12.)
                                .color(Color32::from_rgb(230, 170, 100)),
                        )
                        .truncate(),
                    )
                    .on_hover_text(
                        left_out
                            .iter()
                            .map(|(name, why)| format!("{name}: {why}"))
                            .collect::<Vec<_>>()
                            .join("\n"),
                    );
                }
                let body = egui::Rect::from_min_max(
                    rect.left_top() + Vec2::new(16., 52.),
                    rect.right_bottom() - Vec2::new(16., 64.),
                );
                let mut content = ui.new_child(egui::UiBuilder::new().max_rect(body));
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(&mut content, |ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(8., 8.);
                        ui.spacing_mut().interact_size.y = 26.;
                        self.export_sections(ui, &source, &at);
                    });
                ui.painter().hline(
                    rect.x_range(),
                    rect.bottom() - 64.,
                    Stroke::new(1., theme::gray(45)),
                );
                let footer = egui::Rect::from_min_max(
                    egui::pos2(rect.left() + 24., rect.bottom() - 56.),
                    egui::pos2(rect.right() - 24., rect.bottom() - 8.),
                );
                let mut bar = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(footer)
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                );
                bar.spacing_mut().button_padding = Vec2::new(18., 6.);
                if primary_button(&mut bar, "Export").clicked() {
                    confirmed = Some(true);
                }
                if bar
                    .add(egui::Button::new("Cancel").min_size(Vec2::new(84., 30.)))
                    .clicked()
                {
                    confirmed = Some(false);
                }
                if let Some((target, _)) = self.exports.draft.target_for(&source, &at) {
                    bar.add_space(12.);
                    bar.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!("Saves to {}", pretty_path(&target)))
                                    .size(12.)
                                    .color(theme::gray(140)),
                            )
                            .truncate(),
                        )
                        .on_hover_text(target.display().to_string());
                    });
                }
            });
        match confirmed.or(response.should_close().then_some(false)) {
            Some(true) if self.exports.draft.target(&source).is_none() => {
                self.status = "Choose a folder to export to".into();
            }
            Some(true) => {
                self.exports.dialog = false;
                self.export(self.exports.draft.clone());
            }
            Some(false) => self.exports.dialog = false,
            None => {}
        }
    }

    fn export_sections(&mut self, ui: &mut egui::Ui, source: &Path, example: &NameContext) {
        let picking = self.exports.picking.load(Ordering::Relaxed);
        let s = &mut self.exports.draft;
        let mut choose = false;
        section(ui, "Export Location");
        form_row(ui, "Export To", |ui| {
            egui::ComboBox::from_id_salt("export-to")
                .width(300.)
                .selected_text(destination_label(s.destination))
                .show_ui(ui, |ui| {
                    for d in Destination::ALL {
                        ui.selectable_value(&mut s.destination, d, destination_label(d));
                    }
                });
        });
        let folder = s.base_folder(source);
        form_row(ui, "Folder", |ui| {
            choose = ui
                .add_enabled(!picking, egui::Button::new("Choose…"))
                .clicked();
            let text = folder
                .as_deref()
                .map_or("No folder chosen".into(), pretty_path);
            ui.add(egui::Label::new(egui::RichText::new(text).color(theme::gray(150))).truncate());
        });
        form_row(ui, "", |ui| {
            ui.checkbox(&mut s.subfolder, "Put in Subfolder:");
            ui.add_enabled(
                s.subfolder,
                egui::TextEdit::singleline(&mut s.subfolder_name).desired_width(240.),
            );
        });
        form_row(ui, "Existing Files", |ui| {
            egui::ComboBox::from_id_salt("export-existing")
                .width(300.)
                .selected_text(existing_label(s.existing))
                .show_ui(ui, |ui| {
                    for e in Existing::ALL {
                        ui.selectable_value(&mut s.existing, e, existing_label(e));
                    }
                });
        });

        section(ui, "File Naming");
        let mut naming = s.naming();
        form_row(ui, "Rename To", |ui| {
            egui::ComboBox::from_id_salt("export-naming")
                .width(220.)
                .selected_text(naming.label())
                .show_ui(ui, |ui| {
                    for n in Naming::ALL {
                        ui.selectable_value(&mut naming, n, n.label());
                    }
                });
        });
        // Once chosen, the template is saved as chosen, whatever the older switch said.
        if naming != s.naming() {
            s.naming = Some(naming);
        }
        form_row(ui, "Custom Text", |ui| {
            ui.add_enabled(
                naming.uses_text(),
                egui::TextEdit::singleline(&mut s.custom_text).desired_width(220.),
            );
        });
        form_row(ui, "Start Number", |ui| {
            ui.add_enabled(
                naming.uses_sequence(),
                egui::DragValue::new(&mut s.start_number).range(0..=999_999),
            );
        });
        form_row(ui, "Example", |ui| {
            ui.label(
                egui::RichText::new(s.file_name_for(source, example).0).color(theme::gray(225)),
            );
        });
        form_row(ui, "Extensions", |ui| {
            egui::ComboBox::from_id_salt("export-case")
                .width(140.)
                .selected_text(if s.uppercase {
                    "Uppercase"
                } else {
                    "Lowercase"
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut s.uppercase, false, "Lowercase");
                    ui.selectable_value(&mut s.uppercase, true, "Uppercase");
                });
        });

        section(ui, "File Settings");
        form_row(ui, "Image Format", |ui| {
            egui::ComboBox::from_id_salt("export-format")
                .width(140.)
                .selected_text(match s.format {
                    Format::Jpeg => "JPEG",
                    Format::Tiff => "TIFF",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut s.format, Format::Jpeg, "JPEG");
                    ui.selectable_value(&mut s.format, Format::Tiff, "TIFF");
                });
        });
        // One row for either format, so nothing below moves.
        match s.format {
            Format::Jpeg => form_row(ui, "Quality", |ui| {
                ui.spacing_mut().slider_width = 220.;
                ui.add(egui::Slider::new(&mut s.quality, 1..=100));
            }),
            Format::Tiff => form_row(ui, "Bit Depth", |ui| {
                ui.label(
                    egui::RichText::new("16 bits/component, uncompressed").color(theme::gray(225)),
                );
            }),
        }
        form_row(ui, "Color Space", |ui| {
            ui.label(egui::RichText::new("sRGB").color(theme::gray(225)));
        });

        section(ui, "Image Sizing");
        form_row(ui, "", |ui| {
            ui.checkbox(&mut s.resize, "Resize to Fit: Long Edge");
            ui.add_enabled(
                s.resize,
                egui::DragValue::new(&mut s.long_edge)
                    .range(100..=30_000)
                    .suffix(" px"),
            );
        });
        form_row(ui, "Resolution", |ui| {
            ui.add(
                egui::DragValue::new(&mut s.ppi)
                    .range(1..=10_000)
                    .suffix(" pixels per inch"),
            );
        });

        section(ui, "Watermarking");
        let presets = self.exports.watermarks.clone();
        let simple = crate::watermark::SIMPLE_COPYRIGHT;
        let mut edit = None;
        form_row(ui, "", |ui| {
            ui.checkbox(&mut s.watermark, "Watermark:");
            let label = if s.watermark_name == simple {
                SIMPLE_COPYRIGHT_LABEL.to_string()
            } else {
                s.watermark_name.clone()
            };
            ui.add_enabled_ui(s.watermark, |ui| {
                egui::ComboBox::from_id_salt("export-watermark")
                    .width(260.)
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut s.watermark_name,
                            simple.to_string(),
                            SIMPLE_COPYRIGHT_LABEL,
                        );
                        for w in &presets {
                            ui.selectable_value(&mut s.watermark_name, w.name.clone(), &w.name);
                        }
                        ui.separator();
                        if ui.selectable_label(false, "Edit Watermarks…").clicked() {
                            edit = Some(
                                presets
                                    .iter()
                                    .find(|w| w.name == s.watermark_name)
                                    .cloned()
                                    .unwrap_or_default(),
                            );
                        }
                    });
            });
            // A preset whose image went missing is flagged here, and the
            // export would stop before rendering.
            let missing = s.watermark
                && presets
                    .iter()
                    .find(|w| w.name == s.watermark_name)
                    .is_some_and(crate::watermark::Watermark::image_missing);
            if missing {
                ui.label(
                    egui::RichText::new("Watermark image not found")
                        .size(12.)
                        .color(Color32::from_rgb(230, 120, 100)),
                );
            }
        });
        if let Some(w) = edit {
            self.exports.watermark_editor = Some(super::watermark_editor::WatermarkEditor::new(w));
            return;
        }
        let s = &mut self.exports.draft;
        section(ui, "Metadata");
        form_row(ui, "Include", |ui| {
            egui::ComboBox::from_id_salt("export-include")
                .width(260.)
                .selected_text(s.include.label())
                .show_ui(ui, |ui| {
                    for include in Include::ALL {
                        if include == Include::Custom {
                            ui.separator();
                        }
                        ui.selectable_value(&mut s.include, include, include.label());
                    }
                });
        });
        // Four rows in every mode, so nothing below moves.
        if s.include == Include::Custom {
            form_row(ui, "", |ui| {
                ui.checkbox(&mut s.capture, "Camera and capture info (EXIF)");
            });
            form_row(ui, "", |ui| {
                ui.add_space(24.);
                ui.add_enabled(
                    s.capture,
                    egui::Checkbox::new(&mut s.location, "Include location info"),
                );
            });
            form_row(ui, "", |ui| {
                ui.checkbox(&mut s.develop, "Develop settings (Camera Raw XMP)");
            });
            form_row(ui, "", |ui| {
                ui.checkbox(
                    &mut s.descriptive,
                    "Title, caption, creator, copyright, rating, label and keywords",
                );
            });
        } else {
            let forced = s.include.removes_location();
            let mut remove = s.remove_location || forced;
            form_row(ui, "", |ui| {
                ui.add_enabled(
                    !forced,
                    egui::Checkbox::new(&mut remove, "Remove Location Info"),
                );
            });
            if !forced {
                s.remove_location = remove;
            }
            for _ in 0..3 {
                form_row(ui, "", |_| {});
            }
        }
        let s = &mut self.exports.draft;
        section(ui, "Post-Processing");
        form_row(ui, "After Export", |ui| {
            egui::ComboBox::from_id_salt("export-after")
                .width(220.)
                .selected_text(s.after_export.label())
                .show_ui(ui, |ui| {
                    for after in AfterExport::ALL {
                        ui.selectable_value(&mut s.after_export, after, after.label());
                    }
                });
        });
        ui.add_space(8.);
        if choose {
            self.choose_export_folder(ui.ctx());
        }
    }

    /// The system folder chooser, on its own thread so the window keeps drawing.
    fn choose_export_folder(&mut self, ctx: &egui::Context) {
        self.exports.picking.store(true, Ordering::Relaxed);
        let picked = self.exports.picked.clone();
        let picking = self.exports.picking.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            if let Some(folder) = rfd::FileDialog::new().pick_folder()
                && let Ok(mut slot) = picked.lock()
            {
                *slot = Some(folder);
            }
            picking.store(false, Ordering::Relaxed);
            ctx.request_repaint();
        });
    }

    /// Ask's question for a batch, once for all its files that exist, in
    /// Lightroom's words; the answer applies to every one of them.
    fn conflict_window(&mut self, ctx: &egui::Context) {
        let Some(pending) = &self.exports.pending else {
            return;
        };
        let shown = 12;
        let mut files: Vec<String> = pending
            .conflicts
            .iter()
            .take(shown)
            .map(|p| pretty_path(p))
            .collect();
        if pending.conflicts.len() > shown {
            files.push(format!("and {} more", pending.conflicts.len() - shown));
        }
        let detail = format!(
            "{}\n\nDo you wish to overwrite the existing files, skip the existing files, or rename the exported files to avoid collision?",
            files.join("\n")
        );
        let Some(choice) = confirm_modal(
            ctx,
            "export-conflict",
            "The following files already exist.",
            &detail,
            false,
            &[
                ("Cancel", None),
                ("Skip", Some(Existing::Skip)),
                ("Use Unique Names", Some(Existing::Unique)),
                ("Overwrite", Some(Existing::Overwrite)),
            ],
            None,
        ) else {
            return;
        };
        let Some(pending) = self.exports.pending.take() else {
            return;
        };
        match choice {
            Some(answer) => self.plan_export(pending, Some(answer)),
            None => self.status = "Export cancelled".into(),
        }
    }
}

/// A Lightroom section band.
fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.), Sense::hover());
    ui.painter().rect_filled(rect, 3., theme::gray(44));
    ui.painter().text(
        rect.left_center() + Vec2::new(12., 0.),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(13.),
        theme::gray(235),
    );
}
