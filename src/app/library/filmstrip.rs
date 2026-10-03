//! Lightroom's filmstrip: one strip across the bottom of the window, below
//! both side panels, in every Library view and in Develop. It keeps its
//! width and scroll position as the views change; what a click does is up
//! to the view shown.
use super::grid::filter_caption;
use super::selection::Mark;
use super::views::View;
use super::{Action, Library, cell};
use crate::app::theme;
use crate::catalog::Photo;
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
    /// Whether the selection changed after the strip was drawn, as a click
    /// in the grid below does: the strip then needs another frame to mark
    /// it and bring it into view.
    pub fn filmstrip_behind(&self) -> bool {
        self.strip_drawn != self.selection
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
        if self.strip_drawn != self.selection {
            self.strip_drawn = self.selection.clone();
        }
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
        let shown = current.zip(position);
        let reveal = if shown != self.strip_revealed {
            self.strip_revealed = shown;
            position
        } else {
            None
        };
        let height = ui.available_height().max(40.);
        let size = Vec2::new(height * 1.25, height);
        egui::ScrollArea::horizontal()
            .id_salt("filmstrip")
            .auto_shrink(false)
            .show_viewport(ui, |ui, viewport| {
                // Only the cells in view are laid out and drawn, however
                // many photos the source has.
                ui.set_min_size(Vec2::new(size.x * self.visible.len() as f32, height));
                let origin = ui.max_rect().min;
                let at = |n: usize| {
                    egui::Rect::from_min_size(origin + Vec2::new(size.x * n as f32, 0.), size)
                };
                if let Some(n) = reveal {
                    let rect = at(n);
                    if !ui.clip_rect().contains_rect(rect) {
                        ui.scroll_to_rect(rect, None);
                    }
                }
                let first = (viewport.min.x / size.x).floor().max(0.) as usize;
                let last = ((viewport.max.x / size.x).ceil() as usize).min(self.visible.len());
                for n in first..last {
                    let photo = self.photos[self.visible[n]].clone();
                    let mark = if current == Some(photo.id) {
                        Mark::Active
                    } else if library && self.selection.selected.contains(&photo.id) {
                        Mark::Selected
                    } else {
                        Mark::None
                    };
                    let (pick, edited) = self.strip_cell(ui, at(n), &photo, mark, whole_selection);
                    target = pick.or(target);
                    if edited {
                        // Filters may have changed the visible list.
                        changed = true;
                        break;
                    }
                }
            });
        (target, changed)
    }
    /// One photo in the strip, with its menu. Returns a photo chosen and
    /// whether the menu changed its metadata.
    fn strip_cell(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        photo: &Photo,
        mark: Mark,
        whole_selection: bool,
    ) -> (Option<Pick>, bool) {
        let response = ui.interact(rect, ui.id().with(photo.id), egui::Sense::click());
        self.request_previews(photo, ui.ctx());
        paint_cell(
            ui.painter(),
            rect,
            photo,
            self.texture(photo),
            mark,
            response.hovered(),
        );
        if let Some(menu) = cell::photo_menu(&response, photo, self.is_available(&photo.path)) {
            let edited = matches!(menu, cell::PhotoAction::Edit(_));
            let develop = self.photo_action(ui.ctx(), photo, menu, whole_selection);
            return (develop.map(Pick::Develop), edited);
        }
        let context = crate::app::widgets::context_clicked(&response);
        let clicked = response
            .on_hover_text(format!("{}{}", photo.filename, cell::copy_suffix(photo)))
            .clicked();
        ((clicked && !context).then_some(Pick::Show(photo.id)), false)
    }
}

/// A strip cell: the preview over a row for its flag and stars, tinted by
/// its label, with the same cues as the grid.
fn paint_cell(
    painter: &egui::Painter,
    rect: egui::Rect,
    photo: &Photo,
    texture: Option<&egui::TextureHandle>,
    mark: Mark,
    hovered: bool,
) {
    let active = mark == Mark::Active;
    let cell = rect.shrink(2.);
    let base = theme::gray(match mark {
        Mark::Active => 120,
        Mark::Selected => 78,
        Mark::None if hovered => 58,
        Mark::None => 40,
    });
    let fill = crate::app::photo_metadata::label_color(&photo.label).map_or(base, |label| {
        base.lerp_to_gamma(label, if mark == Mark::None { 0.25 } else { 0.35 })
    });
    painter.rect_filled(cell, 2., fill);
    let strip = 14.;
    if let Some(texture) = texture {
        let area = egui::Rect::from_min_max(
            cell.min + Vec2::splat(5.),
            cell.max - Vec2::new(5., strip + 2.),
        );
        let size = texture.size_vec2();
        let scale = (area.width() / size.x).min(area.height() / size.y);
        let image = egui::Rect::from_center_size(area.center(), size * scale);
        painter.image(
            texture.id(),
            image,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
            Color32::WHITE,
        );
        if photo.master.is_some() {
            cell::copy_badge(painter, image, fill);
        }
    }
    let y = cell.bottom() - strip / 2. - 2.;
    let mut x = cell.left() + 6.;
    if photo.flag != 0 {
        crate::app::photo_metadata::flag_icon(painter, egui::pos2(x + 4., y), photo.flag, active);
        x += 13.;
    }
    if photo.rating > 0 {
        painter.text(
            egui::pos2(x, y),
            egui::Align2::LEFT_CENTER,
            "★".repeat(photo.rating as usize),
            egui::FontId::proportional(9.),
            theme::gray(if active { 30 } else { 200 }),
        );
    }
}
