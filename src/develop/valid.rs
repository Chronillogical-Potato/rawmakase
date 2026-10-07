//! A recipe known to be valid. Render entry points take a [`ValidRecipe`], so none
//! renders a recipe nobody checked, and a recipe checked once is not checked again
//! on the way down.
use super::Recipe;
use anyhow::Result;
use std::borrow::Cow;

/// A recipe that passed [`Recipe::validate`]: borrowed from the recipe checked, or
/// owned when it was changed since. It reads as the recipe; changing it goes through
/// [`map`](Self::map), which checks the result again.
#[derive(Clone, Debug)]
pub struct ValidRecipe<'a>(Cow<'a, Recipe>);

impl Recipe {
    /// This recipe, checked; the error says what is out of bounds.
    pub fn checked(&self) -> Result<ValidRecipe<'_>> {
        self.validate()?;
        Ok(ValidRecipe(Cow::Borrowed(self)))
    }
    /// This recipe, checked and kept.
    pub fn into_checked(self) -> Result<ValidRecipe<'static>> {
        self.validate()?;
        Ok(ValidRecipe(Cow::Owned(self)))
    }
}

impl ValidRecipe<'_> {
    /// A copy changed by `change`, checked again.
    pub fn map(&self, change: impl FnOnce(&mut Recipe)) -> Result<ValidRecipe<'static>> {
        let mut changed = self.0.as_ref().clone();
        change(&mut changed);
        changed.into_checked()
    }
}

impl std::ops::Deref for ValidRecipe<'_> {
    type Target = Recipe;
    fn deref(&self) -> &Recipe {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_valid_recipe_is_checked_and_a_change_is_checked_again() {
        let r = Recipe::default();
        let valid = r.checked().unwrap();
        assert_eq!(valid.exposure, 0.);
        assert!(valid.map(|r| r.exposure = 1.).is_ok());
        assert!(valid.map(|r| r.exposure = f32::NAN).is_err());
        let invalid = Recipe {
            contrast: 2.,
            ..Recipe::default()
        };
        assert!(invalid.checked().is_err());
        assert!(invalid.into_checked().is_err());
    }
}
