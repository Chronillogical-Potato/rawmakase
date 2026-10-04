//! Which Adobe lens profile a photo uses: Lightroom's Lens Corrections › Profile
//! Setup, and the profile an edit names (`crs:LensProfileSetup`, `LensProfileName`,
//! `LensProfileFilename`, `LensProfileDigest`).
use super::lcp::{Candidate, ImportedProfile, PhotoProfiles};
use crate::raw::Metadata;
use serde::{Deserialize, Serialize};

/// Lightroom's Setup menu.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LensProfileSetup {
    /// The profile saved as the lens's default; RAWmakase keeps no saved lens
    /// defaults, so this matches automatically, as Lightroom does without one.
    #[default]
    Default,
    /// The imported profile that fits the photo's lens best.
    Auto,
    /// The profile the user picked, kept even for another lens.
    Custom,
}
impl LensProfileSetup {
    pub const ALL: [Self; 3] = [Self::Default, Self::Auto, Self::Custom];
    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Auto => "Auto",
            Self::Custom => "Custom",
        }
    }
    /// The `crs:LensProfileSetup` value.
    pub fn xmp(self) -> &'static str {
        match self {
            Self::Default => "LensDefaults",
            Self::Auto => "Auto",
            Self::Custom => "Custom",
        }
    }
    /// A `crs:LensProfileSetup` value; anything unknown reads as Default.
    pub fn from_xmp(value: &str) -> Self {
        match value {
            "Auto" => Self::Auto,
            "Custom" => Self::Custom,
            _ => Self::Default,
        }
    }
}

/// A profile as an edit records it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LensProfileId {
    /// `crs:LensProfileName`, e.g. "Adobe (Sony FE 55mm F1.8 ZA)".
    pub name: String,
    /// `crs:LensProfileFilename`, the LCP file's name.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub filename: String,
    /// `crs:LensProfileDigest`, Adobe's digest of the profile; kept as read, since
    /// RAWmakase cannot compute it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub digest: String,
}
impl LensProfileId {
    pub fn of(profile: &ImportedProfile) -> Self {
        Self {
            name: profile.name.clone(),
            filename: profile.filename.clone(),
            digest: String::new(),
        }
    }
    /// The name to show: the profile name, else the file name.
    pub fn label(&self) -> &str {
        if self.name.is_empty() {
            &self.filename
        } else {
            &self.name
        }
    }
}

/// The recipe's lens profile Setup and the profile it names.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LensProfileChoice {
    pub setup: LensProfileSetup,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<LensProfileId>,
}

/// What a choice renders with on one photo.
#[derive(Debug, Default)]
pub struct Resolved<'c, 'p> {
    /// The imported profile used, if any.
    pub used: Option<&'p Candidate>,
    /// The profile the edit names when it isn't imported.
    pub missing: Option<&'c LensProfileId>,
}

impl LensProfileChoice {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
    /// Picking a profile in the Make, Model or Profile menu, which sets Custom.
    pub fn choose(&mut self, profile: &ImportedProfile) {
        self.setup = LensProfileSetup::Custom;
        self.id = Some(self.id_for(profile));
    }
    /// The identity to record for `profile`: the edit's own when it names the same
    /// file, which keeps Adobe's digest.
    fn id_for(&mut self, profile: &ImportedProfile) -> LensProfileId {
        self.id
            .take()
            .filter(|id| profile.is(&id.filename, &id.name))
            .unwrap_or_else(|| LensProfileId::of(profile))
    }
    /// The choice as it renders: Default and Auto match alike, and the digest is left out.
    pub fn rendering(&self) -> Self {
        Self {
            setup: match self.setup {
                LensProfileSetup::Default => LensProfileSetup::Auto,
                setup => setup,
            },
            // Adobe's digest names the same file; it does not change the render.
            id: self.id.clone().map(|id| LensProfileId {
                digest: String::new(),
                ..id
            }),
        }
    }
    /// Picking a Setup. Custom keeps the profile in use; Default and Auto match again.
    pub fn set_setup(&mut self, setup: LensProfileSetup, in_use: Option<&ImportedProfile>) {
        self.setup = setup;
        self.id = match setup {
            LensProfileSetup::Custom => match in_use {
                Some(profile) => Some(self.id_for(profile)),
                None => self.id.take(),
            },
            LensProfileSetup::Default | LensProfileSetup::Auto => None,
        };
    }
    /// The profile this choice uses on a photo. A named profile is used when it is
    /// imported: under Custom whatever lens it was made for, under Default and Auto
    /// when it profiles the photo's lens (else the best match, as Lightroom's
    /// automatic choice). A named profile that isn't imported is reported; Custom
    /// then uses no Adobe profile.
    pub fn resolve<'c, 'p>(
        &'c self,
        profiles: &'p PhotoProfiles,
        m: &Metadata,
    ) -> Resolved<'c, 'p> {
        let named = self
            .id
            .as_ref()
            .map(|id| (id, profiles.find(&id.filename, &id.name)));
        match (self.setup, named) {
            (LensProfileSetup::Custom, Some((_, Some(found)))) => Resolved {
                used: Some(found).filter(|c| c.correction(m).is_some()),
                missing: None,
            },
            (LensProfileSetup::Custom, Some((id, None))) => Resolved {
                used: None,
                missing: Some(id),
            },
            (_, Some((_, Some(found))))
                if found.lens_rank.is_some() && found.correction(m).is_some() =>
            {
                Resolved {
                    used: Some(found),
                    missing: None,
                }
            }
            (_, named) => Resolved {
                used: profiles.auto(m),
                missing: named.and_then(|(id, found)| found.is_none().then_some(id)),
            },
        }
    }
}

/// Lightroom's Make, Model and Profile menus over a photo's profiles.
pub struct ProfileMenus<'p> {
    sorted: Vec<&'p ImportedProfile>,
}
impl<'p> ProfileMenus<'p> {
    pub fn new(profiles: &'p PhotoProfiles) -> Self {
        let mut sorted: Vec<&ImportedProfile> =
            profiles.all().iter().map(|c| c.profile.as_ref()).collect();
        sorted.sort_by(|a, b| {
            (&a.lens_make, &a.lens_model, &a.name, &a.filename).cmp(&(
                &b.lens_make,
                &b.lens_model,
                &b.name,
                &b.filename,
            ))
        });
        Self { sorted }
    }
    pub fn makes(&self) -> Vec<&'p str> {
        let mut makes: Vec<&str> = self.sorted.iter().map(|p| p.lens_make.as_str()).collect();
        makes.dedup();
        makes
    }
    pub fn models(&self, make: &str) -> Vec<&'p str> {
        let mut models: Vec<&str> = self
            .sorted
            .iter()
            .filter(|p| p.lens_make == make)
            .map(|p| p.lens_model.as_str())
            .collect();
        models.dedup();
        models
    }
    pub fn profiles(&self, make: &str, model: &str) -> Vec<&'p ImportedProfile> {
        self.sorted
            .iter()
            .copied()
            .filter(|p| p.lens_make == make && p.lens_model == model)
            .collect()
    }
    /// What picking a make chooses: its first model's first profile.
    pub fn first_of_make(&self, make: &str) -> Option<&'p ImportedProfile> {
        self.sorted.iter().copied().find(|p| p.lens_make == make)
    }
    /// What picking a model chooses: its first profile.
    pub fn first_of_model(&self, make: &str, model: &str) -> Option<&'p ImportedProfile> {
        self.profiles(make, model).first().copied()
    }
}

#[cfg(test)]
pub(crate) mod tests;
