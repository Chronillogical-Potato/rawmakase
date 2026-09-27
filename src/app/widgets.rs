use crate::app::icons::{self, Icon};
use crate::app::theme;
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};

pub(super) fn toolbar_divider(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(12., 32.), Sense::hover());
    ui.painter().line_segment(
        [
            rect.center_top() + Vec2::new(0., 6.),
            rect.center_bottom() - Vec2::new(0., 6.),
        ],
        Stroke::new(1., theme::gray(53)),
    );
}
pub(super) fn toolbar_action(
    ui: &mut egui::Ui,
    label: &str,
    width: f32,
    selected: bool,
    enabled: bool,
    icon: u8,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(width, 32.),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let hover = enabled && response.hovered();
    let fill = if icon == 4 {
        theme::gray(if hover { 225 } else { 200 })
    } else {
        theme::gray(if selected {
            62
        } else if hover {
            48
        } else {
            29
        })
    };
    ui.painter().rect_filled(rect, 5., fill);
    let color = theme::gray(if !enabled {
        85
    } else if icon == 4 {
        25
    } else if selected || hover {
        235
    } else {
        175
    });
    if !label.is_empty() {
        ui.painter().text(
            rect.center() + Vec2::new(if icon > 0 { 8. } else { 0. }, 0.),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(12.),
            color,
        );
    }
    let c = if label.is_empty() {
        rect.center()
    } else {
        Pos2::new(rect.left() + 13., rect.center().y)
    };
    let glyph = match icon {
        1 => Some(Icon::Undo),
        2 => Some(Icon::Redo),
        3 => Some(Icon::BeforeAfter),
        4 => Some(Icon::Export),
        _ => None,
    };
    if let Some(glyph) = glyph {
        icons::paint_at(ui.painter(), glyph, c, 15., color);
    }
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}
pub(super) fn adjustment_section(
    ui: &mut egui::Ui,
    title: &str,
    contents: impl FnOnce(&mut egui::Ui),
) -> bool {
    section(ui, title, true, contents)
}
/// Where the set of collapsed section titles lives in egui memory; the editor
/// seeds it from the session and saves it back when it changes.
pub(super) fn collapsed_sections_id() -> egui::Id {
    egui::Id::new("rawmakase-collapsed-sections")
}
/// Where a control names the edit it just made, for the History panel. The
/// editor takes it at the end of the frame.
pub(super) fn history_step_id() -> egui::Id {
    egui::Id::new("rawmakase-history-step")
}
pub(super) fn name_history_step(ui: &egui::Ui, name: String, value: String) {
    ui.ctx()
        .data_mut(|d| d.insert_temp(history_step_id(), (name, value)));
}
/// The panel or sub-panel being drawn ("Detail", then "Sharpening"), so a
/// slider's step reads "Sharpening Amount" rather than "Amount".
pub(super) fn set_edit_context(ui: &egui::Ui, title: &str) {
    ui.ctx()
        .data_mut(|d| d.insert_temp(egui::Id::new("rawmakase-edit-context"), title.to_string()));
}
fn edit_context(ui: &egui::Ui) -> String {
    ui.ctx()
        .data(|d| d.get_temp(egui::Id::new("rawmakase-edit-context")))
        .unwrap_or_default()
}
/// Collapsible Lightroom-style panel header; returns whether reset was clicked.
pub(super) fn section(
    ui: &mut egui::Ui,
    title: &str,
    resettable: bool,
    contents: impl FnOnce(&mut egui::Ui),
) -> bool {
    let id = ui.make_persistent_id(("adjustment-section-v3", title));
    let mut open = !ui.ctx().data(|d| {
        d.get_temp::<std::collections::BTreeSet<String>>(collapsed_sections_id())
            .is_some_and(|set| set.contains(title))
    });
    ui.add_space(8.);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.), Sense::hover());
    let reset_rect = Rect::from_center_size(
        Pos2::new(rect.right() - 16., rect.center().y),
        Vec2::splat(24.),
    );
    let toggle_right = if resettable {
        reset_rect.left()
    } else {
        rect.right()
    };
    let toggle_rect = Rect::from_min_max(rect.min, Pos2::new(toggle_right, rect.bottom()));
    let toggle = ui
        .interact(toggle_rect, id.with("toggle"), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let reset = ui.interact(
        reset_rect,
        id.with("reset"),
        if resettable {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let reset = if resettable {
        reset.on_hover_text(format!("Reset {title}"))
    } else {
        reset
    };
    // A filled header band marks each collapsible panel, as in Lightroom.
    ui.painter().rect_filled(
        rect,
        3.,
        theme::gray(if toggle.hovered() { 60 } else { 51 }),
    );
    ui.painter().rect_stroke(
        rect,
        3.,
        Stroke::new(1., theme::gray(if open { 70 } else { 62 })),
        egui::StrokeKind::Inside,
    );
    let c = Pos2::new(rect.left() + 13., rect.center().y);
    let triangle = if open {
        vec![
            c + Vec2::new(-4., -2.),
            c + Vec2::new(4., -2.),
            c + Vec2::new(0., 3.),
        ]
    } else {
        vec![
            c + Vec2::new(-2., -4.),
            c + Vec2::new(3., 0.),
            c + Vec2::new(-2., 4.),
        ]
    };
    ui.painter().add(egui::Shape::convex_polygon(
        triangle,
        theme::gray(if toggle.hovered() { 235 } else { 190 }),
        Stroke::NONE,
    ));
    ui.painter().text(
        Pos2::new(rect.left() + 26., rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(13.),
        theme::gray(if toggle.hovered() { 250 } else { 235 }),
    );
    if resettable {
        let color = theme::gray(if reset.hovered() { 240 } else { 150 });
        icons::paint_at(ui.painter(), Icon::Reset, reset_rect.center(), 12., color);
    }
    if toggle.clicked() {
        open = !open;
        ui.ctx().data_mut(|d| {
            let set = d.get_temp_mut_or_default::<std::collections::BTreeSet<String>>(
                collapsed_sections_id(),
            );
            if open {
                set.remove(title);
            } else {
                set.insert(title.to_string());
            }
        });
    }
    if resettable && reset.clicked() {
        name_history_step(ui, format!("Reset {title}"), String::new());
    }
    if open {
        set_edit_context(ui, title);
        // Scope ids per section so equal slider labels (e.g. two "Amount"s) never clash.
        ui.push_id(title, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin {
                    left: 8,
                    right: 8,
                    top: 8,
                    bottom: 12,
                })
                .show(ui, contents);
        });
    }
    resettable && reset.clicked()
}
#[derive(Clone, Default)]
struct CurveInteraction {
    selected: Option<usize>,
    dragging: Option<usize>,
}
/// Lightroom's curve backdrop: mid-gray field, the image histogram behind the
/// curve, a quarter grid and a dark frame. `channel` 0 is RGB, 1–3 are R, G, B.
fn curve_backdrop(ui: &egui::Ui, rect: Rect, histogram: &[[u32; 256]; 3], channel: usize) {
    let painter = ui.painter();
    painter.rect_filled(rect, 0., theme::gray(82));
    let bins: Vec<f32> = (0..256)
        .map(|i| match channel {
            1..=3 => histogram[channel - 1][i] as f32,
            _ => histogram.iter().map(|h| h[i] as f32).sum::<f32>(),
        })
        .collect();
    // Ignore the extreme bins when scaling so clipped pixels don't flatten the rest.
    let max = bins[1..255].iter().copied().fold(1., f32::max);
    let fill = match channel {
        1 => Color32::from_rgb(112, 62, 60),
        2 => Color32::from_rgb(62, 102, 66),
        3 => Color32::from_rgb(62, 78, 118),
        _ => theme::gray(58),
    };
    if bins.iter().any(|v| *v > 0.) {
        let mut mesh = egui::Mesh::default();
        for (i, v) in bins.iter().enumerate() {
            let x = rect.left() + i as f32 / 255. * rect.width();
            let h = ((v / max).sqrt()).min(1.) * rect.height() * 0.92;
            let base = mesh.vertices.len() as u32;
            mesh.colored_vertex(Pos2::new(x, rect.bottom()), fill);
            mesh.colored_vertex(Pos2::new(x, rect.bottom() - h), fill);
            if i > 0 {
                mesh.add_triangle(base - 2, base - 1, base);
                mesh.add_triangle(base - 1, base, base + 1);
            }
        }
        painter.add(mesh);
    }
    for i in 1..4 {
        let t = i as f32 / 4.;
        let x = rect.left() + t * rect.width();
        let y = rect.bottom() - t * rect.height();
        let grid = Stroke::new(1., Color32::from_rgba_unmultiplied(160, 160, 160, 60));
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            grid,
        );
        painter.line_segment(
            [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
            grid,
        );
    }
    painter.line_segment(
        [rect.left_bottom(), rect.right_top()],
        Stroke::new(1., Color32::from_rgba_unmultiplied(200, 200, 200, 45)),
    );
    painter.rect_stroke(
        rect,
        0.,
        Stroke::new(1., theme::gray(15)),
        egui::StrokeKind::Outside,
    );
}
fn curve_color(channel: usize) -> Color32 {
    match channel {
        1 => Color32::from_rgb(240, 110, 100),
        2 => Color32::from_rgb(120, 215, 125),
        3 => Color32::from_rgb(120, 160, 245),
        _ => theme::gray(255),
    }
}
/// Read-out under a curve: the input and output values at the pointer.
fn curve_readout(ui: &mut egui::Ui, value: Option<[f32; 2]>) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.), Sense::hover());
    if let Some([x, y]) = value {
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            format!("{:.0} / {:.0}", x * 255., y * 255.),
            egui::FontId::proportional(11.),
            theme::gray(170),
        );
    }
}
/// Parametric curve: dragging up or down in the graph changes the region
/// under the pointer, and the three handles below move the region splits.
pub(super) fn parametric_curve_ui(
    ui: &mut egui::Ui,
    effects: &mut crate::develop::effects::Effects,
    histogram: &[[u32; 256]; 3],
) {
    let size = ui.available_width();
    let (outer, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click_and_drag());
    let rect = outer.shrink(4.);
    curve_backdrop(ui, rect, histogram, 0);
    let region = |x: f32, splits: [f32; 3]| splits.iter().filter(|s| x > **s).count();
    let hover = response.hover_pos().filter(|p| rect.contains(*p));
    let hovered_region = hover.map(|p| region((p.x - rect.left()) / rect.width(), effects.splits));
    if let Some(i) = hovered_region {
        let bounds = [
            0.,
            effects.splits[0],
            effects.splits[1],
            effects.splits[2],
            1.,
        ];
        ui.painter().rect_filled(
            Rect::from_x_y_ranges(
                rect.left() + bounds[i] * rect.width()..=rect.left() + bounds[i + 1] * rect.width(),
                rect.y_range(),
            ),
            0.,
            Color32::from_white_alpha(14),
        );
    }
    if response.dragged()
        && let Some(p) = response.interact_pointer_pos()
    {
        let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(p);
        let i = region((origin.x - rect.left()) / rect.width(), effects.splits);
        let delta = -response.drag_delta().y / rect.height() * 2.;
        effects.parametric[i] = (effects.parametric[i] + delta).clamp(-1., 1.);
    }
    if response.double_clicked()
        && let Some(i) = hovered_region
    {
        effects.parametric[i] = 0.;
    }
    let pts: Vec<_> = (0..=128)
        .map(|i| {
            let x = i as f32 / 128.;
            Pos2::new(
                rect.left() + x * rect.width(),
                rect.bottom() - effects.parametric(x) * rect.height(),
            )
        })
        .collect();
    ui.painter()
        .add(egui::Shape::line(pts, Stroke::new(2., theme::gray(255))));
    let name = hovered_region.map(|i| ["Shadows", "Darks", "Lights", "Highlights"][i]);
    response
        .on_hover_cursor(egui::CursorIcon::ResizeVertical)
        .on_hover_text(name.map_or(String::new(), |n| {
            format!("{n}: drag up or down · double-click to reset")
        }));
    // Split handles, as under Lightroom's parametric curve.
    let (strip, _) = ui.allocate_exact_size(Vec2::new(size, 14.), Sense::hover());
    let track = strip.shrink2(Vec2::new(4., 0.));
    ui.painter().rect_filled(
        Rect::from_x_y_ranges(track.x_range(), track.top() + 2.0..=track.top() + 4.),
        1.,
        theme::gray(60),
    );
    for i in 0..3 {
        let x = track.left() + effects.splits[i] * track.width();
        let handle = Rect::from_center_size(Pos2::new(x, track.center().y), Vec2::new(12., 14.));
        let r = ui
            .interact(handle, ui.id().with(("split", i)), Sense::drag())
            .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
        if r.dragged()
            && let Some(p) = r.interact_pointer_pos()
        {
            let lo = if i == 0 {
                0.05
            } else {
                effects.splits[i - 1] + 0.05
            };
            let hi = if i == 2 {
                0.95
            } else {
                effects.splits[i + 1] - 0.05
            };
            effects.splits[i] = ((p.x - track.left()) / track.width()).clamp(lo, hi);
        }
        if r.double_clicked() {
            effects.splits[i] = [0.25, 0.5, 0.75][i];
        }
        let color = theme::gray(if r.hovered() || r.dragged() { 240 } else { 175 });
        let top = Pos2::new(x, track.top() + 3.);
        ui.painter().add(egui::Shape::convex_polygon(
            vec![top, top + Vec2::new(5., 9.), top + Vec2::new(-5., 9.)],
            color,
            Stroke::NONE,
        ));
    }
}
pub(super) fn tone_curve_ui(
    ui: &mut egui::Ui,
    curve: &mut crate::develop::curve::ToneCurve,
    histogram: &[[u32; 256]; 3],
    channel: usize,
) {
    let id = ui.make_persistent_id("tone-curve-editor");
    let mut state = ui
        .ctx()
        .data(|d| d.get_temp::<CurveInteraction>(id))
        .unwrap_or_default();
    state.selected = state.selected.filter(|i| *i < curve.points.len());
    let (outer, response) =
        ui.allocate_exact_size(Vec2::splat(ui.available_width()), Sense::click_and_drag());
    let rect = outer.shrink(4.);
    let screen = |p: [f32; 2]| {
        Pos2::new(
            rect.left() + p[0] * rect.width(),
            rect.bottom() - p[1] * rect.height(),
        )
    };
    let snap = curve.natural;
    let value = |p: Pos2| {
        let p = [
            (p.x - rect.left()) / rect.width(),
            (rect.bottom() - p.y) / rect.height(),
        ];
        if snap {
            p.map(|v| (v * 255.).round() / 255.)
        } else {
            p
        }
    };
    let pointer = response
        .interact_pointer_pos()
        .or_else(|| ui.input(|i| i.pointer.hover_pos()));
    let hit = pointer.and_then(|p| {
        curve
            .points
            .iter()
            .position(|q| screen(*q).distance(p) < 9.)
    });
    if response.clicked_by(egui::PointerButton::Secondary) {
        if let Some(i) = hit {
            curve.remove(i);
            state.selected = None;
        }
    } else if response.drag_started() || response.clicked() {
        response.request_focus();
        // Drag origin keeps the same point selected even after a fast initial movement.
        let origin = if response.drag_started() {
            ui.input(|i| i.pointer.press_origin()).or(pointer)
        } else {
            pointer
        };
        if let Some(p) = origin {
            let i = curve
                .points
                .iter()
                .position(|q| screen(*q).distance(p) < 9.)
                .unwrap_or_else(|| curve.insert(value(p)));
            state.selected = Some(i);
            if response.drag_started() {
                state.dragging = Some(i);
            }
        }
    }
    if response.dragged()
        && let (Some(i), Some(p)) = (state.dragging, pointer)
    {
        curve.move_point(i, value(p));
    }
    if response.drag_stopped() {
        state.dragging = None;
    }
    if response.has_focus()
        && ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
        && let Some(i) = state.selected
    {
        curve.remove(i);
        state.selected = None;
    }
    curve_backdrop(ui, rect, histogram, channel);
    let plot = crate::develop::curve::CurveLut::new(curve);
    let pts = (0..=256)
        .map(|i| {
            let x = i as f32 / 256.;
            screen([x, plot.evaluate(x)])
        })
        .collect();
    ui.painter().add(egui::Shape::line(
        pts,
        Stroke::new(2., curve_color(channel)),
    ));
    for (i, p) in curve.points.iter().enumerate() {
        let p = screen(*p);
        let active = state.selected == Some(i) || hit == Some(i);
        ui.painter().circle_filled(
            p,
            if active { 5.5 } else { 4.5 },
            if state.selected == Some(i) {
                theme::gray(25)
            } else {
                theme::gray(255)
            },
        );
        ui.painter().circle_stroke(
            p,
            if active { 5.5 } else { 4.5 },
            Stroke::new(
                1.5,
                if state.selected == Some(i) {
                    theme::gray(255)
                } else {
                    theme::gray(20)
                },
            ),
        );
    }
    let readout = state.dragging.map(|i| curve.points[i]).or_else(|| {
        pointer.filter(|p| rect.contains(*p)).map(|p| {
            let [x, _] = value(p);
            [x, plot.evaluate(x)]
        })
    });
    response.on_hover_cursor(if state.dragging.is_some() {
        egui::CursorIcon::Grabbing
    } else if hit.is_some() {
        egui::CursorIcon::Grab
    } else {
        egui::CursorIcon::Crosshair
    });
    curve_readout(ui, readout);
    if let Some(i) = state.selected {
        let mut point = curve.points[i].map(|v| v * 255.);
        let changed = ui
            .horizontal(|ui| {
                ui.label("Input");
                let x = ui
                    .add(
                        egui::DragValue::new(&mut point[0])
                            .range(0. ..=255.)
                            .update_while_editing(false)
                            .speed(1.)
                            .fixed_decimals(0),
                    )
                    .changed();
                ui.label("Output");
                let y = ui
                    .add(
                        egui::DragValue::new(&mut point[1])
                            .range(0. ..=255.)
                            .update_while_editing(false)
                            .speed(1.)
                            .fixed_decimals(0),
                    )
                    .changed();
                x || y
            })
            .inner;
        if changed {
            curve.move_point(i, point.map(|v| v / 255.));
        }
    }
    ui.label(
        egui::RichText::new("Click to add · drag to shape · right-click to remove")
            .size(10.)
            .color(theme::gray(125)),
    );
    ui.ctx().data_mut(|d| d.insert_temp(id, state));
}
/// Lightroom-style slider. Normalized ranges within ±1 display as integers
/// (×100), matching Lightroom's numbers while the recipe keeps its units.
pub(super) fn slider(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    default: f32,
) {
    slider_with(ui, label, value, range, default, None, None);
}
/// `display` overrides the shown scale and decimals, e.g. Sharpening's 0–150.
pub(super) fn slider_with(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    default: f32,
    display: Option<(f32, usize)>,
    gradient: Option<(Color32, Color32)>,
) {
    let start = *range.start();
    let end = *range.end();
    let span = end - start;
    let (scale, decimals) = display.unwrap_or(if start >= -1. && end <= 1. {
        (100., 0)
    } else if span >= 100. {
        (1., 0)
    } else {
        (1., 2)
    });
    let signed = start < 0. && default == 0.;
    // Like Lightroom, Temp moves evenly in mireds rather than kelvin.
    let reciprocal = label == "Temp" && start > 0.;
    // Dragging Exposure moves in Lightroom's 0.05 EV steps; typed values stay exact.
    let step = (label == "Exposure").then_some(0.05);
    let to_rail = move |v: f32, rail: Rect| {
        let t = if reciprocal {
            egui::remap_clamp(1. / v, 1. / start..=1. / end, 0. ..=1.)
        } else {
            egui::remap_clamp(v, start..=end, 0. ..=1.)
        };
        egui::lerp(rail.left()..=rail.right(), t)
    };
    let from_rail = move |x: f32, rail: Rect| {
        let t = egui::remap_clamp(x, rail.left()..=rail.right(), 0. ..=1.);
        if reciprocal {
            1. / egui::lerp(1. / start..=1. / end, t)
        } else {
            egui::lerp(start..=end, t)
        }
    };
    let before = *value;
    ui.push_id(label, |ui| {
        let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.), Sense::hover());
        let label_rect = Rect::from_min_max(row.min, Pos2::new(row.left() + 83., row.bottom()));
        let label_response = ui.interact(label_rect, ui.id().with("label"), Sense::click());
        ui.painter().text(
            label_rect.right_center() - Vec2::new(5., 0.),
            egui::Align2::RIGHT_CENTER,
            label,
            egui::FontId::proportional(11.),
            theme::gray(190),
        );
        if label_response.double_clicked() {
            *value = default.clamp(start, end);
        }
        let value_rect = Rect::from_min_max(Pos2::new(row.right() - 43., row.top()), row.max);
        let mut displayed = *value * scale;
        let value_response = ui.place(
            value_rect,
            egui::DragValue::new(&mut displayed)
                .range(start * scale..=end * scale)
                .speed(span * scale / 500.)
                .custom_formatter(move |v, _| slider_text(v, decimals, signed))
                .custom_parser(|s| s.trim().trim_start_matches('+').parse().ok()),
        );
        if value_response.changed() {
            *value = (displayed / scale).clamp(start, end);
        }
        let area = Rect::from_min_max(
            Pos2::new(label_rect.right(), row.top()),
            Pos2::new(value_rect.left() - 4., row.bottom()),
        );
        let response = ui.interact(area, ui.id().with("rail"), Sense::click_and_drag());
        let rail = Rect::from_min_max(
            Pos2::new(area.left() + 5., area.center().y - 1.),
            Pos2::new(area.right() - 5., area.center().y + 1.),
        );
        let gradient = gradient.or(match label {
            "Temp" => Some((
                Color32::from_rgb(74, 123, 182),
                Color32::from_rgb(194, 169, 93),
            )),
            "Tint" => Some((
                Color32::from_rgb(91, 156, 112),
                Color32::from_rgb(165, 105, 158),
            )),
            _ => None,
        });
        if label == "Hue" && start == 0. && end == 360. {
            let mut mesh = egui::Mesh::default();
            for sector in 0..6 {
                let left = egui::lerp(rail.left()..=rail.right(), sector as f32 / 6.);
                let right = egui::lerp(rail.left()..=rail.right(), (sector + 1) as f32 / 6.);
                let color = |h| {
                    let c = crate::develop::color::hue_rgb(h);
                    Color32::from_rgb(
                        (c[0] * 180.) as u8,
                        (c[1] * 180.) as u8,
                        (c[2] * 180.) as u8,
                    )
                };
                let a = color(sector as f32 / 6.);
                let b = color((sector + 1) as f32 / 6.);
                let first = mesh.vertices.len() as u32;
                mesh.colored_vertex(Pos2::new(left, rail.top()), a);
                mesh.colored_vertex(Pos2::new(right, rail.top()), b);
                mesh.colored_vertex(Pos2::new(right, rail.bottom()), b);
                mesh.colored_vertex(Pos2::new(left, rail.bottom()), a);
                mesh.add_triangle(first, first + 1, first + 2);
                mesh.add_triangle(first, first + 2, first + 3);
            }
            ui.painter().add(mesh);
        } else if let Some((a, b)) = gradient {
            let mut mesh = egui::Mesh::default();
            mesh.colored_vertex(rail.left_top(), a);
            mesh.colored_vertex(rail.right_top(), b);
            mesh.colored_vertex(rail.right_bottom(), b);
            mesh.colored_vertex(rail.left_bottom(), a);
            mesh.add_triangle(0, 1, 2);
            mesh.add_triangle(0, 2, 3);
            ui.painter().add(mesh);
        } else {
            ui.painter().rect_filled(rail, 1., theme::gray(83));
        }
        let neutral = to_rail(default, rail);
        ui.painter().line_segment(
            [
                Pos2::new(neutral, area.center().y - 4.),
                Pos2::new(neutral, area.center().y + 4.),
            ],
            Stroke::new(1., theme::gray(115)),
        );
        if response.double_clicked() {
            *value = default.clamp(start, end);
        } else if (response.dragged() || response.clicked())
            && let Some(p) = response.interact_pointer_pos()
        {
            let v = from_rail(p.x, rail);
            let v = step.map_or(v, |step| (v / step).round() * step);
            *value = v.clamp(start, end);
        }
        let x = to_rail(*value, rail);
        if gradient.is_none() {
            ui.painter().line_segment(
                [
                    Pos2::new(neutral, area.center().y),
                    Pos2::new(x, area.center().y),
                ],
                Stroke::new(2., theme::gray(153)),
            );
        }
        let center = Pos2::new(x, area.center().y);
        ui.painter().circle_filled(
            center,
            if response.hovered() || response.dragged() {
                4.5
            } else {
                3.5
            },
            theme::gray(205),
        );
        ui.painter()
            .circle_stroke(center, 3.5, Stroke::new(1., theme::gray(26)));
        response.on_hover_text(
            "Drag to adjust · double-click to reset. Drag or type the number for precise edits.",
        );
    });
    if *value != before {
        let context = edit_context(ui);
        let name = if label.is_empty() {
            context
        } else if matches!(
            context.as_str(),
            "" | "Basic" | "Tone" | "Presence" | "Tone Curve"
        ) {
            label.to_string()
        } else {
            format!("{context} {label}")
        };
        let shown = slider_text(f64::from(*value * scale), decimals, signed);
        name_history_step(ui, name, shown);
    }
}
/// A slider's number as shown: Lightroom's scale, with a sign when it has one.
fn slider_text(v: f64, decimals: usize, signed: bool) -> String {
    let text = format!("{v:.decimals$}");
    if signed && v > 0. && !text.trim_start_matches(['0', '.']).is_empty() {
        format!("+{text}")
    } else {
        text
    }
}
/// A Lightroom module-picker entry: plain text, brightest when active.
pub(super) fn workspace_tab(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        label.into(),
        egui::FontId::proportional(16.),
        theme::gray(255),
    );
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(galley.size().x + 24., 28.), Sense::click());
    let color = theme::gray(if selected {
        248
    } else if !ui.is_enabled() {
        70
    } else if response.hovered() {
        175
    } else {
        105
    });
    ui.painter()
        .galley(rect.center() - galley.size() / 2., galley, color);
    if selected {
        // Accent bar under the active module.
        let bar = Rect::from_center_size(
            Pos2::new(rect.center().x, rect.bottom() + 5.),
            Vec2::new(rect.width() - 20., 2.),
        );
        ui.painter()
            .rect_filled(bar, 1., Color32::from_rgb(120, 165, 210));
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
/// Compact Lightroom-style segmented control spanning `width`.
pub(super) fn segmented<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    value: &mut T,
    options: &[(T, &str)],
    width: f32,
) -> bool {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 22.), Sense::hover());
    ui.painter().rect_filled(rect, 3., theme::gray(30));
    let segment = rect.width() / options.len() as f32;
    let mut changed = false;
    for (i, (option, label)) in options.iter().enumerate() {
        let cell = Rect::from_min_size(
            Pos2::new(rect.left() + i as f32 * segment, rect.top()),
            Vec2::new(segment, rect.height()),
        );
        let response = ui.interact(cell, ui.id().with(("segment", label, i)), Sense::click());
        let active = *value == *option;
        if active || response.hovered() {
            ui.painter().rect_filled(
                cell.shrink(1.),
                3.,
                theme::gray(if active { 72 } else { 44 }),
            );
        }
        let text = egui::WidgetText::from(*label).into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            cell.width() - 4.,
            egui::FontId::proportional(11.),
        );
        ui.painter().galley(
            cell.center() - text.size() / 2.,
            text,
            theme::gray(if active { 240 } else { 165 }),
        );
        if response.clicked() && !active {
            *value = *option;
            changed = true;
        }
    }
    changed
}
/// A full-width menu row: optional check mark, label, right-aligned shortcut.
pub(super) fn menu_item(
    ui: &mut egui::Ui,
    label: &str,
    shortcut: &str,
    enabled: bool,
    checked: bool,
) -> bool {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), 24.),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    if enabled && response.hovered() {
        ui.painter().rect_filled(rect, 3., theme::accent());
    }
    let color = if enabled && response.hovered() {
        theme::on_accent_text(250)
    } else {
        theme::gray(if enabled { 215 } else { 100 })
    };
    if checked {
        let c = rect.left_center() + Vec2::new(11., 0.);
        icons::paint_at(ui.painter(), Icon::Check, c, 13., color);
    }
    ui.painter().text(
        rect.left_center() + Vec2::new(22., 0.),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(13.),
        color,
    );
    if !shortcut.is_empty() {
        ui.painter().text(
            rect.right_center() - Vec2::new(10., 0.),
            egui::Align2::RIGHT_CENTER,
            shortcut,
            egui::FontId::proportional(11.),
            theme::gray(if enabled { 140 } else { 90 }),
        );
    }
    enabled && response.clicked()
}
pub(super) fn menu_separator(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 9.), Sense::hover());
    ui.painter().line_segment(
        [rect.left_center(), rect.right_center()],
        Stroke::new(1., theme::gray(55)),
    );
}
/// True for a context-menu click: a right click, or Control-click on macOS.
pub(super) fn context_clicked(response: &egui::Response) -> bool {
    response.secondary_clicked()
        || (cfg!(target_os = "macos")
            && response.clicked()
            && response.ctx.input(|i| i.modifiers.ctrl))
}
/// Like `Response::context_menu`, but Control-click also opens it on macOS.
pub(super) fn context_menu(response: &egui::Response, add: impl FnOnce(&mut egui::Ui)) {
    let command = if context_clicked(response) {
        Some(egui::SetOpenCommand::Bool(true))
    } else if response.clicked() {
        Some(egui::SetOpenCommand::Bool(false))
    } else {
        None
    };
    egui::Popup::menu(response)
        .open_memory(command)
        .at_pointer_fixed()
        .show(add);
}
/// Makes egui submenu buttons match `menu_item` rows: same height, text
/// inset and hover color.
pub(super) fn submenu_style(ui: &mut egui::Ui) {
    let spacing = ui.spacing_mut();
    spacing.item_spacing.y = 0.;
    spacing.button_padding = Vec2::new(22., 4.);
    spacing.interact_size.y = 24.;
    let visuals = ui.visuals_mut();
    for widget in [
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.open,
        &mut visuals.widgets.active,
    ] {
        widget.weak_bg_fill = theme::accent();
        widget.bg_fill = theme::accent();
        widget.bg_stroke = Stroke::NONE;
        widget.fg_stroke = Stroke::new(1., theme::on_accent_text(250));
    }
    visuals.widgets.inactive.fg_stroke = Stroke::new(1., theme::gray(215));
}
/// The frame of a modal window such as Preferences or Export.
pub(super) fn modal_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(theme::gray(33))
        .stroke(Stroke::new(1., theme::gray(52)))
        .corner_radius(10.)
}
/// The button that confirms a modal window ("Done", "Export").
pub(super) fn primary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).color(theme::on_accent()))
            .fill(theme::accent())
            .min_size(Vec2::new(84., 30.)),
    )
}
/// A form row as in Lightroom's dialogs: a right-aligned label in a fixed
/// column, then the controls.
pub(super) fn form_row(ui: &mut egui::Ui, label: &str, contents: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(150., 28.), Sense::hover());
        ui.painter().text(
            rect.right_center() - Vec2::new(12., 0.),
            egui::Align2::RIGHT_CENTER,
            label,
            egui::FontId::proportional(13.),
            theme::gray(150),
        );
        contents(ui);
    });
}
/// A path for display, with the home folder as `~`.
pub(super) fn pretty_path(path: &std::path::Path) -> String {
    let text = path.display().to_string();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && text.starts_with(&home) => {
            format!("~{}", &text[home.len()..])
        }
        _ => text,
    }
}
