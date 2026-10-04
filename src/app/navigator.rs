//! Lightroom's Navigator and the zoom it controls, shared by Develop and the
//! Library. One `Zoom` is the view's zoom wherever the photo is shown, so
//! Develop, the Loupe and its JPEG/TIFF/PNG view all keep the same level and
//! place; the panel shows the whole photo with the part in view outlined.
use super::theme;
use super::widgets::{section, segmented};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};

/// Zoom levels the Navigator offers; 0 stands for Fit.
const LEVELS: [(f32, &str); 5] = [
    (0., "Fit"),
    (0.5, "50%"),
    (1., "100%"),
    (2., "200%"),
    (4., "400%"),
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Zoom {
    /// Zoomed in rather than fitted.
    pub on: bool,
    /// Screen pixels per image pixel when zoomed in (1 = 100%).
    pub level: f32,
    /// The centre of the view, as a fraction of the photo's width and height.
    pub pan: [f32; 2],
}
impl Default for Zoom {
    fn default() -> Self {
        Self {
            on: false,
            level: 1.,
            pan: [0.5, 0.5],
        }
    }
}
impl Zoom {
    /// Screen rectangle of a whole photo of `size` image pixels laid out in `area`:
    /// fitted in Fit, otherwise `level` screen pixels per image pixel around `pan`,
    /// centred when smaller than `area`. `ppp` is pixels per point.
    pub fn photo_rect(&self, area: egui::Rect, size: egui::Vec2, ppp: f32) -> egui::Rect {
        if !self.on {
            let k = (area.width() / size.x).min(area.height() / size.y);
            return egui::Rect::from_center_size(area.center(), size * k);
        }
        let size = size * (self.level / ppp);
        let place = |pan: f32, lo: f32, len: f32, size: f32| {
            if size <= len {
                lo + (len - size) / 2.
            } else {
                (lo + len / 2. - pan * size).clamp(lo + len - size, lo)
            }
        };
        let min = egui::Pos2::new(
            place(self.pan[0], area.left(), area.width(), size.x),
            place(self.pan[1], area.top(), area.height(), size.y),
        );
        egui::Rect::from_min_size(min, size)
    }
    /// The level shown, 0 for Fit.
    pub fn shown(&self) -> f32 {
        if self.on { self.level } else { 0. }
    }
    /// Sets a level; 0 fits.
    pub fn set(&mut self, level: f32) {
        self.on = level > 0.;
        if self.on {
            self.level = level;
        }
    }
}

/// A photo's texture and its size.
pub(super) type Photo = (egui::TextureId, Vec2);

/// What a click in the Navigator asked for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Change {
    /// A zoom level from the buttons; 0 fits.
    Level(f32),
    /// Zoom in on this point of the photo.
    Inspect([f32; 2]),
}

/// The Navigator panel: `photo` is the whole photo's texture and size,
/// `shown` the part in view as fractions (x, y, width, height). Without a
/// `zoom` it is only the photo, as in the Library's grid.
pub(super) fn navigator(
    ui: &mut egui::Ui,
    photo: Option<Photo>,
    zoom: Option<Zoom>,
    shown: Option<[f32; 4]>,
) -> Option<Change> {
    let mut change = None;
    ui.spacing_mut().item_spacing.y = 0.;
    section(ui, "Navigator", false, |ui| {
        if let Some(zoom) = zoom {
            let mut level = zoom.shown();
            let w = ui.available_width();
            if segmented(ui, &mut level, &LEVELS, w) {
                change = Some(Change::Level(level));
            }
            ui.add_space(6.);
        }
        let sense = if zoom.is_some() {
            Sense::click_and_drag()
        } else {
            Sense::hover()
        };
        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), ui.available_width() * 0.66),
            sense,
        );
        ui.painter().rect_filled(rect, 0., theme::photo_backdrop());
        let Some((texture, size)) = photo else {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "No photo selected",
                egui::FontId::proportional(11.),
                theme::gray(95),
            );
            return;
        };
        let scale = (rect.width() / size.x).min(rect.height() / size.y);
        let image = Rect::from_center_size(rect.center(), size * scale);
        ui.painter().image(
            texture,
            image,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
            Color32::WHITE,
        );
        if let Some([x, y, w, h]) = shown {
            let at = |fx: f32, fy: f32| {
                Pos2::new(
                    image.left() + fx * image.width(),
                    image.top() + fy * image.height(),
                )
            };
            ui.painter().rect_stroke(
                Rect::from_min_max(at(x, y), at(x + w, y + h)),
                0.,
                Stroke::new(1.5, Color32::WHITE),
                egui::StrokeKind::Outside,
            );
        }
        if zoom.is_none() {
            return;
        }
        if (response.clicked() || response.dragged())
            && let Some(p) = response.interact_pointer_pos()
        {
            change = Some(Change::Inspect([
                ((p.x - image.left()) / image.width()).clamp(0., 1.),
                ((p.y - image.top()) / image.height()).clamp(0., 1.),
            ]));
        }
        response
            .on_hover_cursor(egui::CursorIcon::Crosshair)
            .on_hover_text("Click or drag to inspect that area");
    });
    change
}
