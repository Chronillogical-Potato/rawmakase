//! Red Eye Correction (Lightroom's tool between Remove and Masking): drag from the
//! centre of an eye outward, or click to use the last size; the red pupil found inside
//! that circle gets a correction. Drag a correction to move it; Pupil Size and Darken
//! change the selected one; Delete removes it.
use super::Editor;
use super::retouch_tool::{hint, indented};
use super::theme;
use super::widgets::slider_with;
use crate::develop::{
    ViewMapping,
    red_eye::{self, EyeKind, RedEyeOp},
    retouch::radii,
};
use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};

pub(super) struct RedEyeTool {
    /// Index into the recipe's red eye corrections.
    pub(super) selected: Option<usize>,
    /// The last circle's radius, as a fraction of the long edge, for clicks.
    pub(super) size: f32,
    drag: Drag,
}
impl Default for RedEyeTool {
    fn default() -> Self {
        Self {
            selected: None,
            size: 0.02,
            drag: Drag::None,
        }
    }
}
#[derive(Default)]
enum Drag {
    #[default]
    None,
    /// Drawing the search circle out from its centre (image space).
    Circle([f32; 2]),
    /// Moving a correction; image-space pointer position at the start and the
    /// correction then.
    Move([f32; 2], RedEyeOp),
}
impl RedEyeTool {
    pub(super) fn clear_document(&mut self) {
        self.selected = None;
        self.drag = Drag::None;
    }
}
/// The distance from `a` to `b` (image space) as a fraction of the long edge.
fn long_edge_distance(a: [f32; 2], b: [f32; 2], aspect: f32) -> f32 {
    let (sx, sy) = radii(1., aspect);
    ((b[0] - a[0]) / sx).hypot((b[1] - a[1]) / sy)
}

impl Editor {
    /// Handles the pointer on the photo and draws the corrections. The tool owns the
    /// pointer.
    pub(super) fn red_eye_overlay(
        &mut self,
        ui: &mut egui::Ui,
        response: &egui::Response,
        rect: Rect,
    ) {
        let Some(im) = self.document.full().cloned() else {
            return;
        };
        let recipe = self.effective_recipe();
        let map = ViewMapping::new(&im, &recipe);
        let aspect = map.frame().aspect();
        let to_screen = |p: [f32; 2]| {
            let [u, v] = map.to_view(p);
            Pos2::new(
                rect.left() + u * rect.width(),
                rect.top() + v * rect.height(),
            )
        };
        let to_image = |p: Pos2| {
            map.to_image(
                (p.x - rect.left()) / rect.width(),
                (p.y - rect.top()) / rect.height(),
            )
        };
        let ops = self.document.recipe.red_eye.clone();
        let hit = |pos: Pos2| {
            let at = to_image(pos);
            (0..ops.len()).rev().find(|i| ops[*i].contains(at, aspect))
        };
        let pointer = response.hover_pos();
        let hovered = pointer.and_then(hit);

        if response.drag_started()
            && let Some(origin) = ui.input(|i| i.pointer.press_origin())
        {
            let at = to_image(origin);
            self.view.red_eye.drag = match hit(origin) {
                Some(i) => {
                    self.view.red_eye.selected = Some(i);
                    Drag::Move(at, ops[i].clone())
                }
                None => Drag::Circle(at),
            };
        }
        if response.dragged()
            && let Some(pos) = response.interact_pointer_pos()
            && let Drag::Move(start, original) = &self.view.red_eye.drag
            && let Some(i) = self.view.red_eye.selected
            && i < self.document.recipe.red_eye.len()
        {
            let at = to_image(pos);
            let mut op = original.clone();
            op.translate([at[0] - start[0], at[1] - start[1]]);
            self.document.recipe.red_eye[i] = op;
            self.show_red_eye();
        }
        if response.drag_stopped() {
            if let Drag::Circle(center) = std::mem::take(&mut self.view.red_eye.drag)
                && let Some(pos) = response.interact_pointer_pos()
            {
                let size = long_edge_distance(center, to_image(pos), aspect);
                if size > 0.002 {
                    self.view.red_eye.size = size.min(red_eye::MAX_RADIUS);
                    self.add_red_eye(center, size);
                }
            }
            self.view.red_eye.drag = Drag::None;
        }
        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            match hit(pos) {
                Some(i) => self.view.red_eye.selected = Some(i),
                None => {
                    let size = self.view.red_eye.size;
                    self.add_red_eye(to_image(pos), size);
                }
            }
        }

