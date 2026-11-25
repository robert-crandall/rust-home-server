use std::collections::HashMap;
use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::auth::User;

const MANIFEST_FILENAME: &str = "users.yaml";
const MAX_AVATAR_BYTES: u64 = 5 * 1024 * 1024;
const MAX_NAME_LEN: usize = 120;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HouseholdProfile {
    pub name: Option<String>,
    pub avatar: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvatarAsset {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
}

#[derive(Debug, Deserialize)]
struct ManifestFile {
    #[serde(default)]
    users: HashMap<String, ManifestUserEntry>,
}

#[derive(Debug, Deserialize)]
struct ManifestUserEntry {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    avatar: Option<String>,
}

#[derive(Default)]
struct CachedManifest {
    stamp: Option<(SystemTime, u64)>,
    entries: HashMap<String, HouseholdProfile>,
}

struct HouseholdProfilesInner {
    dir: PathBuf,
    cache: Mutex<CachedManifest>,
}

#[derive(Clone)]
pub struct HouseholdProfiles {
    inner: Arc<HouseholdProfilesInner>,
}

impl HouseholdProfiles {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            inner: Arc::new(HouseholdProfilesInner {
                dir,
                cache: Mutex::new(CachedManifest::default()),
            }),
        }
    }

    pub fn display_name_for(&self, email: &str) -> Option<String> {
        self.lookup_profile(email).and_then(|p| p.name)
    }

    pub fn apply_to_user(&self, user: &mut User) {
        let Some(profile) = self.lookup_profile(&user.email) else {
            user.avatar_url = None;
            return;
        };
        if let Some(name) = profile.name {
            user.name = name;
        }
        user.avatar_url = profile.avatar.as_deref().and_then(|filename| {
            let (path, _content_type) = self.resolve_avatar_path(filename)?;
            let meta = std::fs::metadata(&path).ok()?;
            if !meta.is_file() || meta.len() > MAX_AVATAR_BYTES {
                return None;
            }
            let version = meta
                .modified()
                .ok()
                .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs());
            Some(match version {
                Some(v) => format!("/api/users/{}/avatar?v={v}", user.id),
                None => format!("/api/users/{}/avatar", user.id),
            })
        });
    }

    pub fn load_avatar(&self, email: &str) -> Option<AvatarAsset> {
        let filename = self.lookup_profile(email)?.avatar?;
        let (path, content_type) = self.resolve_avatar_path(&filename)?;

        let file = File::open(&path).ok()?;
        let meta = file.metadata().ok()?;
        if !meta.is_file() || meta.len() > MAX_AVATAR_BYTES {
            return None;
        }

        let mut bytes = Vec::with_capacity(meta.len().min(MAX_AVATAR_BYTES) as usize);
        if file
            .take(MAX_AVATAR_BYTES + 1)
            .read_to_end(&mut bytes)
            .is_err()
        {
            return None;
        }
        if bytes.len() as u64 > MAX_AVATAR_BYTES {
            return None;
        }

        Some(AvatarAsset {
            bytes,
            content_type,
        })
    }

    fn lookup_profile(&self, email: &str) -> Option<HouseholdProfile> {
        let key = email.trim().to_ascii_lowercase();
        let manifest_path = self.inner.dir.join(MANIFEST_FILENAME);

        let mut guard = self.inner.cache.lock().unwrap_or_else(|e| e.into_inner());
        match std::fs::metadata(&manifest_path) {
            Ok(meta) => {
                let stamp = meta.modified().ok().map(|m| (m, meta.len()));
                if stamp.is_none() || stamp != guard.stamp {
                    match std::fs::read_to_string(&manifest_path) {
                        Ok(raw) => match serde_yaml_ng::from_str::<ManifestFile>(&raw) {
                            Ok(parsed) => {
                                guard.entries = normalize_manifest(parsed);
                                guard.stamp = stamp;
                            }
                            Err(err) => {
                                tracing::warn!(
                                    path = %manifest_path.display(),
                                    ?err,
                                    "failed to parse household users.yaml; keeping previous profiles"
                                );
                                guard.stamp = stamp;
                            }
                        },
                        Err(err) => {
                            tracing::warn!(
                                path = %manifest_path.display(),
                                ?err,
                                "failed to read household users.yaml; keeping previous profiles"
                            );
                        }
                    }
                }
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {
                guard.stamp = None;
                guard.entries.clear();
            }
            Err(err) => {
                tracing::warn!(
                    path = %manifest_path.display(),
                    ?err,
                    "failed to stat household users.yaml; keeping previous profiles"
                );
            }
        }

        guard.entries.get(&key).cloned()
    }

    fn resolve_avatar_path(&self, filename: &str) -> Option<(PathBuf, &'static str)> {
        let trimmed = filename.trim();
        let rel = Path::new(trimmed);
        let mut components = rel.components();
        let file_name = match (components.next(), components.next()) {
            (Some(Component::Normal(name)), None) => name,
            _ => return None,
        };

        let ext = Path::new(file_name)
            .extension()
            .and_then(|e| e.to_str())?
            .to_ascii_lowercase();
        let content_type = match ext.as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            "gif" => "image/gif",
            "avif" => "image/avif",
            _ => return None,
        };

        let canonical_dir = std::fs::canonicalize(&self.inner.dir).ok()?;
        let canonical_file = std::fs::canonicalize(canonical_dir.join(file_name)).ok()?;
        if !canonical_file.starts_with(&canonical_dir) {
            return None;
        }
        Some((canonical_file, content_type))
    }
}

