//! Lightroom's filmstrip, shown below Develop.
use super::grid::filter_caption;
use super::{Library, cell};
use crate::app::theme;
use eframe::egui::{self, Color32, Vec2};

impl Library {
    /// Lightroom's filmstrip for Develop: the current source's photos with the
    /// open one highlighted. Returns a photo to open and whether metadata changed.
    pub fn filmstrip(&mut self, ui: &mut egui::Ui, current: i64) -> (Option<i64>, bool) {
        let mut target = None;
        let mut changed = false;
        let photo = self.photo(current).cloned();
        let position = self
            .visible
            .iter()
            .position(|i| self.photos[*i].id == current);
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(10, 3))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(self.source_name())
                            .size(11.)
                            .color(theme::gray(200)),
                    );
                    ui.label(filter_caption(&match position {
                        Some(at) => format!("{} of {} photos", at + 1, self.visible.len()),
                        None => format!("{} photos", self.visible.len()),
                    }));
                    if let Some(p) = &photo {
                        ui.label(filter_caption(&format!(
                            "{}{}",
                            p.filename,
                            cell::copy_suffix(p)
                        )));
                    }
                    if photo.is_some() {
                        ui.add_space((ui.available_width() - 250.).max(8.));
                        changed = self.metadata_controls(ui, current, false);
                    }
                });
            });
        let height = ui.available_height().max(40.);
        egui::ScrollArea::horizontal()
            .id_salt("develop-filmstrip")
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.;
                    for n in 0..self.visible.len() {
                        let p = self.photos[self.visible[n]].clone();
                        let (rect, response) = ui.allocate_exact_size(
                            Vec2::new(height * 1.25, height),
                            egui::Sense::click(),
                        );
                        let active = p.id == current;
                        // Scroll only to bring the open photo into view: a photo
                        // already visible, e.g. one just clicked, stays put.
                        if active && self.strip_current != Some(current) {
                            if !ui.clip_rect().contains_rect(rect) {
                                response.scroll_to_me(None);
                            }
                            self.strip_current = Some(current);
                        }
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
                        self.request_previews(&p, ui.ctx());
                        let cell = rect.shrink(2.);
                        let base = theme::gray(if active {
                            120
                        } else if response.hovered() {
                            58
                        } else {
                            40
                        });
                        // Same cues as the grid: the label tints the cell, and
                        // flag and stars sit on a strip below the photo.
                        let fill = crate::app::photo_metadata::label_color(&p.label)
                            .map_or(base, |label| {
                                base.lerp_to_gamma(label, if active { 0.35 } else { 0.25 })
                            });
                        ui.painter().rect_filled(cell, 2., fill);
                        let strip = 14.;
                        if let Some(texture) = self.texture(&p) {
                            let area = egui::Rect::from_min_max(
                                cell.min + Vec2::splat(5.),
                                cell.max - Vec2::new(5., strip + 2.),
                            );
                            let size = texture.size_vec2();
                            let scale = (area.width() / size.x).min(area.height() / size.y);
                            let image = egui::Rect::from_center_size(area.center(), size * scale);
                            ui.painter().image(
                                texture.id(),
                                image,
                                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
                                Color32::WHITE,
                            );
                            if p.master.is_some() {
                                cell::copy_badge(ui.painter(), image, fill);
                            }
                        }
                        let y = cell.bottom() - strip / 2. - 2.;
                        let mut x = cell.left() + 6.;
                        if p.flag != 0 {
                            crate::app::photo_metadata::flag_icon(
                                ui.painter(),
                                egui::pos2(x + 4., y),
                                p.flag,
                                active,
                            );
                            x += 13.;
                        }
                        if p.rating > 0 {
                            ui.painter().text(
                                egui::pos2(x, y),
                                egui::Align2::LEFT_CENTER,
                                "★".repeat(p.rating as usize),
                                egui::FontId::proportional(9.),
                                theme::gray(if active { 30 } else { 200 }),
                            );
                        }
                        if let Some(menu) = cell::photo_menu(&response, &p) {
                            let is_edit = matches!(menu, cell::PhotoAction::Edit(_));
                            if let Some(id) = self.photo_action(ui.ctx(), &p, menu, false) {
                                target = Some(id).filter(|id| *id != current);
                            }
                            if is_edit {
                                // Filters may have changed the visible list.
                                changed = true;
                                break;
                            }
                        }
                        let context = crate::app::widgets::context_clicked(&response);
                        if response
                            .on_hover_text(format!("{}{}", p.filename, cell::copy_suffix(&p)))
                            .clicked()
                            && !active
                            && !context
                        {
                            target = Some(p.id);
                        }
                    }
                });
            });
        (target, changed)
    }
}
