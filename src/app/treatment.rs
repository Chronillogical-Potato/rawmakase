//! The Basic panel's Treatment and the B&W panel's Auto. Each runs during an edit
//! frame, which records it as one History step under the name it gives.
use super::{Editor, history::Step};
use crate::develop::{AutoMix, ColorSpread, Treatment};
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
    /// the Auto mix the first time, if the preference asks for it. Called during an
    /// edit frame, which records the change as one History step under this name.
    /// While the photo is still decoding, a conversion that needs the Auto mix waits
    /// for it ([`Editor::finish_pending_treatment`]).
    pub(super) fn set_treatment(&mut self, treatment: Treatment) {
        self.document.pending_treatment = None;
        if self.document.recipe.treatment() == treatment {
            return;
        }
        let colors = self.first_conversion_colors();
        let needs_auto = treatment == Treatment::BlackWhite
            && self.first_conversion == FirstConversion::AutoMix
            && self.document.recipe.effects.gray_mix == [0.; 8];
        if needs_auto && colors.is_none() {
            self.document.pending_treatment = Some(treatment);
            self.status = "Converting to Black & White once the photo is decoded".into();
            return;
        }
        let first = colors.as_ref().map(PhotoColors::auto_mix);
        let color_profile = self.photo_defaults().and_then(|d| d.recipe.profile);
        let document = &mut self.document;
        match &document.metadata {
            Some(m) => document
                .recipe
                .choose_treatment(treatment, first, color_profile, m),
            None => document.recipe.set_treatment(treatment, first),
        }
        let name = match treatment {
            Treatment::Color => "Convert to Color",
            Treatment::BlackWhite => "Convert to Black & White",
        };
        self.document.history.label(Step::new(name, ""));
    }

    /// Converts as asked while the photo was decoding, once it is decoded. Called
    /// during an edit frame.
    pub(super) fn finish_pending_treatment(&mut self) {
        if self.document.full().is_none() {
            return;
        }
        if let Some(treatment) = self.document.pending_treatment.take() {
            self.set_treatment(treatment);
        }
        if std::mem::take(&mut self.document.pending_auto_mix)
            && self.document.recipe.treatment() == Treatment::BlackWhite
            && self.document.recipe.effects.gray_mix == [0.; 8]
        {
            self.auto_black_white_mix();
        }
    }

    /// Keeps the Treatment with a newly chosen profile (see
    /// [`Recipe::follow_profile_treatment`]). Called during the edit frame that
    /// changed the profile, so it is part of that step.
    pub(super) fn follow_profile_treatment(
        &mut self,
        old: Option<&crate::camera_profiles::CameraProfile>,
    ) {
        let colors = self.first_conversion_colors();
        let first = colors.as_ref().map(PhotoColors::auto_mix);
        let was = self.document.recipe.effects.monochrome;
        self.document.recipe.follow_profile_treatment(old, first);
        // Converted before the photo decoded: the Auto mix follows once it has.
        let r = &self.document.recipe;
        self.document.pending_auto_mix = !was
            && r.effects.monochrome
            && r.effects.gray_mix == [0.; 8]
            && self.first_conversion == FirstConversion::AutoMix
            && colors.is_none();
    }

    /// V: switches between Color and Black & White.
    pub(super) fn toggle_treatment(&mut self) {
        // A conversion still waiting for the photo counts as done: V again cancels it.
        let shown = self
            .document
            .pending_treatment
            .take()
            .unwrap_or_else(|| self.document.recipe.treatment());
        self.set_treatment(match shown {
            Treatment::Color => Treatment::BlackWhite,
            Treatment::BlackWhite => Treatment::Color,
        });
    }

    /// The B&W panel's Auto: sets the black & white mix from the photo's colors.
    /// Called during an edit frame, which records it as one History step.
    pub(super) fn auto_black_white_mix(&mut self) {
        let Some(colors) = self.photo_colors() else {
            return;
        };
        let mix = colors.auto_mix().for_recipe(&self.document.recipe);
        if self.document.recipe.effects.gray_mix != mix {
            self.document.recipe.effects.gray_mix = mix;
            self.document
                .history
                .label(Step::new("Black & White Mix", "Auto"));
        }
    }
}
