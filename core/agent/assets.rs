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
}