fn normalize_manifest(manifest: ManifestFile) -> HashMap<String, HouseholdProfile> {
    let mut out = HashMap::with_capacity(manifest.users.len());
    for (raw_email, entry) in manifest.users {
        let email = raw_email.trim().to_ascii_lowercase();
        if email.is_empty() {
            continue;
        }
        let name = entry
            .name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty() && n.chars().count() <= MAX_NAME_LEN)
            .filter(|n| !n.chars().any(char::is_control))
            .map(ToString::to_string);
        let avatar = entry
            .avatar
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .map(ToString::to_string);
        out.insert(email, HouseholdProfile { name, avatar });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    fn sample_user(email: &str, name: &str) -> User {
        User {
            id: Uuid::new_v4(),
            email: email.to_string(),
            name: name.to_string(),
            avatar_url: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn loads_profiles_applies_overrides_and_hot_reloads() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("robert.png"), b"\x89PNG\r\n\x1a\nfake").unwrap();
        std::fs::write(
            dir.path().join("users.yaml"),
            "users:\n  Robert@Example.com:\n    name: Robert\n    avatar: robert.png\n",
        )
        .unwrap();

        let profiles = HouseholdProfiles::new(dir.path().to_path_buf());
        let mut user = sample_user("robert@example.com", "old-name");
        profiles.apply_to_user(&mut user);

        assert_eq!(user.name, "Robert");
        let avatar_url = user.avatar_url.as_deref().expect("avatar_url set");
        assert!(avatar_url.starts_with(&format!("/api/users/{}/avatar", user.id)));

        let asset = profiles
            .load_avatar("robert@example.com")
            .expect("avatar asset");
        assert_eq!(asset.content_type, "image/png");
        assert_eq!(asset.bytes, b"\x89PNG\r\n\x1a\nfake");

        // Update users.yaml (different size changes stamp immediately without sleep)
        std::fs::write(
            dir.path().join("users.yaml"),
            "users:\n  robert@example.com:\n    name: Bobby Crandall\n",
        )
        .unwrap();

        let mut updated = sample_user("robert@example.com", "old-name");
        profiles.apply_to_user(&mut updated);
        assert_eq!(updated.name, "Bobby Crandall");
        assert_eq!(updated.avatar_url, None);
    }

    #[test]
    fn rejects_path_traversal_svg_and_symlink_escapes() {
        let outer = tempfile::tempdir().expect("outer tempdir");
        let household_dir = outer.path().join("household");
        std::fs::create_dir(&household_dir).unwrap();

        let secret = outer.path().join("secret.png");
        std::fs::write(&secret, b"secret-bytes").unwrap();
        std::fs::write(household_dir.join("xss.svg"), b"<svg/>").unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink(&secret, household_dir.join("escape.png")).unwrap();

        std::fs::write(
            household_dir.join("users.yaml"),
            "users:\n  a@example.com:\n    avatar: ../secret.png\n  b@example.com:\n    avatar: xss.svg\n  c@example.com:\n    avatar: escape.png\n",
        )
        .unwrap();

        let profiles = HouseholdProfiles::new(household_dir);
        assert!(profiles.load_avatar("a@example.com").is_none());
        assert!(profiles.load_avatar("b@example.com").is_none());
        #[cfg(unix)]
        assert!(profiles.load_avatar("c@example.com").is_none());
    }

    #[test]
    fn keeps_previous_profiles_on_malformed_yaml_and_clears_when_removed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let yaml_path = dir.path().join("users.yaml");
        std::fs::write(&yaml_path, "users:\n  a@example.com:\n    name: Alice\n").unwrap();

        let profiles = HouseholdProfiles::new(dir.path().to_path_buf());
        assert_eq!(
            profiles.display_name_for("a@example.com").as_deref(),
            Some("Alice")
        );

        // Corrupt YAML keeps the last good map
        std::fs::write(&yaml_path, "users: [not: valid: yaml\n").unwrap();
        assert_eq!(
            profiles.display_name_for("a@example.com").as_deref(),
            Some("Alice")
        );

        // Removing users.yaml clears cached entries
        std::fs::remove_file(&yaml_path).unwrap();
        assert_eq!(profiles.display_name_for("a@example.com"), None);
    }
}
