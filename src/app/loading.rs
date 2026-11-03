pub(super) enum LoadEvent {
    Probed(MediaInfo),
    Peaks(Result<Peaks, String>),
    Failed(String),
}

const MEDIA_EXTENSIONS: &[&str] = &[
    "aac", "aiff", "alac", "flac", "m4a", "mp3", "ogg", "opus", "wav", "wma", "avi", "flv", "m4v", "mkv", "mov", "mp4",
    "mpeg", "mpg", "mts", "m2ts", "ts", "webm", "wmv", "3gp",
];

use super::{DialogKind, LoadTask, LoadedMedia, VideoCutApp, file_name};
use crate::media::{self, MediaInfo, Peaks};
use crate::timefmt::format_time;
use crate::waveform::WaveformView;
use egui::Context;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{TryRecvError, channel};
use std::thread;

impl VideoCutApp {
    pub(super) fn choose_file(&mut self, ctx: &Context) {
        self.pause();
        let mut dialog = rfd::FileDialog::new()
            .set_title("Choisir un fichier audio ou vidéo")
            .add_filter("Audio / vidéo", MEDIA_EXTENSIONS)
            .add_filter("Tous les fichiers", &["*"]);
        if let Some(dir) = self.media.as_ref().and_then(|m| m.path.parent()) {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.pick_file() {
            self.open_file(path, ctx);
        }
    }

    pub(super) fn open_file(&mut self, path: PathBuf, ctx: &Context) {
        let path = std::path::absolute(&path).unwrap_or(path);
        if !path.is_file() {
            self.show_dialog(DialogKind::Error, "Fichier introuvable", format!("{}", path.display()));
            return;
        }
        if self.tools.ffmpeg.is_none() {
            self.show_dialog(
                DialogKind::Error,
                "FFmpeg n'est pas encore installé",
                "VideoCut a besoin de FFmpeg pour lire les fichiers. Il se télécharge automatiquement au \
                 premier lancement ; vérifie ta connexion ou choisis un ffmpeg.exe dans les paramètres (⚙).",
            );
            if self.update.is_none() {
                self.start_update(ctx, false);
            }
            return;
        }
        self.playback = None;
        self.generation += 1;
        self.media = None;
        self.video_texture = None;
        self.view = WaveformView::default();
        self.sync_time_fields();

        let (tx, rx) = channel();
        let progress = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (tools, worker_path) = (self.tools.clone(), path.clone());
        let (worker_progress, worker_cancel, worker_ctx) = (Arc::clone(&progress), Arc::clone(&cancel), ctx.clone());
        thread::spawn(move || {
            let send = |event| {
                let _ = tx.send(event);
                worker_ctx.request_repaint();
            };
            let info = match media::probe(&tools, &worker_path) {
                Ok(info) => info,
                Err(err) => return send(LoadEvent::Failed(err)),
            };
            send(LoadEvent::Probed(info.clone()));
            let Some(audio_index) = info.audio_index else {
                return send(LoadEvent::Peaks(Err("Ce fichier n'a pas de piste audio.".into())));
            };
            let result =
                media::extract_peaks(&tools, &worker_path, audio_index, info.duration, &worker_cancel, |ratio| {
                    worker_progress.store((ratio * 1000.0) as u32, Ordering::Relaxed);
                    worker_ctx.request_repaint();
                });
            send(LoadEvent::Peaks(result));
        });
        self.status = format!("Analyse de {}…", file_name(&path));
        self.loading = Some(LoadTask { path, rx, progress, cancel, info: None });
    }

    pub(super) fn poll_loading(&mut self, ctx: &Context) {
        let Some(task) = &mut self.loading else { return };
        let mut finished = None;
        loop {
            match task.rx.try_recv() {
                Ok(LoadEvent::Probed(info)) => task.info = Some(info),
                Ok(LoadEvent::Peaks(result)) => finished = Some(Ok(result)),
                Ok(LoadEvent::Failed(err)) => finished = Some(Err(err)),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    finished.get_or_insert(Err("L'analyse s'est interrompue de façon inattendue.".to_string()));
                    break;
                }
            }
        }
        let Some(finished) = finished else { return };
        let mut task = self.loading.take().expect("chargement en cours");
        let (path, info) = (std::mem::take(&mut task.path), task.info.take());
        match (finished, info) {
            (Ok(peaks), Some(info)) => self.finish_load(path, info, peaks, ctx),
            (Err(err), _) | (Ok(Err(err)), None) => self.fail_load(&path, &err),
            (Ok(Ok(_)), None) => self.fail_load(&path, "Analyse incomplète."),
        }
    }

    fn fail_load(&mut self, path: &Path, err: &str) {
        if err == media::CANCELLED {
            return;
        }
        self.status = "Erreur pendant l'analyse.".into();
        self.show_dialog(DialogKind::Error, "Lecture du fichier impossible", format!("{}\n\n{err}", file_name(path)));
    }

    fn finish_load(&mut self, path: PathBuf, info: MediaInfo, peaks: Result<Peaks, String>, ctx: &Context) {
        let (peaks, audio_problem) = match peaks {
            Ok(peaks) => (Some(peaks), None),
            Err(err) if info.is_video() => (None, info.has_audio().then_some(err)),
            Err(err) => return self.fail_load(&path, &err),
        };
        let duration = info.duration.or_else(|| peaks.as_ref().map(Peaks::duration)).filter(|d| *d > 0.0);
        let Some(duration) = duration else {
            return self.fail_load(&path, "La durée de ce fichier est introuvable.");
        };
        self.view = WaveformView::new(duration);
        let mut details = vec![format!(
            "{} chargé : {}",
            if info.is_video() { "Fichier vidéo" } else { "Fichier audio" },
            format_time(duration)
        )];
        if let Some(video) = &info.video {
            details.push(format!("{}×{}", video.width, video.height));
            details.push(video.fps.map_or("? images/s".into(), |fps| format!("{fps:.2} images/s")));
            if !info.has_audio() {
                details.push("sans piste audio".into());
            }
        }
        self.status = details.join(" | ");
        self.media = Some(LoadedMedia { path, info, peaks });
        self.sync_time_fields();
        self.request_grab(0.0);
        if let Some(err) = audio_problem {
            self.show_dialog(
                DialogKind::Error,
                "Audio illisible",
                format!("La vidéo est utilisable, mais sa piste audio n'a pas pu être analysée :\n\n{err}"),
            );
        }
        ctx.request_repaint();
    }
}