        // Drawing.
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        let tool = &self.view.red_eye;
        for (i, op) in self.document.recipe.red_eye.iter().enumerate() {
            let selected = tool.selected == Some(i);
            let points: Vec<Pos2> = op.outline(aspect, 72).into_iter().map(to_screen).collect();
            let width = if selected || hovered == Some(i) {
                1.5
            } else {
                1.
            };
            outline(&painter, points, width);
            if selected {
                crosshair(&painter, to_screen(op.center));
            }
        }
        if let Drag::Circle(center) = &tool.drag
            && let Some(pos) = pointer
        {
            let size = long_edge_distance(*center, to_image(pos), aspect);
            let circle = circle_points(*center, size, aspect);
            outline(&painter, circle.into_iter().map(to_screen).collect(), 1.);
            crosshair(&painter, to_screen(*center));
        } else if let Some(pos) = pointer
            && hovered.is_none()
            && rect.contains(pos)
        {
            let circle = circle_points(to_image(pos), tool.size, aspect);
            outline(&painter, circle.into_iter().map(to_screen).collect(), 1.);
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        } else if hovered.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
        }
    }
    /// Corrects the red pupil found within `size` (long-edge fraction) of image
    /// position `center`, or says none was found.
    pub(super) fn add_red_eye(&mut self, center: [f32; 2], size: f32) {
        if self.document.recipe.red_eye.len() >= red_eye::MAX_OPS {
            self.status = "Too many red eye corrections on this photo".into();
            return;
        }
        let Some(im) = self.document.full() else {
            return;
        };
        match red_eye::find_pupil(im, center, size.min(red_eye::MAX_RADIUS)) {
            Ok(pupil) => {
                let (pupil_size, darken) = self
                    .view
                    .red_eye
                    .selected
                    .and_then(|i| self.document.recipe.red_eye.get(i))
                    .map_or(
                        (red_eye::DEFAULT_PUPIL_SIZE, red_eye::DEFAULT_DARKEN),
                        |op| (op.pupil_size, op.darken),
                    );
                let op = RedEyeOp {
                    kind: EyeKind::Red,
                    center: pupil.center,
                    radius: pupil.radius,
                    correlation: pupil.correlation,
                    pupil_size,
                    darken,
                };
                if let Err(e) = op.validate() {
                    self.status = e.to_string();
                    return;
                }
                self.document.recipe.red_eye.push(op);
                self.show_red_eye();
                self.view.red_eye.selected = Some(self.document.recipe.red_eye.len() - 1);
            }
            Err(e) => self.status = e.to_string(),
        }
    }
    /// Turns the Red Eye switch on, so a correction just made or changed shows, as
    /// Lightroom does.
    fn show_red_eye(&mut self) {
        use crate::develop::panels::{Panel, PanelState};
        self.document
            .recipe
            .panels
            .set(Panel::RedEye, PanelState::On);
    }
    /// The Red Eye tool's keys: Delete removes the selected correction.
    pub(super) fn red_eye_keys(&mut self, i: &egui::InputState) {
        if i.modifiers.command || i.modifiers.alt {
            return;
        }
        if i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace) {
            self.delete_red_eye();
        }
    }
    pub(super) fn delete_red_eye(&mut self) {
        if let Some(i) = self.view.red_eye.selected.take()
            && i < self.document.recipe.red_eye.len()
        {
            self.document.recipe.red_eye.remove(i);
        }
    }
    /// The Red Eye drawer below the tool strip.
    pub(super) fn red_eye_panel(&mut self, ui: &mut egui::Ui) {
        super::widgets::set_edit_context(ui, "Red Eye");
        let selected = self
            .view
            .red_eye
            .selected
            .filter(|i| *i < self.document.recipe.red_eye.len());
        super::retouch_tool::control_label(ui, "Type", |ui| {
            ui.label(
                egui::RichText::new("Red Eye")
                    .size(12.)
                    .color(theme::gray(220)),
            )
            .on_hover_text("Pet Eye is not supported yet");
        });
        match selected {
            Some(i) => {
                let op = &mut self.document.recipe.red_eye[i];
                let before = (op.pupil_size, op.darken);
                slider_with(
                    ui,
                    "Pupil Size",
                    &mut op.pupil_size,
                    0. ..=1.,
                    red_eye::DEFAULT_PUPIL_SIZE,
                    Some((100., 0)),
                    None,
                );
                slider_with(
                    ui,
                    "Darken",
                    &mut op.darken,
                    0. ..=1.,
                    red_eye::DEFAULT_DARKEN,
                    Some((100., 0)),
                    None,
                );
                if (op.pupil_size, op.darken) != before {
                    self.show_red_eye();
                    super::widgets::name_history_step(
                        ui,
                        "Update Red Eye Correction".into(),
                        String::new(),
                    );
                }
            }
            None => hint(
                ui,
                "Select a correction to change its Pupil Size and Darken",
            ),
        }
        ui.add_space(4.);
        hint(
            ui,
            "Drag from the center of the eye or click to use current size",
        );
        hint(ui, "Delete removes the selected correction");
        ui.add_space(4.);
        indented(ui, |ui| {
            let w = (ui.available_width() - 4.) / 2.;
            let any = !self.document.recipe.red_eye.is_empty();
            if ui
                .add_enabled(any, egui::Button::new("Reset").min_size(Vec2::new(w, 22.)))
                .on_hover_text("Remove every red eye correction")
                .clicked()
            {
                self.document.recipe.red_eye.clear();
                self.view.red_eye.selected = None;
            }
            if ui
                .add_sized(
                    [w, 22.],
                    egui::Button::new(egui::RichText::new("Close").color(theme::on_accent()))
                        .fill(theme::accent()),
                )
                .on_hover_text("Close the tool")
                .clicked()
            {
                self.view.tool = super::state::Tool::None;
            }
        });
    }
}
/// A circle of `radius` (long-edge fraction) around image position `center`.
fn circle_points(center: [f32; 2], radius: f32, aspect: f32) -> Vec<[f32; 2]> {
    RedEyeOp {
        kind: EyeKind::Red,
        center,
        radius: [radius; 2],
        correlation: 0.,
        pupil_size: 0.,
        darken: 0.,
    }
    .outline(aspect, 72)
}
fn outline(painter: &egui::Painter, points: Vec<Pos2>, width: f32) {
    painter.add(egui::Shape::closed_line(
        points.clone(),
        Stroke::new(width + 1.5, Color32::from_black_alpha(110)),
    ));
    painter.add(egui::Shape::closed_line(
        points,
        Stroke::new(width, Color32::from_white_alpha(230)),
    ));
}
fn crosshair(painter: &egui::Painter, at: Pos2) {
    for (a, b) in [
        (Vec2::new(-5., 0.), Vec2::new(5., 0.)),
        (Vec2::new(0., -5.), Vec2::new(0., 5.)),
    ] {
        painter.line_segment(
            [at + a, at + b],
            Stroke::new(2.5, Color32::from_black_alpha(110)),
        );
        painter.line_segment([at + a, at + b], Stroke::new(1., Color32::WHITE));
    }
}
