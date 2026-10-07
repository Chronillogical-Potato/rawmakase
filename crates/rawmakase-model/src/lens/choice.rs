//! Which Adobe lens profile a photo uses: Lightroom's Lens Corrections › Profile
//! Setup, and the profile an edit names (`crs:LensProfileSetup`, `LensProfileName`,
//! `LensProfileFilename`, `LensProfileDigest`).
use crate::camera_data::Metadata;
use crate::optics::lcp::{Candidate, ImportedProfile, PhotoProfiles};
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
    /// `crs:LensProfileIsEmbedded`: the profile is the one the RAW carries (Adobe
    /// names it "Camera Settings"), so the built-in correction renders, not an LCP.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub embedded: bool,
}
impl LensProfileId {
    pub fn of(profile: &ImportedProfile) -> Self {
        Self {
            name: profile.name.clone(),
            filename: profile.filename.clone(),
            digest: String::new(),
            embedded: false,
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
            // The camera's own profile is not an imported file, whatever its name.
            .filter(|id| !id.embedded && profile.is(&id.filename, &id.name))
            // A name alone may fit several files: record the one picked.
            .map(|id| LensProfileId {
                filename: profile.filename.clone(),
                ..id
            })
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
        if self.id.as_ref().is_some_and(|id| id.embedded) {
            return Resolved::default();
        }
        let named = self
            .id
            .as_ref()
            .map(|id| (id, profiles.find(&id.filename, &id.name)));
        match (self.setup, named) {
            // Found but unable to correct this photo, it is reported as not imported is.
            (LensProfileSetup::Custom, Some((id, Some(found)))) => match found.correction(m) {
                Some(_) => Resolved {
                    used: Some(found),
                    missing: None,
                },
                None => Resolved {
                    used: None,
                    missing: Some(id),
                },
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
                // A named profile that isn't imported, or can't correct this photo,
                // stays named and is reported.
                missing: named.and_then(|(id, found)| {
                    found
                        .is_none_or(|c| c.correction(m).is_none())
                        .then_some(id)
                }),
            },
        }
    }
}

/// Lightroom's Make, Model and Profile menus over a photo's profiles.
/// Profiles whose correction does not validate for the photo are left out of the
/// Profile menu and never chosen; they are checked as the menus need them, since a
/// full Adobe library offers thousands.
pub struct ProfileMenus<'p> {
    profiles: &'p PhotoProfiles,
    photo: &'p Metadata,
}
impl<'p> ProfileMenus<'p> {
    pub fn new(profiles: &'p PhotoProfiles, photo: &'p Metadata) -> Self {
        Self { profiles, photo }
    }
    fn sorted(&self) -> impl Iterator<Item = &'p ImportedProfile> + 'p {
        self.profiles.in_menu_order().map(|c| c.profile.as_ref())
    }
    pub fn makes(&self) -> Vec<&'p str> {
        let mut makes: Vec<&str> = self.sorted().map(|p| p.lens_make.as_str()).collect();
        makes.dedup();
        makes
    }
    /// The make's models with a profile that validates; only the chosen make's
    /// profiles are checked, so opening the menu stays quick.
    pub fn models(&self, make: &str) -> Vec<&'p str> {
        let photo = self.photo;
        let mut models: Vec<&str> = self
            .profiles
            .in_menu_order()
            .filter(|c| c.profile.lens_make == make && c.correction(photo).is_some())
            .map(|c| c.profile.lens_model.as_str())
            .collect();
        models.dedup();
        models
    }
    pub fn profiles(&self, make: &str, model: &str) -> Vec<&'p ImportedProfile> {
        let photo = self.photo;
        self.profiles
            .in_menu_order()
            .filter(|c| c.profile.lens_make == make && c.profile.lens_model == model)
            .filter(|c| c.correction(photo).is_some())
            .map(|c| c.profile.as_ref())
            .collect()
    }
    /// What picking a make chooses: its first model's first profile.
    pub fn first_of_make(&self, make: &str) -> Option<&'p ImportedProfile> {
        let photo = self.photo;
        self.profiles
            .in_menu_order()
            .filter(|c| c.profile.lens_make == make)
            .find(|c| c.correction(photo).is_some())
            .map(|c| c.profile.as_ref())
    }
    /// What picking a model chooses: its first profile.
    pub fn first_of_model(&self, make: &str, model: &str) -> Option<&'p ImportedProfile> {
        self.profiles(make, model).first().copied()
    }
}

#[cfg(any(test, feature = "test-support"))]
#[cfg_attr(not(test), allow(dead_code))]
pub mod tests;
