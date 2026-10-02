//! Lightroom's Compare (C): the select and a candidate side by side, to keep
//! the better of two. Left and Right move the candidate along the photos
//! shown, Up makes it the select, Down swaps the two. A click makes a photo
//! active: rating, flag and label keys go to it, and the panels show it.
//! Both are shown at the size of their half, with their edits (see `screen`).
use super::grid::filter_caption;
use super::screen::Shown;
use super::{Action, Library, Pick};
use crate::app::photo_metadata::{flag_icon, label_color};
use crate::app::theme;
use crate::catalog::Photo;
use eframe::egui::{self, Color32, Rect, Vec2};

/// One of the two photos.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Side {
    #[default]
    Select,
    Candidate,
}
impl Side {
    fn name(self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::Candidate => "Candidate",
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct Compare {
    pub open: bool,
    pub select: Option<i64>,
    /// None when no other photo is shown.
    pub candidate: Option<i64>,
    pub active: Side,
}
impl Compare {
    fn id(&self, side: Side) -> Option<i64> {
        match side {
            Side::Select => self.select,
            Side::Candidate => self.candidate,
        }
    }
}

/// The strip under each photo: its role, name, rating, flag and label.
const CAPTION: f32 = 26.;
/// Space between the photos and around them.
const MARGIN: f32 = 14.;

impl Library {
    pub fn compare_open(&self) -> bool {
        self.compare.open
    }
    /// Whether rating, flag and label keys go to the active photo alone, as
    /// in the Loupe and Compare, rather than to every photo selected.
    pub fn edits_active_only(&self) -> bool {
        self.loupe.open || self.compare.open
    }
    /// G: the grid, from the Loupe or Compare.
    pub fn show_grid(&mut self) {
        self.close_loupe();
        self.close_compare();
    }
    /// C: the active photo as the select, beside the next photo selected
    /// with it, or else the next one shown.
    pub fn open_compare(&mut self) {
        let Some(select) = self
            .selection
            .active
            .or_else(|| self.visible.first().map(|i| self.photos[*i].id))
        else {
            return;
        };
        let others: Vec<i64> = self
            .selected_ids()
            .into_iter()
            .filter(|id| *id != select)
            .collect();
        let at = self
            .visible
            .iter()
            .position(|i| self.photos[*i].id == select);
        let after = |id: &i64| {
            let position = self.visible.iter().position(|i| self.photos[*i].id == *id);
            position > at
        };
        let candidate = others
            .iter()
            .find(|id| after(id))
            .or(others.first())
            .copied()
            .or_else(|| self.next_candidate(select, None, 1))
            .or_else(|| self.next_candidate(select, None, -1));
        self.loupe.open = false;
        self.loupe.reset();
        self.compare = Compare {
            open: true,
            select: Some(select),
            candidate,
            active: Side::Select,
        };
        self.sync_compare_selection();
    }
    /// Esc or G: back to the grid, with both photos selected.
    pub fn close_compare(&mut self) {
        if self.compare.open {
            self.compare.open = false;
            self.scroll_to_active = true;
        }
    }
    /// The photo `by` steps from `from` (or from the select) among those
    /// shown, passing over the select; None at either end.
    fn next_candidate(&self, select: i64, from: Option<i64>, by: isize) -> Option<i64> {
        let ids: Vec<i64> = self.visible.iter().map(|i| self.photos[*i].id).collect();
        let from = from.filter(|id| ids.contains(id)).unwrap_or(select);
        let mut at = ids.iter().position(|id| *id == from)? as isize;
        loop {
            at += by.signum();
            let id = *ids.get(usize::try_from(at).ok()?)?;
            if id != select {
                return Some(id);
            }
        }
    }
    /// Left or Right: the candidate before or after, which becomes active.
    pub(super) fn step_candidate(&mut self, by: isize) {
        let Some(select) = self.compare.select else {
            return;
        };
        if let Some(next) = self.next_candidate(select, self.compare.candidate, by) {
            self.compare.candidate = Some(next);
            self.compare.active = Side::Candidate;
            self.sync_compare_selection();
        }
    }
    /// Up: the candidate becomes the select, beside the next candidate.
    pub(super) fn make_select(&mut self) {
        let Some(chosen) = self.compare.candidate else {
            return;
        };
        self.compare.select = Some(chosen);
        self.compare.candidate = self
            .next_candidate(chosen, None, 1)
            .or_else(|| self.next_candidate(chosen, None, -1));
        self.compare.active = Side::Candidate;
        self.sync_compare_selection();
    }
    /// Down: the select and candidate change places; the active photo stays.
    pub(super) fn swap_compare(&mut self) {
        if self.compare.candidate.is_none() {
            return;
        }
        let compare = &mut self.compare;
        std::mem::swap(&mut compare.select, &mut compare.candidate);
        compare.active = match compare.active {
            Side::Select => Side::Candidate,
            Side::Candidate => Side::Select,
        };
        self.sync_compare_selection();
    }
    fn activate(&mut self, side: Side) {
        if self.compare.id(side).is_some() {
            self.compare.active = side;
            self.sync_compare_selection();
        }
    }
    /// Selects the two photos, with the active one active, so the panels,
    /// the filmstrip and the keys follow Compare.
    fn sync_compare_selection(&mut self) {
        let active = self.compare.id(self.compare.active).or(self.compare.select);
        self.selection.selected = [self.compare.select, self.compare.candidate]
            .into_iter()
            .flatten()
            .collect();
        self.selection.active = active;
        self.selection.anchor = active;
    }
    /// A rating, flag or label key in Compare: the active photo only. With
    /// Shift, the candidate moves on.
    pub fn edit_compared(
        &mut self,
        edit: crate::app::photo_metadata::Edit,
        advance: bool,
    ) -> anyhow::Result<()> {
        let Some(id) = self.compare.id(self.compare.active) else {
            return Ok(());
        };
        self.edit_metadata(id, edit, false)?;
        if advance {
            self.step_candidate(1);
        }
        self.sync_compare_selection();
        Ok(())
    }
    pub(super) fn compare_keys(&mut self, presses: &[super::selection::Press]) {
        use egui::Key;
        for press in presses {
            if press.modifiers.command || press.modifiers.alt {
                continue;
            }
            match press.key {
                Key::ArrowLeft => self.step_candidate(-1),
                Key::ArrowRight => self.step_candidate(1),
                Key::ArrowUp if !press.repeat => self.make_select(),
                Key::ArrowDown if !press.repeat => self.swap_compare(),
                Key::Escape => self.close_compare(),
                Key::E | Key::Enter => self.open_loupe(),
                Key::B if !press.repeat => {
                    let ids = self.selection.active.into_iter().collect();
                    self.quick_key(press.modifiers, ids)
                }
                _ => {}
            }
        }
    }
    /// Compare in place of the grid: the two photos, a toolbar and the
    /// filmstrip.
    pub(super) fn compare(&mut self, ui: &mut egui::Ui) -> Action {
        // Removed since, e.g. by undo: the next photo stands in.
        if self
            .compare
            .candidate
            .is_some_and(|id| self.photo(id).is_none())
        {
            self.compare.candidate = None;
        }
        let Some(select) = self.compare.select.filter(|id| self.photo(*id).is_some()) else {
            self.close_compare();
            return Action::None;
        };
        if self.compare.candidate.is_none() {
            self.compare.candidate = self.next_candidate(select, None, 1);
        }
        self.sync_compare_selection();
        let mut target = None;
        egui::Panel::bottom("library-compare-filmstrip")
            .exact_size(128.)
            .frame(egui::Frame::new().fill(theme::gray(26)))
            .show(ui, |ui| {
                let active = self.selection.active.unwrap_or(select);
                target = self.filmstrip(ui, active).0;
            });
        match target {
            Some(Pick::Show(id)) if Some(id) == self.compare.select => self.activate(Side::Select),
            // Another photo becomes the candidate, as in Lightroom.
            Some(Pick::Show(id)) => {
                self.compare.candidate = Some(id);
                self.activate(Side::Candidate);
            }
            Some(Pick::Develop(id)) => return Action::Develop(id),
            None => {}
        }
        egui::Panel::bottom("library-compare-toolbar")
            .frame(
                egui::Frame::new()
                    .fill(theme::gray(38))
                    .inner_margin(egui::Margin::symmetric(10, 4)),
            )
            .show_separator_line(false)
            .show(ui, |ui| self.compare_toolbar(ui));
        let area = ui.available_rect_before_wrap();
        ui.allocate_rect(area, egui::Sense::hover());
        ui.painter().rect_filled(area, 0., theme::gray(36));
        let inner = area.shrink(MARGIN);
        let half = Vec2::new((inner.width() - MARGIN) / 2., inner.height());
        let panes = [
            (Side::Select, Rect::from_min_size(inner.min, half)),
            (
                Side::Candidate,
                Rect::from_min_size(inner.min + Vec2::new(half.x + MARGIN, 0.), half),
            ),
        ];
        for (side, rect) in panes {
            self.compare_pane(ui, side, rect);
        }
        Action::None
    }
    fn compare_toolbar(&mut self, ui: &mut egui::Ui) {
        let button = |ui: &mut egui::Ui, text: &str, hover: &str, enabled: bool| {
            ui.add_enabled(
                enabled,
                egui::Button::new(egui::RichText::new(text).size(11.)).small(),
            )
            .on_hover_text(hover)
            .clicked()
        };
        ui.horizontal(|ui| {
            self.view_buttons(ui);
            ui.add_space(12.);
            let two = self.compare.candidate.is_some();
            if button(ui, "Swap", "Swap the select and the candidate (Down)", two) {
                self.swap_compare();
            }
            if button(ui, "Make Select", "Make the candidate the select (Up)", two) {
                self.make_select();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if button(ui, "Done", "Back to the grid (Esc)", true) {
                    self.close_compare();
                }
                ui.label(filter_caption("Left / Right change the candidate"));
            });
        });
    }
    /// One photo of the two, fitted to `rect` with its caption below.
    fn compare_pane(&mut self, ui: &mut egui::Ui, side: Side, rect: Rect) {
        let id = egui::Id::new(("library-compare", side.name()));
        let response = ui.interact(rect, id, egui::Sense::click());
        let photo = self.compare.id(side).and_then(|id| self.photo(id)).cloned();
        let Some(photo) = photo else {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "No other photo to compare",
                egui::FontId::proportional(12.),
                theme::gray(120),
            );
            return;
        };
        if response.double_clicked() {
            self.activate(side);
            self.open_loupe();
            return;
        }
        if response.clicked() {
            self.activate(side);
        }
        let active = self.compare.active == side;
        let image_area = Rect::from_min_max(rect.min, rect.max - Vec2::new(0., CAPTION));
        let (texture, note) = self.compare_preview(ui.ctx(), &photo, image_area);
        if let Some(texture) = texture {
            let size = texture.size_vec2();
            let scale = (image_area.width() / size.x).min(image_area.height() / size.y);
            let at = Rect::from_center_size(image_area.center(), size * scale);
            let uv = Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.));
            ui.painter().image(texture.id(), at, uv, Color32::WHITE);
            if active {
                ui.painter().rect_stroke(
                    at.expand(3.),
                    1.,
                    egui::Stroke::new(1.5, theme::gray(225)),
                    egui::StrokeKind::Outside,
                );
            }
        }
        if let Some(note) = note {
            ui.painter().text(
                image_area.center_bottom() - Vec2::new(0., 8.),
                egui::Align2::CENTER_BOTTOM,
                note,
                egui::FontId::proportional(11.),
                theme::gray(170),
            );
        }
        let caption = Rect::from_min_max(rect.left_bottom() - Vec2::new(0., CAPTION), rect.max);
        caption_strip(ui.painter(), caption, side, &photo, active);
    }
    /// The photo's screen preview, or the grid's until it is ready, and a
    /// note on what is shown.
    fn compare_preview(
        &mut self,
        ctx: &egui::Context,
        photo: &Photo,
        area: Rect,
    ) -> (Option<egui::TextureHandle>, Option<String>) {
        self.request_previews(photo, ctx);
        let stand_in = self.texture(photo).cloned();
        if !self.is_available(&photo.path) {
            let note = match stand_in {
                Some(_) => "Offline: showing the cached preview",
                None => "Offline, and there is no cached preview",
            };
            return (stand_in, Some(note.into()));
        }
        let edge = (area.width().max(area.height()) * ctx.pixels_per_point()) as u32;
        let catalog = &self.catalog;
        match self
            .screen
            .get(photo, edge, || super::edit_source(catalog, photo.id))
        {
            Shown::Ready(texture) => (Some(texture.clone()), None),
            Shown::Loading => (stand_in, Some("Loading…".into())),
            Shown::Failed(error) => (stand_in, Some(format!("Preview unavailable: {error}"))),
        }
    }
}

