//! The grid, with the filter bar above it and its toolbar below.
use super::cell::{self, photo_cell};
use super::{Action, Library};
use crate::app::theme;
use crate::catalog::Photo;
use eframe::egui::{self, Vec2};

impl Library {
    pub(super) fn grid_toolbar(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(theme::gray(38))
            .inner_margin(egui::Margin::symmetric(10, 4))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    self.view_buttons(ui);
                    ui.add_space(12.);
                    ui.label(filter_caption("Sort"));
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(if self.filters.reverse {
                                    "Capture Time ↓"
                                } else {
                                    "Capture Time ↑"
                                })
                                .size(11.),
                            )
                            .small()
                            .frame(false),
                        )
                        .on_hover_text("Reverse the sort order")
                        .clicked()
                    {
                        self.filters.reverse = !self.filters.reverse;
                        self.filter();
                    }
                    ui.add_space(12.);
                    ui.small(if self.visible.len() == self.photos.len() {
                        format!("{} photos", self.photos.len())
                    } else {
                        format!("{} of {} photos", self.visible.len(), self.photos.len())
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().slider_width = 110.;
                        ui.add(
                            egui::Slider::new(&mut self.thumb_size, 110. ..=360.).show_value(false),
                        );
                        ui.label(filter_caption("Thumbnails"));
                    });
                });
            });
    }
    /// The grid, or the Loupe in its place at the shared `zoom`.
    pub(in crate::app) fn grid(
        &mut self,
        ui: &mut egui::Ui,
        zoom: &mut crate::app::navigator::Zoom,
    ) -> Action {
        self.poll_previews(ui.ctx());
        if self.compare.open {
            return self.compare(ui);
        }
        if self.survey.open {
            return self.survey(ui);
        }
        if self.loupe.open {
            return self.loupe(ui, zoom);
        }
        let mut action = Action::None;
        self.filter_bar(ui);
        egui::Panel::bottom("library-grid-toolbar")
            .frame(egui::Frame::new())
            .show_separator_line(false)
            .show(ui, |ui| self.grid_toolbar(ui));
        let columns = ((ui.available_width() / self.thumb_size).floor() as usize).max(1);
        let width = (ui.available_width() / columns as f32).floor().max(80.);
        self.grid_columns = columns;
        let mut metadata_edit = None;
        let spacing = ui.spacing().item_spacing;
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt("library-grid")
            .auto_shrink(false);
        // A re-sort moved the selected photo: scroll by the rows it moved.
        if let Some((id, before)) = self.keep_in_place.take()
            && let Some(after) = self.visible.iter().position(|i| self.photos[*i].id == id)
        {
            let rows = (after / columns) as f32 - (before / columns) as f32;
            scroll = scroll.vertical_scroll_offset((self.grid_offset + rows * width).max(0.));
        }
        egui::Frame::new().fill(theme::gray(44)).show(ui, |ui| {
            // A key moved the active photo: bring its row into view.
            if std::mem::take(&mut self.scroll_to_active)
                && let Some(at) = self
                    .selection
                    .active
                    .and_then(|id| self.visible.iter().position(|i| self.photos[*i].id == id))
            {
                let top = (at / columns) as f32 * width;
                let height = ui.available_height();
                if top < self.grid_offset {
                    scroll = scroll.vertical_scroll_offset(top);
                } else if top + width > self.grid_offset + height {
                    scroll = scroll.vertical_scroll_offset(top + width - height);
                }
            }
            let output = scroll.show_rows(
                ui,
                width,
                self.visible.len().div_ceil(columns),
                |ui, rows| {
                    self.grid_shown = rows.start * columns..rows.end * columns;
                    for row in rows {
                        ui.horizontal(|ui| {
                            for col in 0..columns {
                                let Some(&index) = self.visible.get(row * columns + col) else {
                                    break;
                                };
                                let p = self.photos[index].clone();
                                let exists = self.is_available(&p.path);
                                self.request_previews(&p, ui.ctx());
                                let shown = cell::Shown {
                                    mark: self.mark(p.id),
                                    number: row * columns + col + 1,
                                    available: exists,
                                    quick: self.in_quick(p.id),
                                };
                                let (response, edit) =
                                    photo_cell(ui, &p, self.texture(&p), shown, width);
                                let response = if response.hovered() {
                                    let text = self.hover_text(&p);
                                    response.on_hover_text(text)
                                } else {
                                    response
                                };
                                if response.clicked() {
                                    self.click(p.id, ui.input(|i| i.modifiers));
                                } else if response.secondary_clicked() {
                                    // A menu on a selected photo acts on the selection.
                                    self.make_active(p.id);
                                }
                                // As in Lightroom, a double-click opens the Loupe.
                                if response.double_clicked() {
                                    self.make_active(p.id);
                                    self.open_loupe();
                                }
                                if let Some(edit) = edit {
                                    metadata_edit = Some((p.clone(), edit));
                                }
                            }
                        });
                    }
                },
            );
            self.grid_offset = output.state.offset.y;
        });
        ui.spacing_mut().item_spacing = spacing;
        if let Some((photo, menu)) = metadata_edit
            && let Some(id) = self.photo_action(ui.ctx(), &photo, menu, true)
        {
            action = Action::Develop(id);
        }
        action
    }
    /// Carries out a thumbnail menu choice; returns a photo to open in Develop.
    /// A metadata change covers the selection when `whole_selection` (the
    /// grid) and the photo is in it, else that photo alone (the filmstrip).
    pub(super) fn photo_action(
        &mut self,
        ctx: &egui::Context,
        photo: &Photo,
        action: cell::PhotoAction,
        whole_selection: bool,
    ) -> Option<i64> {
        use cell::PhotoAction;
        match action {
            PhotoAction::Develop => return Some(photo.id),
            PhotoAction::Reveal => {
                if let Err(e) = crate::platform::reveal::reveal(&photo.path) {
                    self.message = format!("Could not show {}: {e}", photo.filename);
                }
            }
            PhotoAction::CopyPath => {
                ctx.copy_text(photo.path.display().to_string());
                self.message = format!("Copied {}", photo.path.display());
            }
            PhotoAction::Edit(edit) => {
                let ids = if whole_selection && self.selection.selected.contains(&photo.id) {
                    self.selected_ids()
                } else {
                    vec![photo.id]
                };
                if let Err(e) = self.edit_photos(&ids, edit, false) {
                    self.message = format!("Metadata could not be saved: {e}");
                }
            }
            PhotoAction::Copy(copy) => self.copy_request = Some(copy),
        }
        None
    }
}
pub(super) fn filter_caption(text: &str) -> egui::RichText {
    egui::RichText::new(text).size(11.).color(theme::gray(150))
}
