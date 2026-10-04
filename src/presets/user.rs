//! Presets made in RAWmakase: XMP files in the user library's "User Presets" folder,
//! one folder per group, which Lightroom and Camera Raw can read too. Only these are
//! updated, renamed or deleted from the app; imported and built-in presets are not.
use crate::{
    develop::{
        Recipe,
        settings_groups::{GroupInclusion, GroupSelection},
    },
    storage::Replace,
    xmp::{
        Preset,
        preset_write::{PresetInfo, group_of_key, preset},
    },
};
use anyhow::{Context, Result, ensure};
use std::path::{Path, PathBuf};

/// The folder of presets made in RAWmakase, under the user library.
#[derive(Clone, Debug, PartialEq)]
pub struct UserPresets {
    pub dir: PathBuf,
}

impl Default for UserPresets {
    fn default() -> Self {
        Self {
            dir: crate::storage::data_dir()
                .join("xmp-presets")
                .join("User Presets"),
        }
    }
}

impl UserPresets {
    /// Whether `preset` was made in RAWmakase, so it may be changed here.
    /// Only files RAWmakase wrote count: they carry its marker, so a preset copied
    /// into the folder from elsewhere is never updated or deleted here.
    pub fn owns(&self, preset: &Preset) -> bool {
        preset.path.starts_with(&self.dir)
            && preset
                .settings
                .get("RAWmakasePreset")
                .is_some_and(|v| v == "1")
    }
    /// Saves a new preset of `r`'s `groups`; returns its file.
    pub fn create(
        &self,
        r: &Recipe,
        info: &PresetInfo,
        groups: &GroupSelection,
    ) -> Result<PathBuf> {
        ensure!(!info.name.is_empty(), "A preset needs a name");
        ensure!(!groups.is_empty(), "Choose at least one setting");
        let path = self.file_for(info)?;
        write(&path, Replace::NoClobber, &preset(r, info, groups)).with_context(|| {
            format!("A preset named {} is already in {}", info.name, info.group)
        })?;
        Ok(path)
    }
    /// Lightroom's Update with Current Settings: the preset keeps its name, group,
    /// identity and the setting groups it had.
    pub fn update(&self, existing: &Preset, r: &Recipe) -> Result<()> {
        ensure!(self.owns(existing), "Only presets made here can be updated");
        let info = info_of(existing);
        write(
            &existing.path,
            Replace::Overwrite,
            &preset(r, &info, &groups_of(existing)),
        )
    }
    /// Renames a preset, keeping its settings and identity; returns its new file.
    /// Only presets written here are renamed, so their Name element is ours.
    pub fn rename(&self, existing: &Preset, name: &str) -> Result<PathBuf> {
        ensure!(self.owns(existing), "Only presets made here can be renamed");
        let info = PresetInfo {
            name: name.trim().to_string(),
            ..info_of(existing)
        };
        ensure!(!info.name.is_empty(), "A preset needs a name");
        let text = std::fs::read_to_string(&existing.path)?;
        let element = |name: &str| {
            format!(
                "<crs:Name>\n    <rdf:Alt>\n     <rdf:li xml:lang=\"x-default\">{}</rdf:li>",
                crate::xmp::xml::escape_text(name)
            )
        };
        let old = element(&existing.name);
        ensure!(
            text.contains(&old),
            "This preset's name can't be changed here"
        );
        let path = self.file_for(&info)?;
        let renamed = text.replacen(&old, &element(&info.name), 1);
        // The same file whatever the case of its letters, as on macOS and Windows:
        // write it in place, then let the rename fix the case.
        // Only when both names lead to this very file, as on a case-insensitive file
        // system; on Linux they can be two presets, and the other must not go.
        let resolved = |p: &Path| std::fs::canonicalize(p).ok();
        let same_file = path == existing.path
            || resolved(&path).is_some_and(|p| Some(p) == resolved(&existing.path));
        if same_file {
            write(&existing.path, Replace::Overwrite, &renamed)?;
            std::fs::rename(&existing.path, &path)?;
        } else {
            write(&path, Replace::NoClobber, &renamed).with_context(|| {
                format!("A preset named {} is already in {}", info.name, info.group)
            })?;
            std::fs::remove_file(&existing.path)?;
        }
        Ok(path)
    }
    /// Deletes a preset made here.
    pub fn delete(&self, existing: &Preset) -> Result<()> {
        ensure!(self.owns(existing), "Only presets made here can be deleted");
        std::fs::remove_file(&existing.path)?;
        Ok(())
    }
    /// `dir/<group>/<name>.xmp`, with characters file systems refuse replaced.
    fn file_for(&self, info: &PresetInfo) -> Result<PathBuf> {
        let group = match file_name(&info.group) {
            g if g.is_empty() => "User Presets".to_string(),
            g => g,
        };
        let name = file_name(&info.name);
        ensure!(!name.is_empty(), "A preset needs a name");
        Ok(self.dir.join(group).join(format!("{name}.xmp")))
    }
}

/// `s` as a file name, with the characters and names Windows, macOS or Linux refuse
/// in one replaced.
pub(crate) fn file_name(s: &str) -> String {
    let name: String = s
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() {
                '-'
            } else {
                c
            }
        })
        .collect();
    let name = name.trim_start_matches('.').trim_end_matches(['.', ' ']);
    let reserved = ["CON", "PRN", "AUX", "NUL"].iter().any(|r| {
        name.split('.')
            .next()
            .is_some_and(|n| n.eq_ignore_ascii_case(r))
    }) || ["COM", "LPT"].iter().any(|r| {
        let stem = name.split('.').next().unwrap_or("");
        let stem = stem.as_bytes();
        stem.len() == 4 && stem[..3].eq_ignore_ascii_case(r.as_bytes()) && stem[3].is_ascii_digit()
    });
    if reserved {
        format!("_{name}")
    } else {
        name.to_string()
    }
}