/// "SELECT  DSCF0001.RAF  ★★★  ⚑  ■" under a photo.
fn caption_strip(painter: &egui::Painter, rect: Rect, side: Side, photo: &Photo, active: bool) {
    let y = rect.center().y;
    let role = painter.text(
        egui::pos2(rect.left(), y),
        egui::Align2::LEFT_CENTER,
        side.name().to_uppercase(),
        egui::FontId::proportional(10.5),
        theme::gray(if active { 200 } else { 125 }),
    );
    let name = painter.text(
        egui::pos2(role.right() + 10., y),
        egui::Align2::LEFT_CENTER,
        format!("{}{}", photo.filename, super::cell::copy_suffix(photo)),
        egui::FontId::proportional(12.),
        theme::gray(if active { 235 } else { 175 }),
    );
    let mut x = name.right() + 12.;
    if photo.rating > 0 {
        let stars = painter.text(
            egui::pos2(x, y),
            egui::Align2::LEFT_CENTER,
            "★".repeat(photo.rating.clamp(0, 5) as usize),
            egui::FontId::proportional(11.),
            theme::gray(210),
        );
        x = stars.right() + 10.;
    }
    if photo.flag != 0 {
        flag_icon(painter, egui::pos2(x + 6., y), photo.flag, true);
        x += 20.;
    }
    if let Some(color) = label_color(&photo.label) {
        let chip = Rect::from_center_size(egui::pos2(x + 5., y), Vec2::splat(10.));
        painter.rect_filled(chip, 1., color);
    }
}
