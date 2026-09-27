use super::Editor;
use super::state::TextureMode;
use super::widgets::{section, segmented};
use crate::app::theme;
use crate::develop::{self, Geometry};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};

impl Editor {
    /// Constrains the crop to the chosen aspect: negative keeps the photo's
    /// own ratio, zero is free, and presets follow the photo's orientation.
    pub(super) fn fit_aspect(&mut self) {
        if self.view.aspect == 0. {
            return;
        }
        if let Some(im) = self.document.full().cloned() {
            let mut r = self.document.recipe.clone();
            r.crop = [0., 0., 1., 1.];
            let g = Geometry::new(&im, &r, 0);
            let portrait = g.oriented_height > g.oriented_width;
            let aspect = if self.view.aspect < 0. {
                g.oriented_width / g.oriented_height
            } else if portrait {
                1. / self.view.aspect
            } else {
                self.view.aspect
            };
            let ratio = aspect * g.oriented_height / g.oriented_width;
            let c = &mut self.document.recipe.crop;
            let cx = (c[0] + c[2]) / 2.;
            let cy = (c[1] + c[3]) / 2.;
            let mut w = c[2] - c[0];
            let mut h = c[3] - c[1];
            if w / h > ratio {
                w = h * ratio;
            } else {
                h = w / ratio;
            }
            let w = w.max(0.01);
            let h = h.max(0.01);
            *c = [cx - w / 2., cy - h / 2., cx + w / 2., cy + h / 2.];
        }
    }
    /// Lightroom's Navigator: the whole photo with the zoomed area outlined.
    /// Clicking or dragging in it moves the 100% view there.
    pub(super) fn navigator_ui(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 0.;
        section(ui, "Navigator", false, |ui| {
            // 0 stands for Fit.
            let mut zoom = if self.view.zoom100 {
                self.view.zoom_level
            } else {
                0.
            };
            let w = ui.available_width();
            if segmented(
                ui,
                &mut zoom,
                &[
                    (0., "Fit"),
                    (0.5, "50%"),
                    (1., "100%"),
                    (2., "200%"),
                    (4., "400%"),
                ],
                w,
            ) {
                self.set_zoom(zoom);
            }
            ui.add_space(6.);
            let (rect, response) = ui.allocate_exact_size(
                Vec2::new(ui.available_width(), ui.available_width() * 0.66),
                Sense::click_and_drag(),
            );
            ui.painter().rect_filled(rect, 0., theme::photo_backdrop());
            let Some(texture) = &self.preview.navigator else {
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "No photo open",
                    egui::FontId::proportional(11.),
                    theme::gray(95),
                );
                return;
            };
            let size = texture.size_vec2();
            let scale = (rect.width() / size.x).min(rect.height() / size.y);
            let image = Rect::from_center_size(rect.center(), size * scale);
            ui.painter().image(
                texture.id(),
                image,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
                Color32::WHITE,
            );
            if let (Some([x, y, w, h]), Some(im)) = (self.region(), self.document.full()) {
                let g = Geometry::new(im, &self.effective_recipe(), 0);
                let to = |px: u32, py: u32| {
                    Pos2::new(
                        image.left() + px as f32 / g.width as f32 * image.width(),
                        image.top() + py as f32 / g.height as f32 * image.height(),
                    )
                };
                ui.painter().rect_stroke(
                    Rect::from_min_max(to(x, y), to(x + w, y + h)),
                    0.,
                    Stroke::new(1.5, Color32::WHITE),
                    egui::StrokeKind::Outside,
                );
            }
            if (response.clicked() || response.dragged())
                && let Some(p) = response.interact_pointer_pos()
            {
                self.view.pan = [
                    ((p.x - image.left()) / image.width()).clamp(0., 1.),
                    ((p.y - image.top()) / image.height()).clamp(0., 1.),
                ];
                self.view.zoom100 = true;
                self.view.crop_mode = false;
            }
            response
                .on_hover_cursor(egui::CursorIcon::Crosshair)
                .on_hover_text("Click or drag to inspect that area at 100%");
        });
    }
    /// Before the first image arrives: the Library preview of the photo being
    /// opened with a spinner, or a hint when nothing is open.
    fn loading_placeholder(&self, ui: &mut egui::Ui, area: Rect) {
        // A photo counts as opening until its first render arrives, even after
        // the decode has finished; only an empty document shows the hint.
        let opening = self.load.is_running()
            || self.document.catalog_photo.is_some()
            || self.document.path.is_some();
        if !opening {
            ui.painter().text(
                area.center(),
                egui::Align2::CENTER_CENTER,
                "Open a RAW photo, or pick one in the Library",
                egui::FontId::proportional(15.),
                theme::gray(120),
            );
            return;
        }
        let path = self
            .document
            .catalog_photo
            .and_then(|id| self.library.as_ref()?.photo(id))
            .map(|p| p.path.clone());
        let thumb = path
            .as_ref()
            .and_then(|p| self.library.as_ref()?.thumbnail(p));
        if let Some(texture) = thumb {
            let size = texture.size_vec2();
            let k = (area.width() / size.x).min(area.height() / size.y);
            ui.painter().image(
                texture.id(),
                Rect::from_center_size(area.center(), size * k),
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
                Color32::WHITE,
            );
        }
        let badge = Rect::from_center_size(
            Pos2::new(area.center().x, area.bottom() - 36.),
            Vec2::new(170., 30.),
        );
        ui.painter()
            .rect_filled(badge, 15., Color32::from_black_alpha(170));
        let spinner =
            Rect::from_center_size(badge.left_center() + Vec2::new(20., 0.), Vec2::splat(14.));
        ui.put(spinner, egui::Spinner::new().size(14.));
        ui.painter().text(
            badge.left_center() + Vec2::new(36., 0.),
            egui::Align2::LEFT_CENTER,
            "Loading photo…",
            egui::FontId::proportional(12.),
            theme::gray(225),
        );
    }
    /// Sets a zoom level; 0 means Fit.
    pub(super) fn set_zoom(&mut self, level: f32) {
        if level <= 0. {
            self.view.zoom100 = false;
        } else {
            self.view.zoom100 = true;
            self.view.zoom_level = level;
            self.view.crop_mode = false;
        }
        self.schedule();
    }
    /// Cmd/Ctrl + and −: step through Lightroom-like zoom levels, with Fit
    /// below the first level larger than the fitted size.
    pub(super) fn step_zoom(&mut self, direction: i32) {
        const LEVELS: [f32; 6] = [0.25, 0.5, 1., 2., 3., 4.];
        let fit = self.document.full().map_or(0., |im| {
            let g = Geometry::new(im, &self.effective_recipe(), 0);
            (self.view.viewport.x / g.width as f32).min(self.view.viewport.y / g.height as f32)
        });
        let current = if self.view.zoom100 {
            self.view.zoom_level
        } else {
            fit
        };
        let next = if direction > 0 {
            LEVELS.iter().copied().find(|l| *l > current + 1e-3)
        } else {
            LEVELS
                .iter()
                .rev()
                .copied()
                .find(|l| *l < current - 1e-3 && *l > fit + 1e-3)
                .or(Some(0.))
        };
        if let Some(level) = next {
            self.set_zoom(level);
        }
    }
    /// Screen rectangle of the whole photo for the current zoom: fitted in Fit,
    /// otherwise `zoom_level` screen pixels per image pixel around `pan`,
    /// centered when smaller than the viewport.
    fn photo_rect(&self, area: Rect, g: &Geometry, ppp: f32) -> Rect {
        let (w, h) = (g.width as f32, g.height as f32);
        if !self.view.zoom100 {
            let k = (area.width() / w).min(area.height() / h);
            return Rect::from_center_size(area.center(), Vec2::new(w, h) * k);
        }
        let size = Vec2::new(w, h) * (self.view.zoom_level / ppp);
        let place = |pan: f32, lo: f32, len: f32, size: f32| {
            if size <= len {
                lo + (len - size) / 2.
            } else {
                (lo + len / 2. - pan * size).clamp(lo + len - size, lo)
            }
        };
        let min = Pos2::new(
            place(self.view.pan[0], area.left(), area.width(), size.x),
            place(self.view.pan[1], area.top(), area.height(), size.y),
        );
        Rect::from_min_size(min, size)
    }
    pub(super) fn viewport_ui(&mut self, ui: &mut egui::Ui) {
        let available = ui.available_size();
        let (area, response) = ui.allocate_exact_size(available, Sense::click_and_drag());
        ui.painter().rect_filled(area, 0., theme::photo_backdrop());
        let ppp = ui.ctx().pixels_per_point();
        self.view.viewport = available * ppp;
        if !self.view.zoom100
            && let Some(im) = self.document.full().cloned()
        {
            let g = Geometry::new(&im, &self.effective_recipe(), 0);
            let edge = crate::develop::quality::fit_edge(
                g.width,
                g.height,
                [self.view.viewport.x as u32, self.view.viewport.y as u32],
            );
            if edge != self.preview.last_fit_edge {
                self.schedule();
            }
        }
        let region_texture = match self.preview.mode {
            TextureMode::Region(_) => self.preview.region.clone(),
            TextureMode::Whole => None,
        };
        let Some(texture) = self.preview.texture.clone().or(region_texture.clone()) else {
            self.loading_placeholder(ui, area);
            return;
        };
        let geometry = self
            .document
            .full()
            .map(|im| Geometry::new(im, &self.effective_recipe(), 0));
        // `rect` is where the whole (cropped) photo sits on screen. The last whole-photo
        // render always fills it, and a 100% region is drawn over its part of it, so
        // zooming and panning scale images that are already there instead of showing
        // a stale region alone until the next render lands.
        let now = ui.input(|i| i.time);
        let target = match &geometry {
            Some(g) => self.photo_rect(area, g, ppp),
            None => {
                let size = texture.size_vec2();
                let k = (available.x / size.x).min(available.y / size.y);
                Rect::from_center_size(area.center(), size * k)
            }
        };
        let key = (self.view.zoom100, self.view.zoom_level);
        if key != self.view.zoom_key {
            self.view.zoom_key = key;
            self.schedule();
            if let Some(from) = self.view.shown_rect {
                self.view.zoom_anim = Some((now, from));
            }
        }
        let rect = match self.view.zoom_anim {
            Some((start, from)) => {
                let t = ((now - start) / 0.22).clamp(0., 1.) as f32;
                let ease = 1. - (1. - t).powi(3);
                if t >= 1. {
                    self.view.zoom_anim = None;
                } else {
                    ui.ctx().request_repaint();
                }
                Rect::from_min_max(
                    from.min.lerp(target.min, ease),
                    from.max.lerp(target.max, ease),
                )
            }
            None => target,
        };
        self.view.shown_rect = Some(rect);
        let region_rect = match (self.preview.mode, &geometry) {
            (TextureMode::Region([x, y, w, h]), Some(g)) => {
                let at = |px: u32, py: u32| {
                    Pos2::new(
                        rect.left() + px as f32 / g.width as f32 * rect.width(),
                        rect.top() + py as f32 / g.height as f32 * rect.height(),
                    )
                };
                Some(Rect::from_min_max(at(x, y), at(x + w, y + h)))
            }
            _ => None,
        };
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.));
        let painter = ui.painter().with_clip_rect(area);
        if self.preview.texture.is_some() {
            painter.image(texture.id(), rect, uv, Color32::WHITE);
        }
        if let (Some(region), Some(at)) = (&region_texture, region_rect) {
            painter.image(region.id(), at, uv, Color32::WHITE);
        }
        if self.view.compare {
            let badge =
                Rect::from_min_size(area.left_top() + Vec2::splat(12.), Vec2::new(62., 25.));
            ui.painter()
                .rect_filled(badge, 3., Color32::from_black_alpha(190));
            ui.painter().text(
                badge.center(),
                egui::Align2::CENTER_CENTER,
                "Before",
                egui::FontId::proportional(12.),
                Color32::WHITE,
            );
        }
        if self.view.zoom100 && !self.view.picker && response.dragged() {
            let delta = ui.input(|i| i.pointer.delta());
            self.view.pan[0] = (self.view.pan[0] - delta.x / rect.width()).clamp(0., 1.);
            self.view.pan[1] = (self.view.pan[1] - delta.y / rect.height()).clamp(0., 1.);
        }
        if response.hovered() && !self.view.crop_mode && !self.view.picker {
            ui.ctx().set_cursor_icon(if self.view.zoom100 {
                egui::CursorIcon::Grab
            } else {
                egui::CursorIcon::ZoomIn
            });
            if response.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            }
        }
        // Lightroom: a click zooms in keeping the clicked point under the
        // pointer; the next click returns to Fit.
        if response.clicked()
            && !self.view.crop_mode
            && !self.view.picker
            && let Some(pos) = response.interact_pointer_pos()
            && rect.contains(pos)
        {
            if !self.view.zoom100
                && let Some(g) = &geometry
            {
                let point = (pos - rect.min) / rect.size();
                let k = self.view.zoom_level / ppp;
                let size = Vec2::new(g.width as f32 * k, g.height as f32 * k);
                let origin = pos - point * size;
                let pan = (area.center() - origin) / size;
                self.view.pan = [pan.x.clamp(0., 1.), pan.y.clamp(0., 1.)];
            }
            self.view.zoom100 = !self.view.zoom100;
            self.schedule();
        }
        if self.view.picker
            && response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
            && rect.contains(pos)
            && let Some(im) = self.document.full().cloned()
        {
            let u = (pos.x - rect.left()) / rect.width();
            let v = (pos.y - rect.top()) / rect.height();
            self.document.recipe.wb = develop::neutral_pick(&im, &self.document.recipe, u, v);
            self.document
                .recipe
                .sync_white_balance_controls(&im.metadata);
            self.view.picker = false;
        }
        if self.view.crop_mode && !self.view.zoom100 && self.document.full().is_some() {
            let c = self.document.recipe.crop;
            let cr = Rect::from_min_max(
                Pos2::new(
                    rect.left() + c[0] * rect.width(),
                    rect.top() + c[1] * rect.height(),
                ),
                Pos2::new(
                    rect.left() + c[2] * rect.width(),
                    rect.top() + c[3] * rect.height(),
                ),
            );
            for shade in [
                Rect::from_min_max(rect.min, Pos2::new(rect.right(), cr.top())),
                Rect::from_min_max(Pos2::new(rect.left(), cr.bottom()), rect.max),
                Rect::from_min_max(
                    Pos2::new(rect.left(), cr.top()),
                    Pos2::new(cr.left(), cr.bottom()),
                ),
                Rect::from_min_max(
                    Pos2::new(cr.right(), cr.top()),
                    Pos2::new(rect.right(), cr.bottom()),
                ),
            ] {
                ui.painter()
                    .rect_filled(shade, 0., Color32::from_black_alpha(140));
            }
            ui.painter().rect_stroke(
                cr,
                0.,
                Stroke::new(1., Color32::WHITE),
                egui::StrokeKind::Inside,
            );
            for t in [1. / 3., 2. / 3.] {
                ui.painter().line_segment(
                    [
                        Pos2::new(cr.left() + cr.width() * t, cr.top()),
                        Pos2::new(cr.left() + cr.width() * t, cr.bottom()),
                    ],
                    Stroke::new(1., Color32::from_white_alpha(90)),
                );
                ui.painter().line_segment(
                    [
                        Pos2::new(cr.left(), cr.top() + cr.height() * t),
                        Pos2::new(cr.right(), cr.top() + cr.height() * t),
                    ],
                    Stroke::new(1., Color32::from_white_alpha(90)),
                );
            }
            let handles = [
                cr.left_top(),
                cr.right_top(),
                cr.right_bottom(),
                cr.left_bottom(),
                cr.left_center(),
                cr.right_center(),
                cr.center_top(),
                cr.center_bottom(),
            ];
            for p in handles {
                ui.painter().rect_filled(
                    Rect::from_center_size(p, Vec2::splat(8.)),
                    1.,
                    Color32::WHITE,
                );
            }
            if response.drag_started()
                && let Some(p) = response.interact_pointer_pos()
            {
                let nearest = handles
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| a.distance(p).total_cmp(&b.distance(p)))
                    .map(|(i, q)| (i, q.distance(p)));
                let handle = nearest
                    .filter(|(_, d)| *d < 25.)
                    .map(|(i, _)| i)
                    .unwrap_or(8);
                self.view.crop_drag = Some((c, handle));
            }
            if response.dragged()
                && let Some((start, handle)) = self.view.crop_drag
            {
                let delta = response.total_drag_delta().unwrap_or_default();
                let dx = delta.x / rect.width();
                let dy = delta.y / rect.height();
                let mut c = start;
                match handle {
                    0 => {
                        c[0] += dx;
                        c[1] += dy;
                    }
                    1 => {
                        c[2] += dx;
                        c[1] += dy;
                    }
                    2 => {
                        c[2] += dx;
                        c[3] += dy;
                    }
                    3 => {
                        c[0] += dx;
                        c[3] += dy;
                    }
                    4 => c[0] += dx,
                    5 => c[2] += dx,
                    6 => c[1] += dy,
                    7 => c[3] += dy,
                    _ => {
                        let dx = dx.clamp(-c[0], 1. - c[2]);
                        let dy = dy.clamp(-c[1], 1. - c[3]);
                        c[0] += dx;
                        c[2] += dx;
                        c[1] += dy;
                        c[3] += dy;
                    }
                }
                c[0] = c[0].clamp(0., start[2] - 0.01);
                c[1] = c[1].clamp(0., start[3] - 0.01);
                c[2] = c[2].clamp(c[0] + 0.01, 1.);
                c[3] = c[3].clamp(c[1] + 0.01, 1.);
                self.document.recipe.crop = c;
                self.fit_aspect();
            }
            if response.drag_stopped() {
                self.view.crop_drag = None;
            }
        }
        if self.view.zoom100 && self.region() != self.preview.last_region {
            self.schedule();
        }
    }
}
