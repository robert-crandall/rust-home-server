use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use base64::Engine as _;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::auth::User;

const MAX_AVATAR_BYTES: usize = 2 * 1024 * 1024;
const MAX_NAME_LEN: usize = 120;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvatarAsset {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HouseholdProfile {
    pub name: Option<String>,
    pub avatar: Option<Arc<AvatarAsset>>,
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
    path: PathBuf,
    cache: Mutex<CachedManifest>,
}

#[derive(Clone)]
pub struct HouseholdProfiles {
    inner: Arc<HouseholdProfilesInner>,
}

impl HouseholdProfiles {
    pub fn new(path: PathBuf) -> Self {
        Self {
            inner: Arc::new(HouseholdProfilesInner {
                path,
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
        user.avatar_url = profile
            .avatar
            .as_ref()
            .map(|avatar| format!("/api/users/{}/avatar?v={}", user.id, avatar.version));
    }

    pub fn load_avatar(&self, email: &str) -> Option<Arc<AvatarAsset>> {
        self.lookup_profile(email)?.avatar
    }

    fn lookup_profile(&self, email: &str) -> Option<HouseholdProfile> {
        let key = email.trim().to_ascii_lowercase();
        let manifest_path = &self.inner.path;

        let mut guard = self.inner.cache.lock().unwrap_or_else(|e| e.into_inner());
        match std::fs::metadata(manifest_path) {
            Ok(meta) => {
                let stamp = meta.modified().ok().map(|m| (m, meta.len()));
                if stamp.is_none() || stamp != guard.stamp {
                    match std::fs::read_to_string(manifest_path) {
                        Ok(raw) => match serde_yaml_ng::from_str::<ManifestFile>(&raw) {
                            Ok(parsed) => {
                                guard.entries = normalize_manifest(parsed);
                                guard.stamp = stamp;
                            }
                            Err(err) => {
                                tracing::warn!(
                                    path = %manifest_path.display(),
                                    ?err,
                                    "failed to parse household users YAML; keeping previous profiles"
                                );
                                guard.stamp = stamp;
                            }
                        },
                        Err(err) => {
                            tracing::warn!(
                                path = %manifest_path.display(),
                                ?err,
                                "failed to read household users YAML; keeping previous profiles"
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
                    "failed to stat household users YAML; keeping previous profiles"
                );
            }
        }

        guard.entries.get(&key).cloned()
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
        let avatar = entry.avatar.as_deref().and_then(|raw| {
            let decoded = decode_base64_avatar(raw);
            if decoded.is_none() && !raw.trim().is_empty() {
                tracing::warn!(
                    email = %email,
                    "ignoring invalid or unsupported base64 avatar in household users YAML"
                );
            }
            decoded.map(Arc::new)
        });
        out.insert(email, HouseholdProfile { name, avatar });
    }
    out
}

/// Decodes either a `data:image/<fmt>;base64,<payload>` data URI or raw base64-encoded
/// raster image (`png`, `jpeg`, `webp`, `gif`, `avif`), sniffing magic bytes so SVG and
/// non-image payloads are rejected.
fn decode_base64_avatar(raw: &str) -> Option<AvatarAsset> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (declared_mime, b64_part) = if let Some(rest) = trimmed.strip_prefix("data:") {
        let (meta, payload) = rest.split_once(',')?;
        let mime = meta.strip_suffix(";base64")?.trim().to_ascii_lowercase();
        let normalized_mime = match mime.as_str() {
            "image/png" => "image/png",
            "image/jpeg" | "image/jpg" => "image/jpeg",
            "image/webp" => "image/webp",
            "image/gif" => "image/gif",
            "image/avif" => "image/avif",
            _ => return None,
        };
        (Some(normalized_mime), payload)
    } else {
        (None, trimmed)
    };

    let compact: String = b64_part.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.is_empty() {
        return None;
    }
    // Reject oversized inputs before allocating decoded bytes.
    let max_b64_len = (MAX_AVATAR_BYTES.div_ceil(3)) * 4;
    if compact.len() > max_b64_len {
        return None;
    }

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&compact)
        .ok()?;
    if bytes.is_empty() || bytes.len() > MAX_AVATAR_BYTES {
        return None;
    }

    let sniffed_mime = sniff_raster_image_mime(&bytes)?;
    if let Some(declared) = declared_mime
        && declared != sniffed_mime
    {
        return None;
    }

    let digest = Sha256::digest(&bytes);
    let version = hex::encode(&digest[..6]);
    Some(AvatarAsset {
        bytes,
        content_type: sniffed_mime,
        version,
    })
}

fn sniff_raster_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12
        && &bytes[4..8] == b"ftyp"
        && (&bytes[8..12] == b"avif" || &bytes[8..12] == b"avis")
    {
        Some("image/avif")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    const PNG_BYTES: &[u8] = b"\x89PNG\r\n\x1a\nfake-png-payload";
    const WEBP_BYTES: &[u8] = b"RIFF\x10\x00\x00\x00WEBPfake-webp";

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
    fn loads_data_uri_and_raw_base64_avatars_and_hot_reloads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let yaml_path = dir.path().join("users.yaml");
        let png_b64 = base64::engine::general_purpose::STANDARD.encode(PNG_BYTES);
        let webp_b64 = base64::engine::general_purpose::STANDARD.encode(WEBP_BYTES);

        std::fs::write(
            &yaml_path,
            format!(
                "users:\n  Robert@Example.com:\n    name: Robert\n    avatar: \"data:image/png;base64,{png_b64}\"\n  alex@example.com:\n    name: Alex\n    avatar: |\n      {}\n      {}\n",
                &webp_b64[..8],
                &webp_b64[8..]
            ),
        )
        .unwrap();

        let profiles = HouseholdProfiles::new(yaml_path.clone());
        let mut robert = sample_user("robert@example.com", "old-name");
        profiles.apply_to_user(&mut robert);

        assert_eq!(robert.name, "Robert");
        let robert_url = robert.avatar_url.as_deref().expect("avatar_url set");
        assert!(robert_url.starts_with(&format!("/api/users/{}/avatar?v=", robert.id)));

        let robert_asset = profiles
            .load_avatar("robert@example.com")
            .expect("robert avatar");
        assert_eq!(robert_asset.content_type, "image/png");
        assert_eq!(robert_asset.bytes, PNG_BYTES);

        let alex_asset = profiles
            .load_avatar("alex@example.com")
            .expect("alex raw base64 avatar");
        assert_eq!(alex_asset.content_type, "image/webp");
        assert_eq!(alex_asset.bytes, WEBP_BYTES);

        // Update users.yaml (different size changes stamp immediately without sleep)
        std::fs::write(
            &yaml_path,
            "users:\n  robert@example.com:\n    name: Bobby Crandall\n",
        )
        .unwrap();

        let mut updated = sample_user("robert@example.com", "old-name");
        profiles.apply_to_user(&mut updated);
        assert_eq!(updated.name, "Bobby Crandall");
        assert_eq!(updated.avatar_url, None);
    }

    #[test]
    fn rejects_svg_mismatched_mime_and_non_image_base64() {
        let svg_b64 = base64::engine::general_purpose::STANDARD.encode(b"<svg><script/></svg>");
        let png_b64 = base64::engine::general_purpose::STANDARD.encode(PNG_BYTES);
        let text_b64 = base64::engine::general_purpose::STANDARD.encode(b"not an image");

        assert!(decode_base64_avatar(&format!("data:image/svg+xml;base64,{svg_b64}")).is_none());
        assert!(decode_base64_avatar(&svg_b64).is_none());
        assert!(decode_base64_avatar(&format!("data:image/jpeg;base64,{png_b64}")).is_none());
        assert!(decode_base64_avatar(&text_b64).is_none());
        assert!(decode_base64_avatar("not-valid-base64!!!").is_none());
    }

    #[test]
    fn keeps_previous_profiles_on_malformed_yaml_and_clears_when_removed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let yaml_path = dir.path().join("users.yaml");
        std::fs::write(&yaml_path, "users:\n  a@example.com:\n    name: Alice\n").unwrap();

        let profiles = HouseholdProfiles::new(yaml_path.clone());
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
