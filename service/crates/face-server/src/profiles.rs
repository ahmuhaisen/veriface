//! Profiles loaded at startup from `paths.profiles` (`*.yaml` / `*.yml`). Each one is validated
//! with [`face_core::Profile::from_yaml`]; its `ProfileRef.sha256` is the SHA-256 of the file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use face_core::{Catalogue, ImageFormat, Profile};
use face_imaging::codec::{DecodeLimits, Format};

use crate::auth::sha256_hex;
use crate::dto::ProfileRef;

/// A validated profile plus what the server derives from it.
#[derive(Debug)]
pub struct LoadedProfile {
    pub profile: Profile,
    pub reference: ProfileRef,
    /// `input.maxBytes`, `input.maxPixels` and `input.formats`, ready for `codec::decode`.
    pub decode_limits: DecodeLimits,
    /// Where it was loaded from (for logs).
    pub source: String,
}

impl LoadedProfile {
    pub fn name(&self) -> &str {
        &self.profile.name
    }

    fn new(profile: Profile, bytes: &[u8], source: String) -> Self {
        let formats: Vec<Format> = profile
            .input
            .formats
            .iter()
            .map(|f| match f {
                ImageFormat::Jpeg => Format::Jpeg,
                ImageFormat::Png => Format::Png,
                ImageFormat::Webp => Format::Webp,
            })
            .collect();
        let max_bytes = usize::try_from(profile.input.max_bytes).unwrap_or(usize::MAX);
        LoadedProfile {
            reference: ProfileRef {
                name: profile.name.clone(),
                version: profile.version.clone(),
                sha256: sha256_hex(bytes),
                description: profile.description.clone(),
            },
            decode_limits: DecodeLimits::new(max_bytes, profile.input.max_pixels, &formats),
            profile,
            source,
        }
    }
}

/// All loaded profiles, by name.
#[derive(Debug, Default)]
pub struct ProfileStore {
    profiles: BTreeMap<String, Arc<LoadedProfile>>,
}

impl ProfileStore {
    /// Load every `*.yaml` / `*.yml` file in `dir`. Reports every problem, not just the first.
    pub fn load_dir(dir: &Path) -> Result<ProfileStore, Vec<String>> {
        let entries = std::fs::read_dir(dir).map_err(|e| {
            vec![format!(
                "cannot read profiles directory `{}`: {e}",
                dir.display()
            )]
        })?;
        let mut files: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.is_file()
                    && p.extension().and_then(|x| x.to_str()).is_some_and(|x| {
                        x.eq_ignore_ascii_case("yaml") || x.eq_ignore_ascii_case("yml")
                    })
            })
            .collect();
        files.sort();
        if files.is_empty() {
            return Err(vec![format!(
                "no profile files (*.yaml) in `{}`",
                dir.display()
            )]);
        }
        let mut sources = Vec::new();
        let mut problems = Vec::new();
        for file in files {
            match std::fs::read(&file) {
                Ok(bytes) => sources.push((file.display().to_string(), bytes)),
                Err(e) => problems.push(format!("cannot read `{}`: {e}", file.display())),
            }
        }
        match ProfileStore::from_sources(sources) {
            Ok(store) if problems.is_empty() => Ok(store),
            Ok(_) => Err(problems),
            Err(more) => {
                problems.extend(more);
                Err(problems)
            }
        }
    }

    /// Build from `(source label, file bytes)` pairs.
    pub fn from_sources(
        sources: impl IntoIterator<Item = (String, Vec<u8>)>,
    ) -> Result<ProfileStore, Vec<String>> {
        let mut problems = Vec::new();
        let mut profiles: BTreeMap<String, Arc<LoadedProfile>> = BTreeMap::new();
        for (source, bytes) in sources {
            let text = match std::str::from_utf8(&bytes) {
                Ok(text) => text,
                Err(_) => {
                    problems.push(format!("{source}: not UTF-8"));
                    continue;
                }
            };
            let profile = match Profile::from_yaml(text, Catalogue::embedded()) {
                Ok(p) => p,
                Err(e) => {
                    problems.push(format!("{source}: {e}"));
                    continue;
                }
            };
            let loaded = LoadedProfile::new(profile, &bytes, source);
            if let Some(existing) = profiles.get(loaded.name()) {
                problems.push(format!(
                    "{}: profile name `{}` is already defined in {}",
                    loaded.source,
                    loaded.name(),
                    existing.source
                ));
                continue;
            }
            profiles.insert(loaded.name().to_owned(), Arc::new(loaded));
        }
        if problems.is_empty() {
            Ok(ProfileStore { profiles })
        } else {
            Err(problems)
        }
    }

    pub fn get(&self, name: &str) -> Option<&Arc<LoadedProfile>> {
        self.profiles.get(name)
    }

    /// In name order.
    pub fn iter(&self) -> impl Iterator<Item = &Arc<LoadedProfile>> {
        self.profiles.values()
    }

    pub fn len(&self) -> usize {
        self.profiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }

    /// The largest `input.maxBytes` of any profile.
    pub fn max_input_bytes(&self) -> usize {
        self.iter()
            .map(|p| p.decode_limits.max_bytes)
            .max()
            .unwrap_or(0)
    }

    /// The shipped `standard` and `icao-strict` profiles (unit tests).
    #[cfg(test)]
    pub(crate) fn for_tests() -> ProfileStore {
        ProfileStore::from_sources([
            (
                "standard.yaml".to_owned(),
                include_bytes!("../../../../profiles/standard.yaml").to_vec(),
            ),
            (
                "icao-strict.yaml".to_owned(),
                include_bytes!("../../../../profiles/icao-strict.yaml").to_vec(),
            ),
        ])
        .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_the_shipped_profiles_directory() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../profiles");
        let store = ProfileStore::load_dir(&dir).unwrap();
        let standard = store.get("standard").unwrap();
        assert_eq!(standard.reference.version, "0.1.0");
        assert_eq!(standard.reference.sha256.len(), 64);
        assert_eq!(standard.decode_limits.max_bytes, 10 * 1024 * 1024);
        assert_eq!(standard.decode_limits.formats, Format::ALL.to_vec());
        assert!(store.get("icao-strict").is_some());
        assert_eq!(store.max_input_bytes(), 10 * 1024 * 1024);
    }

    #[test]
    fn reports_all_invalid_files_and_duplicates() {
        let standard = include_bytes!("../../../../profiles/standard.yaml").to_vec();
        let broken = String::from_utf8(standard.clone())
            .unwrap()
            .replace("HEAD_POSE_YAW", "HEAD_POSE_YAWN")
            .into_bytes();
        let problems = ProfileStore::from_sources([
            ("a.yaml".to_owned(), standard.clone()),
            ("b.yaml".to_owned(), standard),
            ("c.yaml".to_owned(), broken),
            ("d.yaml".to_owned(), vec![0xFF, 0xFE]),
        ])
        .unwrap_err();
        assert_eq!(problems.len(), 3, "{problems:#?}");
        assert!(problems[0].contains("already defined in a.yaml"));
    }

    #[test]
    fn empty_directory_is_an_error() {
        let dir = std::env::temp_dir().join(format!("veriface-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(ProfileStore::load_dir(&dir).is_err());
        let _ = std::fs::remove_dir(&dir);
    }
}
