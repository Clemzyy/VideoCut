use super::{DialogKind, VideoCutApp};
use crate::config;
use crate::process;
use crate::tools;
use crate::updater::{self, UpdateEvent, UpdateRequest};
use egui::Context;
use std::time::Duration;

impl VideoCutApp {
    pub(super) fn start_update(&mut self, ctx: &Context, check_app: bool) {
        if self.update.is_some() {
            return;
        }
        let Some(data_dir) = self.data_dir.clone() else {
            self.update_status = Some("Dossier de données introuvable : mise à jour impossible.".into());
            return;
        };
        self.config.last_update_check = config::now_unix();
        self.save_config();
        let request = UpdateRequest { data_dir, installed_version: self.config.ffmpeg_version.clone(), check_app };
        self.update = Some(updater::spawn(request, ctx.clone()));
    }

    pub(super) fn poll_update(&mut self) {
        let mut events = Vec::new();
        if let Some(handle) = &self.update {
            while let Some(event) = handle.try_recv() {
                let finished = event == UpdateEvent::Finished;
                events.push(event);
                if finished {
                    break;
                }
            }
        }
        for event in events {
            self.apply_update_event(event);
        }
    }

    /// Recherche à nouveau FFmpeg après un changement de configuration ou d'installation.
    pub(super) fn relocate_tools(&mut self) {
        self.tools = tools::locate(&self.config, self.data_dir.as_deref());
    }

    fn apply_update_event(&mut self, event: UpdateEvent) {
        match event {
            UpdateEvent::Checking => self.update_status = Some("Recherche de mises à jour…".into()),
            UpdateEvent::Downloading { version, received, total } => {
                let ratio = if total > 0 { received as f32 / total as f32 } else { 0.0 };
                self.download_progress = Some(ratio);
                self.update_status = Some(format!(
                    "Téléchargement de FFmpeg {version} : {:.0} % ({:.0} Mo)",
                    ratio * 100.0,
                    total as f64 / 1_048_576.0
                ));
            }
            UpdateEvent::Installing { version } => {
                self.download_progress = None;
                self.update_status = Some(format!("Installation de FFmpeg {version}…"));
            }
            UpdateEvent::FfmpegReady { version, installed } => {
                self.config.ffmpeg_version = Some(version.clone());
                self.save_config();
                self.relocate_tools();
                self.update_status = Some(if installed {
                    format!("FFmpeg {version} installé.")
                } else {
                    format!("FFmpeg {version} est à jour.")
                });
                if installed {
                    self.status = format!("FFmpeg {version} installé : VideoCut est prêt.");
                }
            }
            UpdateEvent::AppUpdateAvailable { version, url } => self.app_update = Some((version, url)),
            UpdateEvent::Failed(err) => {
                self.download_progress = None;
                if self.tools.ffmpeg.is_none() {
                    self.show_dialog(DialogKind::Error, "Installation de FFmpeg impossible", err.clone());
                }
                self.update_status = Some(err);
            }
            UpdateEvent::Finished => {
                self.update = None;
                self.download_progress = None;
            }
        }
    }

    pub(super) fn choose_custom_ffmpeg(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Choisir ffmpeg.exe")
            .add_filter("ffmpeg", &[if cfg!(windows) { "exe" } else { "*" }])
            .pick_file()
        else {
            return;
        };
        if !process::runs_as_ffmpeg(&path, Duration::from_secs(15)).unwrap_or(false) {
            self.show_dialog(DialogKind::Error, "Fichier refusé", "Ce fichier n'est pas un ffmpeg fonctionnel.");
            return;
        }
        self.config.custom_ffmpeg = Some(path);
        self.save_config();
        self.relocate_tools();
    }
}
