//! The Basic panel's Treatment and the B&W panel's Auto, each one History step.
use super::{Editor, history::Step};
use crate::develop::{AutoMix, ColorSpread, Recipe, Treatment};
use crate::raw::Metadata;

/// What converting to black & white does to a mix that was never set: Lightroom's
/// "Apply auto mix when first converting to black and white" preference, on by
/// default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum FirstConversion {
    #[default]
    AutoMix,
    KeepMix,
}

/// What Auto black & white measures for the open photo: its colors and camera.
pub(super) struct PhotoColors {
    spread: ColorSpread,
    metadata: Metadata,
}

impl PhotoColors {
    pub(super) fn auto_mix(&self) -> AutoMix<'_> {
        AutoMix {
            spread: &self.spread,
            metadata: &self.metadata,
        }
    }
}

impl Editor {
    /// The open photo's colors, once it is decoded.
    pub(super) fn photo_colors(&self) -> Option<PhotoColors> {
        Some(PhotoColors {
            spread: self.document.color_spread()?,
            metadata: self.document.metadata.clone()?,
        })
    }

    /// The Auto mix a first conversion applies: none while the preference is off.
    pub(super) fn first_conversion_colors(&self) -> Option<PhotoColors> {
        match self.first_conversion {
            FirstConversion::AutoMix => self.photo_colors(),
            FirstConversion::KeepMix => None,
        }
    }

    /// Lightroom's Treatment switcher, and V: converting to black & white applies
    /// the Auto mix the first time, if the preference asks for it.
    pub(super) fn set_treatment(&mut self, treatment: Treatment) {
        if self.document.recipe.treatment() == treatment {
            return;
        }
        let old = self.begin_step();
        let colors = self.first_conversion_colors();
        let first = colors.as_ref().map(PhotoColors::auto_mix);
        let document = &mut self.document;
        match &document.metadata {
            Some(m) => document
                .recipe
                .choose_treatment(treatment, first, m, &document.profiles),
            None => document.recipe.set_treatment(treatment, first),
        }
        let name = match treatment {
            Treatment::Color => "Convert to Color",
            Treatment::BlackWhite => "Convert to Black & White",
        };
        self.end_step(old, Step::new(name, ""));
    }

    /// V: switches between Color and Black & White.
    pub(super) fn toggle_treatment(&mut self) {
        self.set_treatment(match self.document.recipe.treatment() {
            Treatment::Color => Treatment::BlackWhite,
            Treatment::BlackWhite => Treatment::Color,
        });
    }

    /// The B&W panel's Auto: sets the black & white mix from the photo's colors.
    pub(super) fn auto_black_white_mix(&mut self) {
        let Some(colors) = self.photo_colors() else {
            return;
        };
        let mix = colors.auto_mix().for_recipe(&self.document.recipe);
        if self.document.recipe.effects.gray_mix == mix {
            return;
        }
        let old = self.begin_step();
        self.document.recipe.effects.gray_mix = mix;
        self.end_step(old, Step::new("Black & White Mix", "Auto"));
    }

    /// Records a drag still under way first, so undoing the step keeps it; returns
    /// the recipe before the step.
    fn begin_step(&mut self) -> Recipe {
        if self.document.history.in_gesture() {
            self.document.history.finish_gesture(&self.document.recipe);
            self.document.save.mark_changed();
        }
        self.document.recipe.clone()
    }

    fn end_step(&mut self, old: Recipe, step: Step) {
        self.document.history.label(step);
        self.history(old);
        self.schedule();
    }
}
