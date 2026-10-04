//! Dragging in the histogram, as in Lightroom: the histogram is split into
//! Blacks, Shadows, Exposure, Highlights and Whites, and dragging sideways in
//! one moves that slider. The regions are fifths of the width, along the
//! histogram's grid lines.
use crate::develop::Recipe;
use eframe::egui::{self, CursorIcon, Rect, Sense};

/// A histogram region and the Basic slider it drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ToneRegion {
    Blacks,
    Shadows,
    Exposure,
    Highlights,
    Whites,
}
impl ToneRegion {
    pub(super) const ALL: [Self; 5] = [
        Self::Blacks,
        Self::Shadows,
        Self::Exposure,
        Self::Highlights,
        Self::Whites,
    ];
    /// The region at `x`, a fraction of the histogram's width.
    pub(super) fn at(x: f32) -> Self {
        Self::ALL[((x.clamp(0., 1.) * 5.) as usize).min(4)]
    }
    /// Left and right edges as fractions of the width.
    pub(super) fn span(self) -> [f32; 2] {
        let i = Self::ALL.iter().position(|r| *r == self).unwrap_or(0) as f32;
        [i / 5., (i + 1.) / 5.]
    }
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Blacks => "Blacks",
            Self::Shadows => "Shadows",
            Self::Exposure => "Exposure",
            Self::Highlights => "Highlights",
            Self::Whites => "Whites",
        }
    }
    pub(super) fn value(self, r: &Recipe) -> f32 {
        match self {
            Self::Blacks => r.blacks,
            Self::Shadows => r.shadows,
            Self::Exposure => r.exposure,
            Self::Highlights => r.highlights,
            Self::Whites => r.whites,
        }
    }
    fn value_mut(self, r: &mut Recipe) -> &mut f32 {
        match self {
            Self::Blacks => &mut r.blacks,
            Self::Shadows => &mut r.shadows,
            Self::Exposure => &mut r.exposure,
            Self::Highlights => &mut r.highlights,
            Self::Whites => &mut r.whites,
        }
    }
    /// The slider's range: Exposure ±5 EV, the others ±100 (stored ±1).
    fn limit(self) -> f32 {
        if self == Self::Exposure { 5. } else { 1. }
    }
    /// The value as its slider shows it: "+0.35" EV, "+12".
    pub(super) fn display(self, value: f32) -> String {
        let text = if self == Self::Exposure {
            format!("{value:.2}")
        } else {
            format!("{:.0}", value * 100.)
        };
        if text.trim_start_matches(['-', '0', '.']).is_empty() {
            text.trim_start_matches('-').to_string()
        } else if value > 0. {
            format!("+{text}")
        } else {
            text
        }
    }
    /// `start` moved by a drag of `dx`, a fraction of the histogram's width:
    /// the whole width moves the slider from its centre to one end. Steps as
    /// the slider's drag does: 0.05 EV, or one unit.
    pub(super) fn dragged(self, start: f32, dx: f32) -> f32 {
        let limit = self.limit();
        let steps = if self == Self::Exposure { 20. } else { 100. };
        let value = start + dx * limit;
        ((value * steps).round() / steps).clamp(-limit, limit)
    }
}

/// A drag in progress: its region and the value when it started.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ToneDrag {
    region: ToneRegion,
    start: f32,
}

