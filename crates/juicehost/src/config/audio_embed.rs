use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::ConfigError;

/// Settings for dynamically generated video previews of audio files.
///
/// Discord does not render audio attachments in link embeds. For audio files
/// we generate a static video card that contains the original audio and
/// metadata, which Discord can render with its normal video player.
#[derive(Debug, Deserialize, Serialize)]
pub struct AudioEmbedFile {
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Maximum length of the generated clip, in seconds. Zero means the full track.
    #[serde(default = "default_max_seconds")]
    pub max_seconds: u64,

    /// Audio files larger than this are left to the normal audio embed.
    #[serde(default = "default_max_source_mb")]
    pub max_source_mb: u64,

    /// Directory for generated MP4/JPG/JSON cache entries.
    #[serde(default = "default_cache_dir")]
    pub cache_dir: PathBuf,

    /// Soft cap for the generated embed cache.
    #[serde(default = "default_cache_max_mb")]
    pub cache_max_mb: u64,

    #[serde(default = "default_width")]
    pub width: u32,

    #[serde(default = "default_height")]
    pub height: u32,

    /// Number of ffmpeg jobs that may run at once.
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: usize,

    /// Hard timeout for a single generation job.
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,

    #[serde(default = "default_ffmpeg_bin")]
    pub ffmpeg_bin: String,
}

impl Default for AudioEmbedFile {
    fn default() -> Self {
        Self {
            enabled: true,
            max_seconds: default_max_seconds(),
            max_source_mb: default_max_source_mb(),
            cache_dir: default_cache_dir(),
            cache_max_mb: default_cache_max_mb(),
            width: default_width(),
            height: default_height(),
            max_concurrent: default_max_concurrent(),
            timeout_seconds: default_timeout_seconds(),
            ffmpeg_bin: default_ffmpeg_bin(),
        }
    }
}

const fn default_true() -> bool {
    true
}
const fn default_max_seconds() -> u64 {
    0
}
const fn default_max_source_mb() -> u64 {
    256
}
fn default_cache_dir() -> PathBuf {
    PathBuf::from("./audio-embeds")
}
const fn default_cache_max_mb() -> u64 {
    512
}
const fn default_width() -> u32 {
    640
}
const fn default_height() -> u32 {
    360
}
const fn default_max_concurrent() -> usize {
    2
}
const fn default_timeout_seconds() -> u64 {
    120
}
fn default_ffmpeg_bin() -> String {
    String::from("ffmpeg")
}

#[derive(Debug, Clone)]
pub struct AudioEmbedSettings {
    enabled: bool,
    max_seconds: u64,
    max_source_bytes: u64,
    cache_dir: PathBuf,
    cache_max_bytes: u64,
    width: u32,
    height: u32,
    max_concurrent: usize,
    timeout_seconds: u64,
    ffmpeg_bin: String,
}

impl AudioEmbedSettings {
    pub fn load(file: &AudioEmbedFile) -> Result<Self, ConfigError> {
        let max_source_bytes = file.max_source_mb.saturating_mul(1024 * 1024);
        let cache_max_bytes = file.cache_max_mb.saturating_mul(1024 * 1024);
        let valid = !file.ffmpeg_bin.trim().is_empty()
            && !file.cache_dir.as_os_str().is_empty()
            && (0..=86_400).contains(&file.max_seconds)
            && (1..=4096).contains(&file.max_source_mb)
            && (16..=8192).contains(&file.cache_max_mb)
            && (64..=1920).contains(&file.width)
            && file.width.is_multiple_of(2)
            && (64..=1080).contains(&file.height)
            && file.height.is_multiple_of(2)
            && (1..=4).contains(&file.max_concurrent)
            && (5..=600).contains(&file.timeout_seconds);
        if !valid {
            return Err(ConfigError::InvalidAudioEmbedConfiguration);
        }
        Ok(Self {
            enabled: file.enabled,
            max_seconds: file.max_seconds,
            max_source_bytes,
            cache_dir: file.cache_dir.clone(),
            cache_max_bytes,
            width: file.width,
            height: file.height,
            max_concurrent: file.max_concurrent,
            timeout_seconds: file.timeout_seconds,
            ffmpeg_bin: file.ffmpeg_bin.trim().to_string(),
        })
    }

    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    #[must_use]
    pub const fn max_seconds(&self) -> u64 {
        self.max_seconds
    }

    #[must_use]
    pub const fn max_source_bytes(&self) -> u64 {
        self.max_source_bytes
    }

    #[must_use]
    pub fn cache_dir(&self) -> &PathBuf {
        &self.cache_dir
    }

    #[must_use]
    pub const fn cache_max_bytes(&self) -> u64 {
        self.cache_max_bytes
    }

    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    #[must_use]
    pub const fn max_concurrent(&self) -> usize {
        self.max_concurrent
    }

    #[must_use]
    pub const fn timeout_seconds(&self) -> u64 {
        self.timeout_seconds
    }

    #[must_use]
    pub fn ffmpeg_bin(&self) -> &str {
        &self.ffmpeg_bin
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        let settings = AudioEmbedSettings::load(&AudioEmbedFile::default()).expect("valid");
        assert!(settings.enabled());
        assert_eq!(settings.max_seconds(), 0);
        assert_eq!(settings.width(), 640);
        assert_eq!(settings.height(), 360);
    }

    #[test]
    fn rejects_odd_dimensions() {
        let file = AudioEmbedFile {
            width: 641,
            ..AudioEmbedFile::default()
        };
        assert!(AudioEmbedSettings::load(&file).is_err());
    }
}
