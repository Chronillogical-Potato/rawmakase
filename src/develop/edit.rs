//! What changing a develop setting implies for the rest of the recipe. The
//! panels, the control socket and MIDI all change settings this way, so the side
//! effects of a change live in one place rather than in each of them.
use super::{Recipe, params::ParameterId};
use crate::raw::Metadata;

/// Brings the recipe in line after setting `id` changed from `previous`:
/// - a new Temp or Tint recomputes the white balance multipliers for `photo`,
///   which then no longer come from Auto;
/// - Texture or Clarity added from none renders with the measured operator.
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
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clarity_added_from_none_takes_the_measured_operator() {
        let mut r = Recipe::default();
        assert_eq!(
            r.clarity_model,
            crate::develop::clarity::ClarityModel::Original
        );
        r.effects.clarity = 0.2;
        setting_changed(&mut r, ParameterId::Clarity, 0., None);
        assert_eq!(
            r.clarity_model,
            crate::develop::clarity::ClarityModel::Measured
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
