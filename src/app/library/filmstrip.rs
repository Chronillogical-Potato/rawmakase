//! Lightroom's filmstrip: one strip across the bottom of the window, below
//! both side panels, in every Library view and in Develop. It keeps its
//! width and scroll position as the views change; what a click does is up
//! to the view shown.
use super::grid::filter_caption;
use super::selection::Mark;
use super::views::View;
use super::{Action, Library, cell};
use crate::app::theme;
use eframe::egui::{self, Color32, Vec2};

/// The strip's height, the same in every view.
pub const HEIGHT: f32 = 128.;

/// A photo chosen in the filmstrip: clicked, or opened from its menu.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pick {
    Show(i64),
    Develop(i64),
}

impl Library {
    /// The filmstrip's panel, at the bottom of the window. One panel and one
    /// scroll area serve every view, so the strip stays where it was when
    /// they change. `current` is the photo shown (Develop's, or the active
    /// one); `library` marks the selection as the grid does.
    pub fn filmstrip_panel(
        &mut self,
        ui: &mut egui::Ui,
        current: Option<i64>,
        library: bool,
    ) -> (Option<Pick>, bool) {
        let mut out = (None, false);
        egui::Panel::bottom("filmstrip")
            .exact_size(HEIGHT)
            .frame(egui::Frame::new().fill(theme::gray(26)))
            .show(ui, |ui| out = self.filmstrip(ui, current, library));
        out
    }
    /// A filmstrip click in the Library, as the view shown takes it: Grid,
    /// Loupe and Survey select as the grid does (Cmd and Shift add), Compare
    /// makes the photo its candidate, and Select activates its side.
    pub fn filmstrip_pick(&mut self, pick: Pick, modifiers: egui::Modifiers) -> Action {
        match pick {
            Pick::Develop(id) => Action::Develop(id),
            Pick::Show(id) if self.compare.open => {
                self.compare_pick(id);
                Action::None
            }
            Pick::Show(id) => {
                self.click(id, modifiers);
                // The grid follows a photo chosen below it.
                self.scroll_to_active = true;
                Action::None
            }
        }
    }
    /// The photos in the current source, with `current` highlighted and,
    /// in the Library, the rest of the selection marked. With no photo
    /// current the strip still shows them. Returns a photo chosen and
    /// whether metadata changed.
    pub(super) fn filmstrip(
        &mut self,
        ui: &mut egui::Ui,
        current: Option<i64>,
        library: bool,
    ) -> (Option<Pick>, bool) {
        let mut target = None;
        let mut changed = false;
        let photo = current.and_then(|id| self.photo(id)).cloned();
        let position = current.and_then(|current| {
            self.visible
                .iter()
                .position(|i| self.photos[*i].id == current)
        });
        // The grid's edits cover its selection; elsewhere the photo shown.
        let whole_selection = library && self.view() == View::Grid;
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(10, 3))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(self.source_name())
                            .size(11.)
                            .color(theme::gray(200)),
                    );
                    let selected = if library {
                        self.selection.selected.len()
                    } else {
                        0
                    };
                    ui.label(filter_caption(&match position {
                        Some(_) if selected > 1 => {
                            format!("{} of {} photos selected", selected, self.visible.len())
                        }
                        Some(at) => format!("{} of {} photos", at + 1, self.visible.len()),
                        None => format!("{} photos", self.visible.len()),
                    }));
                    if let Some(p) = &photo {
                        ui.label(filter_caption(&format!(
                            "{}{}",
                            p.filename,
                            cell::copy_suffix(p)
                        )));
                        ui.add_space((ui.available_width() - 250.).max(8.));
                        changed = self.metadata_controls(ui, p.id, whole_selection);
                    }
                });
            });
        // Scroll only to bring a newly shown photo into view, or one a sort
        // or filter moved: a photo already visible, e.g. one just clicked,
        // stays put, and so does a strip scrolled away from it.
        let reveal = current.zip(position);
        let reveal = if reveal != self.strip_revealed {
            self.strip_revealed = reveal;
            reveal.map(|(id, _)| id)
        } else {
            None
        };
        let height = ui.available_height().max(40.);
        egui::ScrollArea::horizontal()
            .id_salt("filmstrip")
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
                        let mark = if current == Some(p.id) {
                            Mark::Active
                        } else if library && self.selection.selected.contains(&p.id) {
                            Mark::Selected
                        } else {
                            Mark::None
                        };
                        let active = mark == Mark::Active;
                        if reveal == Some(p.id) && !ui.clip_rect().contains_rect(rect) {
                            response.scroll_to_me(None);
                        }
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
                        self.request_previews(&p, ui.ctx());
                        let cell = rect.shrink(2.);
                        let base = theme::gray(if active {
                            120
                        } else if mark == Mark::Selected {
                            78
                        } else if response.hovered() {
                            58
                        } else {
                            40
                        });
                        // Same cues as the grid: the label tints the cell, and
                        // flag and stars sit on a strip below the photo.
                        let fill = crate::app::photo_metadata::label_color(&p.label).map_or(
                            base,
                            |label| {
                                base.lerp_to_gamma(
                                    label,
                                    if mark == Mark::None { 0.25 } else { 0.35 },
                                )
                            },
                        );
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
                        if let Some(menu) =
                            cell::photo_menu(&response, &p, self.is_available(&p.path))
                        {
                            let is_edit = matches!(menu, cell::PhotoAction::Edit(_));
                            if let Some(id) = self.photo_action(ui.ctx(), &p, menu, whole_selection)
                            {
                                target = Some(Pick::Develop(id));
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
                            && !context
                        {
                            target = Some(Pick::Show(p.id));
                        }
                    }
                });
            });
        (target, changed)
    }
}