/// The histogram's drag handling over `rect`. Returns the region to show:
/// the one dragged, or the one under the pointer.
pub(super) fn tone_drag_ui(
    ui: &egui::Ui,
    rect: Rect,
    drag: &mut Option<ToneDrag>,
    recipe: &mut Recipe,
) -> Option<ToneRegion> {
    let response = ui.interact(rect, ui.id().with("histogram-drag"), Sense::drag());
    let fraction = |x: f32| (x - rect.left()) / rect.width().max(1.);
    if response.drag_started()
        && let Some(p) = response.interact_pointer_pos()
    {
        let region = ToneRegion::at(fraction(p.x));
        *drag = Some(ToneDrag {
            region,
            start: region.value(recipe),
        });
    }
    if let Some(d) = *drag {
        if response.dragged() {
            let dx = response.total_drag_delta().map_or(0., |v| v.x) / rect.width().max(1.);
            *d.region.value_mut(recipe) = d.region.dragged(d.start, dx);
            let value = d.region.value(recipe);
            super::widgets::name_history_step(
                ui,
                d.region.label().to_string(),
                d.region.display(value),
            );
        }
        if !response.dragged() {
            *drag = None;
        }
    }
    let shown = drag
        .map(|d| d.region)
        .or_else(|| response.hover_pos().map(|p| ToneRegion::at(fraction(p.x))));
    if shown.is_some() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Pos2, Vec2};

    #[test]
    fn the_histogram_splits_into_fifths_from_blacks_to_whites() {
        assert_eq!(ToneRegion::at(0.), ToneRegion::Blacks);
        assert_eq!(ToneRegion::at(0.19), ToneRegion::Blacks);
        assert_eq!(ToneRegion::at(0.21), ToneRegion::Shadows);
        assert_eq!(ToneRegion::at(0.5), ToneRegion::Exposure);
        assert_eq!(ToneRegion::at(0.7), ToneRegion::Highlights);
        assert_eq!(ToneRegion::at(1.), ToneRegion::Whites);
        assert_eq!(ToneRegion::at(-3.), ToneRegion::Blacks);
        assert_eq!(ToneRegion::Highlights.span(), [0.6, 0.8]);
    }

    #[test]
    fn a_drag_moves_the_slider_in_its_own_steps_within_range() {
        // The whole width moves from the centre to one end.
        assert_eq!(ToneRegion::Exposure.dragged(0., 1.), 5.);
        assert_eq!(ToneRegion::Shadows.dragged(0., -0.25), -0.25);
        assert_eq!(ToneRegion::Exposure.dragged(1., 0.013), 1.05);
        assert_eq!(ToneRegion::Whites.dragged(0.9, 0.5), 1.);
        assert_eq!(ToneRegion::Exposure.display(0.35), "+0.35");
        assert_eq!(ToneRegion::Blacks.display(-0.12), "-12");
        assert_eq!(ToneRegion::Blacks.display(-0.001), "0");
    }

    #[test]
    fn one_drag_is_one_history_step_named_after_its_region() {
        let ctx = egui::Context::default();
        let rect = Rect::from_min_size(Pos2::new(0., 0.), Vec2::new(500., 100.));
        let mut recipe = Recipe::default();
        let mut drag = None;
        let mut history = super::super::history::History::default();
        let mut frame = |events: Vec<egui::Event>, down: bool, recipe: &mut Recipe| {
            let before = recipe.clone();
            let mut shown = None;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(600.))),
                    events,
                    ..Default::default()
                },
                |ui| shown = tone_drag_ui(ui, rect, &mut drag, recipe),
            );
            output.textures_delta.clear();
            if let Some((name, value)) = ctx.data_mut(|d| {
                d.remove_temp::<(String, String)>(super::super::widgets::history_step_id())
            }) {
                history.label(super::super::history::Step::new(name, value));
            }
            history.observe(before, recipe, down);
            shown
        };
        let button = |p: Pos2, pressed| egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        // Hovering shows the region without changing anything.
        let start = Pos2::new(70., 50.);
        frame(vec![], false, &mut recipe);
        assert_eq!(
            frame(vec![egui::Event::PointerMoved(start)], false, &mut recipe),
            Some(ToneRegion::Blacks)
        );
        frame(vec![button(start, true)], true, &mut recipe);
        // Leaving the region mid-drag keeps driving Blacks.
        for x in [120., 170., 220.] {
            let shown = frame(
                vec![egui::Event::PointerMoved(Pos2::new(x, 50.))],
                true,
                &mut recipe,
            );
            assert_eq!(shown, Some(ToneRegion::Blacks));
        }
        frame(
            vec![button(Pos2::new(220., 50.), false)],
            false,
            &mut recipe,
        );
        assert_eq!(recipe.blacks, 0.3);
        assert_eq!(recipe.exposure, 0.);
        let (steps, applied) = history.steps();
        assert_eq!(applied, 1);
        assert_eq!(
            (steps[0].name.as_str(), steps[0].value.as_str()),
            ("Blacks", "+30")
        );
    }
}
