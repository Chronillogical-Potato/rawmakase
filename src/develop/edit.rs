//! What changing a develop setting implies for the rest of the recipe. The
//! panels, the control socket and MIDI all change settings this way, so the side
//! effects of a change live in one place rather than in each of them.
use super::{Recipe, params::ParameterId};
use crate::camera_data::Metadata;

/// Brings the recipe in line after setting `id` changed from `previous`:
/// - a new Temp or Tint recomputes the white balance multipliers for `photo`,
///   which then no longer come from Auto;
/// - Texture, Clarity, Grain or the lens Vignetting Amount added from none renders
///   with the measured operator.
pub fn setting_changed(r: &mut Recipe, id: ParameterId, previous: f32, photo: Option<&Metadata>) {
    let current = *id.value_mut(r);
    match id {
        ParameterId::Temperature | ParameterId::Tint => {
            if current != previous
                && let Some(m) = photo
            {
                r.update_wb(m);
                r.auto_white_balance = None;
            }
        }
        ParameterId::Texture => r.adopt_measured_texture(previous),
        ParameterId::Clarity => r.adopt_measured_clarity(previous),
        ParameterId::GrainAmount => r.adopt_measured_grain(previous),
        ParameterId::LensVignetteAmount => r.adopt_measured_vignette(previous),
        _ => {}
    }
}

/// Turns a switched-off panel back on when the change from `before` touched only
/// its settings, as Lightroom does, so the change shows: a slider in it, or an edit
/// made another way (B&W Auto, Clear Guides, the fringe picker, a swatch).
pub fn turn_on_edited_panel(before: &Recipe, after: &mut Recipe) {
    use super::panels::{Panel, PanelState};
    let edited = Panel::ALL.into_iter().find(|panel| {
        before.panels.state(*panel) == PanelState::Off
            && after.panels.state(*panel) == PanelState::Off
            && panel.holds_change(before, after)
    });
    if let Some(panel) = edited {
        after.panels.set(panel, PanelState::On);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grain_added_from_none_takes_the_measured_operator() {
        use crate::model::operators::GrainModel;
        let mut r = Recipe::default();
        assert_eq!(r.grain_model, GrainModel::Original);
        // Size alone is not grain added.
        r.effects.grain_size = 0.5;
        setting_changed(&mut r, ParameterId::GrainSize, 0.25, None);
        assert_eq!(r.grain_model, GrainModel::Original);
        r.effects.grain = 0.2;
        setting_changed(&mut r, ParameterId::GrainAmount, 0., None);
        assert_eq!(r.grain_model, GrainModel::Measured);
    }

    #[test]
    fn lens_vignetting_added_from_none_takes_the_measured_operator() {
        use crate::model::operators::LensVignetteModel;
        let mut r = Recipe::default();
        assert_eq!(r.lens_vignette_model, LensVignetteModel::Original);
        r.effects.lens_vignette = 0.3;
        setting_changed(&mut r, ParameterId::LensVignetteAmount, 0., None);
        assert_eq!(r.lens_vignette_model, LensVignetteModel::Measured);
    }

    #[test]
    fn only_a_change_to_one_switched_off_panel_turns_it_on() {
        use crate::develop::panels::{Panel, PanelState};
        let mut before = Recipe::default();
        before.panels.set(Panel::Detail, PanelState::Off);
        let mut after = before.clone();
        after.sharpening = 0.5;
        turn_on_edited_panel(&before, &mut after);
        assert_eq!(after.panels.state(Panel::Detail), PanelState::On);
        // Not when the change reaches beyond it, as a preset's does.
        let mut after = before.clone();
        after.sharpening = 0.5;
        after.exposure = 1.;
        turn_on_edited_panel(&before, &mut after);
        assert_eq!(after.panels.state(Panel::Detail), PanelState::Off);
    }

    #[test]
    fn clarity_added_from_none_takes_the_measured_operator() {
        let mut r = Recipe::default();
        assert_eq!(
            r.clarity_model,
            crate::model::operators::ClarityModel::Original
        );
        r.effects.clarity = 0.2;
        setting_changed(&mut r, ParameterId::Clarity, 0., None);
        assert_eq!(
            r.clarity_model,
            crate::model::operators::ClarityModel::Measured
        );
    }

    #[test]
    fn white_balance_waits_for_a_photo_and_a_change() {
        let mut r = Recipe {
            auto_white_balance: Some([5000., 0.]),
            ..Recipe::default()
        };
        let before = r.temperature;
        setting_changed(&mut r, ParameterId::Temperature, before, None);
        assert!(r.auto_white_balance.is_some());
        r.temperature += 100.;
        setting_changed(&mut r, ParameterId::Temperature, before, None);
        assert!(
            r.auto_white_balance.is_some(),
            "no photo, nothing to recompute"
        );
    }
}
