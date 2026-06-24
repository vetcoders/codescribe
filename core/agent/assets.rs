use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use rand::distributions::Alphanumeric;
use rand::{Rng, thread_rng};

use super::types::ImageAsset;

pub struct AgentAssetStore;

impl AgentAssetStore {
    pub fn assets_dir() -> PathBuf {
        crate::config::Config::config_dir().join("assets")
    }

    pub fn save_image(data: &[u8], media_type: &str) -> Result<ImageAsset> {
        let dir = Self::assets_dir();
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create agent assets dir: {}", dir.display()))?;

        let extension = extension_for_media_type(media_type);
        let asset_id = format!(
            "screenshot_{}_{}",
            Utc::now().format("%Y%m%d_%H%M%S_%3f"),
            random_suffix(6)
        );
        let file_name = format!("{asset_id}.{extension}");
        let path = dir.join(file_name);
        atomic_write(&path, data)?;

        Ok(ImageAsset {
            asset_id,
            path,
            media_type: normalize_media_type(media_type),
            size_bytes: u64::try_from(data.len()).expect("usize length should fit in u64"),
        })
    }

    /// Read an asset previously stored by `save_image`. Asset paths travel
    /// through conversation state, so the requested path is never handed to
    /// the filesystem: only its file name is matched against the actual
    /// entries of the canonical assets dir, and the read uses the enumerated
    /// entry's own path. Directory components (including any `..`) cannot
    /// influence what gets opened.
    pub fn read_image(path: &Path) -> Result<Vec<u8>> {
        let requested = path
            .file_name()
            .with_context(|| format!("Asset path has no file name: {}", path.display()))?
            .to_owned();
        let dir = Self::assets_dir();
        let entries = std::fs::read_dir(&dir)
            .with_context(|| format!("Failed to list agent assets dir {}", dir.display()))?;
        for entry in entries {
            let entry = entry.context("Failed to enumerate agent assets dir")?;
            if entry.file_name() == requested {
                let contained = entry.path();
                return std::fs::read(&contained).with_context(|| {
                    format!("Failed to read image asset {}", contained.display())
                });
            }
        }
        anyhow::bail!(
            "Unknown image asset: {} (not present in {})",
            requested.to_string_lossy(),
            dir.display()
        )
    }
}

fn normalize_media_type(media_type: &str) -> String {
    let media_type = media_type.trim();
    if media_type.is_empty() {
        "image/png".to_string()
    } else {
        media_type.to_string()
    }
}

fn extension_for_media_type(media_type: &str) -> &'static str {
    match media_type.trim().to_ascii_lowercase().as_str() {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "png",
    }
}

fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data)
        .with_context(|| format!("Failed to write temporary asset {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("Failed to store agent asset {}", path.display()))?;
    Ok(())
}

fn random_suffix(len: usize) -> String {
    thread_rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect::<String>()
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_for_media_type_defaults_to_png() {
        assert_eq!(extension_for_media_type("image/png"), "png");
        assert_eq!(extension_for_media_type(""), "png");
        assert_eq!(extension_for_media_type("image/jpeg"), "jpg");
    }

    #[test]
    fn read_image_strips_directory_components() {
        // A traversal-shaped path must never reach the attacker-chosen
        // location: only the leaf is matched against real assets-dir
        // entries, so /etc/passwd is unreachable by construction.
        let err = AgentAssetStore::read_image(Path::new("../../etc/passwd"))
            .expect_err("uncontained read must not succeed");
        let msg = err.to_string();
        assert!(
            msg.contains("Unknown image asset: passwd") || msg.contains("assets dir"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn read_image_roundtrips_saved_asset() {
        let asset = AgentAssetStore::save_image(b"png-bytes", "image/png")
            .expect("save_image should succeed");
        let data = AgentAssetStore::read_image(&asset.path).expect("saved asset must be readable");
        assert_eq!(data, b"png-bytes");
        std::fs::remove_file(&asset.path).ok();
    }

    #[test]
    fn read_image_rejects_paths_without_file_name() {
        let err = AgentAssetStore::read_image(Path::new("/tmp/.."))
            .expect_err("path without file name must be rejected");
        assert!(err.to_string().contains("no file name"));
    }
}
