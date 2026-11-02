//! Localisation de `ffmpeg` et `ffprobe`, toujours par chemin absolu.
//!
//! Ordre de recherche : exécutable choisi à la main, copie gérée et mise à
//! jour par VideoCut, dossier de l'application, puis le PATH. Le dossier
//! courant n'est jamais consulté.

use crate::config::Config;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub enum ToolOrigin {
    Custom,
    Managed(String),
    Portable,
    SystemPath,
    Missing,
}

#[derive(Debug, Clone)]
pub struct Tools {
    pub ffmpeg: Option<PathBuf>,
    pub ffprobe: Option<PathBuf>,
    pub origin: ToolOrigin,
}

impl Tools {
    pub fn describe(&self) -> String {
        match &self.origin {
            ToolOrigin::Managed(version) => format!("FFmpeg {version}"),
            ToolOrigin::Custom => "FFmpeg (choisi à la main)".into(),
            ToolOrigin::Portable => "FFmpeg (dossier de l'application)".into(),
            ToolOrigin::SystemPath => "FFmpeg (PATH système)".into(),
            ToolOrigin::Missing => "FFmpeg absent".into(),
        }
    }
}

pub fn exe_name(base: &str) -> String {
    if cfg!(windows) { format!("{base}.exe") } else { base.to_string() }
}

/// Dossier racine des versions de FFmpeg gérées par VideoCut.
pub fn managed_root(data_dir: &Path) -> PathBuf {
    data_dir.join("ffmpeg")
}

pub fn managed_dir(data_dir: &Path, version: &str) -> PathBuf {
    managed_root(data_dir).join(version)
}

pub fn locate(config: &Config, data_dir: Option<&Path>) -> Tools {
    let mut candidates: Vec<(PathBuf, ToolOrigin)> = Vec::new();
    if let Some(custom) = config.custom_ffmpeg.as_ref().and_then(|p| p.parent()) {
        candidates.push((custom.to_path_buf(), ToolOrigin::Custom));
    }
    if let (Some(data_dir), Some(version)) = (data_dir, config.ffmpeg_version.as_deref()) {
        candidates.push((managed_dir(data_dir, version), ToolOrigin::Managed(version.to_string())));
    }
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        candidates.push((exe_dir, ToolOrigin::Portable));
    }
    candidates.extend(path_dirs().into_iter().map(|dir| (dir, ToolOrigin::SystemPath)));
    locate_in(&candidates, config.custom_ffmpeg.as_deref())
}

fn locate_in(candidates: &[(PathBuf, ToolOrigin)], custom_ffmpeg: Option<&Path>) -> Tools {
    let ffmpeg_name = exe_name("ffmpeg");
    let ffprobe_name = exe_name("ffprobe");
    for (dir, origin) in candidates {
        let ffmpeg = match origin {
            ToolOrigin::Custom => custom_ffmpeg.map(Path::to_path_buf).filter(|p| p.is_file()),
            _ => existing_file(dir, &ffmpeg_name),
        };
        let Some(ffmpeg) = ffmpeg else { continue };
        let ffprobe = existing_file(dir, &ffprobe_name)
            .or_else(|| candidates.iter().find_map(|(d, _)| existing_file(d, &ffprobe_name)));
        return Tools { ffmpeg: Some(ffmpeg), ffprobe, origin: origin.clone() };
    }
    Tools { ffmpeg: None, ffprobe: None, origin: ToolOrigin::Missing }
}

fn existing_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let path = dir.join(name);
    path.is_file().then_some(path)
}

/// Entrées absolues du PATH uniquement (jamais `.` ni de chemin relatif).
fn path_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|value| std::env::split_paths(&value).filter(|p| p.is_absolute()).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fake_dir(name: &str, files: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("videocut-tools-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for file in files {
            fs::write(dir.join(exe_name(file)), b"").unwrap();
        }
        dir
    }

    #[test]
    fn first_directory_with_ffmpeg_wins() {
        let empty = fake_dir("empty", &[]);
        let managed = fake_dir("managed", &["ffmpeg", "ffprobe"]);
        let portable = fake_dir("portable", &["ffmpeg", "ffprobe"]);
        let tools = locate_in(
            &[
                (empty.clone(), ToolOrigin::Portable),
                (managed.clone(), ToolOrigin::Managed("9.0".into())),
                (portable.clone(), ToolOrigin::Portable),
            ],
            None,
        );
        assert_eq!(tools.origin, ToolOrigin::Managed("9.0".into()));
        assert_eq!(tools.ffmpeg.unwrap(), managed.join(exe_name("ffmpeg")));
        assert_eq!(tools.ffprobe.unwrap(), managed.join(exe_name("ffprobe")));
        for dir in [empty, managed, portable] {
            let _ = fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn ffprobe_can_come_from_another_directory() {
        let only_ffmpeg = fake_dir("only-ffmpeg", &["ffmpeg"]);
        let only_probe = fake_dir("only-probe", &["ffprobe"]);
        let tools = locate_in(
            &[(only_ffmpeg.clone(), ToolOrigin::Portable), (only_probe.clone(), ToolOrigin::SystemPath)],
            None,
        );
        assert_eq!(tools.origin, ToolOrigin::Portable);
        assert_eq!(tools.ffprobe.unwrap(), only_probe.join(exe_name("ffprobe")));
        let _ = fs::remove_dir_all(only_ffmpeg);
        let _ = fs::remove_dir_all(only_probe);
    }

    #[test]
    fn nothing_found_reports_missing() {
        let empty = fake_dir("nothing", &[]);
        let tools = locate_in(&[(empty.clone(), ToolOrigin::SystemPath)], None);
        assert_eq!(tools.origin, ToolOrigin::Missing);
        assert!(tools.ffmpeg.is_none());
        assert_eq!(tools.describe(), "FFmpeg absent");
        let _ = fs::remove_dir_all(empty);
    }

    #[test]
    fn custom_executable_must_exist() {
        let dir = fake_dir("custom", &["ffmpeg"]);
        let custom = dir.join(exe_name("ffmpeg"));
        let tools = locate_in(&[(dir.clone(), ToolOrigin::Custom)], Some(&custom));
        assert_eq!(tools.origin, ToolOrigin::Custom);
        let missing = dir.join("absent.exe");
        let tools = locate_in(&[(dir.clone(), ToolOrigin::Custom)], Some(&missing));
        assert_eq!(tools.origin, ToolOrigin::Missing);
        let _ = fs::remove_dir_all(dir);
    }
}
