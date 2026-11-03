//! Préférences persistantes, stockées en JSON dans le dossier de données.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const CONFIG_FILE: &str = "config.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Télécharge et installe automatiquement les nouvelles versions de FFmpeg.
    pub auto_update: bool,
    /// Horodatage Unix de la dernière vérification de mise à jour.
    pub last_update_check: u64,
    /// Version de FFmpeg installée par VideoCut (nom du sous-dossier géré).
    pub ffmpeg_version: Option<String>,
    /// `ffmpeg.exe` choisi à la main, prioritaire sur tout le reste.
    pub custom_ffmpeg: Option<PathBuf>,
    pub volume_percent: u32,
    /// `true` : coupe précise (réencodage), `false` : copie rapide.
    pub precise_export: bool,
    pub video_height: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            auto_update: true,
            last_update_check: 0,
            ffmpeg_version: None,
            custom_ffmpeg: None,
            volume_percent: 100,
            precise_export: true,
            video_height: 230.0,
        }
    }
}

impl Config {
    pub(crate) fn load(dir: &Path) -> Self {
        fs::read_to_string(dir.join(CONFIG_FILE))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Écriture atomique : fichier temporaire puis renommage.
    pub(crate) fn save(&self, dir: &Path) -> Result<(), String> {
        fs::create_dir_all(dir).map_err(|err| format!("Dossier {}: {err}", dir.display()))?;
        let text = serde_json::to_string_pretty(self).map_err(|err| err.to_string())?;
        let tmp = dir.join(format!("{CONFIG_FILE}.tmp"));
        fs::write(&tmp, text).map_err(|err| format!("Écriture de {}: {err}", tmp.display()))?;
        crate::fsutil::rename_with_retry(&tmp, &dir.join(CONFIG_FILE)).map_err(|err| err.to_string())
    }

    pub(crate) fn update_check_due(&self, now: u64) -> bool {
        const DAY: u64 = 24 * 60 * 60;
        now.saturating_sub(self.last_update_check) >= DAY
    }
}

/// Dossier de données de VideoCut (`%LOCALAPPDATA%\VideoCut` sous Windows).
pub fn data_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    base.filter(|path| path.is_absolute()).map(|path| path.join("VideoCut"))
}

pub(crate) fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("videocut-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = temp_dir("config");
        let config = Config {
            auto_update: false,
            ffmpeg_version: Some("9.0.2".into()),
            volume_percent: 150,
            ..Config::default()
        };
        config.save(&dir).unwrap();
        assert_eq!(Config::load(&dir), config);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_or_corrupt_file_gives_defaults() {
        let dir = temp_dir("corrupt");
        assert_eq!(Config::load(&dir), Config::default());
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(CONFIG_FILE), "{ pas du json").unwrap();
        assert_eq!(Config::load(&dir), Config::default());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_and_missing_fields_are_tolerated() {
        let config: Config = serde_json::from_str(r#"{"volume_percent": 80, "futur": 1}"#).unwrap();
        assert_eq!(config.volume_percent, 80);
        assert!(config.auto_update);
    }

    #[test]
    fn update_check_runs_once_a_day() {
        let config = Config { last_update_check: 1_000_000, ..Config::default() };
        assert!(!config.update_check_due(1_000_000 + 3600));
        assert!(config.update_check_due(1_000_000 + 86_400));
    }
}