/// The setting groups a preset holds, from the keys it sets.
pub fn groups_of(preset: &Preset) -> GroupSelection {
    let mut groups = GroupSelection::none();
    // Panel switches go with whichever groups were chosen, so they say nothing here.
    for key in preset
        .settings
        .keys()
        .chain(preset.curves.keys())
        .filter(|k| !k.starts_with("Enable"))
    {
        if let Some(group) = group_of_key(key) {
            groups.set(group, GroupInclusion::Included);
        }
    }
    groups
}

fn info_of(p: &Preset) -> PresetInfo {
    PresetInfo {
        name: p.name.clone(),
        group: p.group.clone(),
        uuid: p
            .settings
            .get("UUID")
            .cloned()
            .unwrap_or_else(|| PresetInfo::new(&p.name, &p.group).uuid),
    }
}

fn write(path: &Path, replace: Replace, text: &str) -> Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(crate::storage::parent_dir(path))?;
    crate::storage::write_atomic(path, replace, |f| Ok(f.write_all(text.as_bytes())?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library(dir: &Path) -> crate::presets::Library {
        let mut presets = Vec::new();
        for entry in walk(dir) {
            presets.push(
                crate::xmp::parse(&entry, &std::fs::read_to_string(&entry).unwrap()).unwrap(),
            );
        }
        crate::presets::Library {
            presets,
            errors: Vec::new(),
        }
    }
    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
        out
    }

    #[test]
    fn presets_made_here_are_created_updated_renamed_and_deleted() -> Result<()> {
        let d = tempfile::tempdir()?;
        let user = UserPresets {
            dir: d.path().join("User Presets"),
        };
        let mut groups = GroupSelection::none();
        groups.set(
            crate::develop::settings_groups::SettingGroup::Exposure,
            GroupInclusion::Included,
        );
        let r = Recipe {
            exposure: 0.4,
            ..Default::default()
        };
        let info = PresetInfo::new("Bright/airy", "Mine");
        let path = user.create(&r, &info, &groups)?;
        assert_eq!(path, user.dir.join("Mine").join("Bright-airy.xmp"));
        // The same name in the same group is refused.
        assert!(user.create(&r, &info, &groups).is_err());
        assert!(
            user.create(
                &r,
                &PresetInfo::new("Empty", "Mine"),
                &GroupSelection::none()
            )
            .is_err()
        );
        let made = library(&user.dir).presets.remove(0);
        assert!(user.owns(&made));
        assert_eq!(made.name, "Bright/airy");
        // The panel switches written with it don't count as groups of their own.
        assert_eq!(groups_of(&made), groups);
        // Update keeps the groups it had: still only Exposure.
        user.update(
            &made,
            &Recipe {
                exposure: 1.,
                contrast: 0.5,
                ..Default::default()
            },
        )?;
        let updated = library(&user.dir).presets.remove(0);
        assert_eq!(updated.settings["Exposure2012"], "+1.00");
        assert!(!updated.settings.contains_key("Contrast2012"));
        assert_eq!(updated.settings["UUID"], info.uuid);
        // Rename moves the file and keeps the rest.
        let renamed = user.rename(&updated, "Brighter")?;
        let presets = library(&user.dir).presets;
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].name, "Brighter");
        assert_eq!(presets[0].path, renamed);
        assert_eq!(presets[0].settings["UUID"], info.uuid);
        user.delete(&presets[0])?;
        assert!(library(&user.dir).presets.is_empty());
        // A case-only rename keeps the one file.
        let created = user.create(&r, &PresetInfo::new("lower", "Mine"), &groups)?;
        let lower = library(&user.dir).presets.remove(0);
        let upper = user.rename(&lower, "Lower")?;
        assert_eq!(library(&user.dir).presets.len(), 1);
        assert_eq!(library(&user.dir).presets[0].name, "Lower");
        assert_ne!(created, upper);
        user.delete(&library(&user.dir).presets[0])?;
        // Windows-unsafe names are made safe.
        let odd = user.create(&r, &PresetInfo::new("x? <b>.", "a|b"), &groups)?;
        assert_eq!(odd, user.dir.join("a-b").join("x- -b-.xmp"));
        let reserved = user.create(&r, &PresetInfo::new("CON", "a|b"), &groups)?;
        assert_eq!(reserved, user.dir.join("a-b").join("_CON.xmp"));
        // Names of four bytes but fewer characters are names like any other.
        for name in ["éé", "🙂", "COMé"] {
            let made = user.create(&r, &PresetInfo::new(name, "a|b"), &groups)?;
            assert_eq!(made, user.dir.join("a-b").join(format!("{name}.xmp")));
        }
        let com = user.create(&r, &PresetInfo::new("com1.x", "a|b"), &groups)?;
        assert_eq!(com, user.dir.join("a-b").join("_com1.x.xmp"));
        // A file copied into the folder from elsewhere is not ours to change.
        let foreign = crate::xmp::Preset {
            settings: Default::default(),
            ..library(&user.dir).presets[0].clone()
        };
        assert!(!user.owns(&foreign));
        // Presets that aren't ours are left alone.
        let elsewhere = crate::xmp::Preset {
            path: d.path().join("Imported").join("x.xmp"),
            ..presets[0].clone()
        };
        assert!(user.delete(&elsewhere).is_err());
        Ok(())
    }
}
