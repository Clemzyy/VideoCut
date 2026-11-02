//! Fenêtre principale de VideoCut.

use crate::assets::{Icons, LINKS};
use crate::config::{self, Config};
use crate::export::{self, ExportHandle, ExportJob, ExportMode, ExportOutcome, FORMATS};
use crate::media::{self, MediaInfo, Peaks, VideoStream};
use crate::player::{self, FrameGrabber, GrabRequest, Playback, PlaybackRequest, Volume};
use crate::timefmt::{format_time, parse_time};
use crate::tools::{self, Tools};
use crate::updater::{self, UpdateEvent, UpdateHandle, UpdateRequest};
use crate::waveform::{self, Marker, WaveEvent, WaveformView};
use egui::{
    Align, Align2, Color32, ColorImage, Context, CursorIcon, FontFamily, FontId, Frame, Id, Key, Layout, Margin,
    Modifiers, RichText, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Ui, ViewportCommand, pos2, vec2,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::thread;
use std::time::Duration;

pub const APP_TITLE: &str = "VideoCut by Clemzy";
pub const REPOSITORY_URL: &str = "https://github.com/Clemzyy/VideoCut";

const BG: Color32 = Color32::from_rgb(0x15, 0x1a, 0x1f);
const PANEL: Color32 = Color32::from_rgb(0x10, 0x14, 0x18);
const BORDER: Color32 = Color32::from_rgb(0x2c, 0x35, 0x40);
const TEXT: Color32 = Color32::from_rgb(0xd8, 0xe2, 0xea);
const TEXT_BRIGHT: Color32 = Color32::from_rgb(0xea, 0xf3, 0xf8);
const TEXT_DIM: Color32 = Color32::from_rgb(0x9f, 0xb1, 0xbf);
const BUTTON_BG: Color32 = Color32::from_rgb(0x14, 0x21, 0x2a);
const BUTTON_HOVER: Color32 = Color32::from_rgb(0x1d, 0x34, 0x41);
const HANDLE_BG: Color32 = Color32::from_rgb(0x26, 0x36, 0x42);
const HANDLE_LINE: Color32 = Color32::from_rgb(0x49, 0x60, 0x6f);
const VIDEO_BG: Color32 = Color32::from_rgb(0x05, 0x07, 0x08);
const ERROR: Color32 = Color32::from_rgb(0xff, 0x7b, 0x72);

const MEDIA_EXTENSIONS: &[&str] = &[
    "aac", "aiff", "alac", "flac", "m4a", "mp3", "ogg", "opus", "wav", "wma", "avi", "flv", "m4v", "mkv", "mov", "mp4",
    "mpeg", "mpg", "mts", "m2ts", "ts", "webm", "wmv", "3gp",
];
const HANDLE_HEIGHT: f32 = 18.0;
const MIN_WAVE_HEIGHT: f32 = 150.0;
const MIN_VIDEO_HEIGHT: f32 = 120.0;

struct LoadedMedia {
    path: PathBuf,
    info: MediaInfo,
    peaks: Option<Peaks>,
}

enum LoadEvent {
    Probed(MediaInfo),
    Peaks(Result<Peaks, String>),
    Failed(String),
}

struct LoadTask {
    path: PathBuf,
    rx: Receiver<LoadEvent>,
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
    info: Option<MediaInfo>,
}

impl Drop for LoadTask {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

enum DialogKind {
    Info,
    Error,
    ConfirmQuit,
}

struct Dialog {
    title: String,
    body: String,
    kind: DialogKind,
    reveal: Option<PathBuf>,
}

pub struct VideoCutApp {
    config: Config,
    data_dir: Option<PathBuf>,
    tools: Tools,
    icons: Icons,
    media: Option<LoadedMedia>,
    loading: Option<LoadTask>,
    /// Incrémenté à chaque fichier ouvert : invalide les résultats périmés.
    generation: u64,
    view: WaveformView,
    playback: Option<Playback>,
    resume_after_drag: bool,
    volume: Volume,
    grabber: FrameGrabber,
    video_texture: Option<TextureHandle>,
    preview_box: (u32, u32),
    start_text: String,
    end_text: String,
    export: Option<ExportHandle>,
    update: Option<UpdateHandle>,
    update_status: Option<String>,
    download_progress: Option<f32>,
    app_update: Option<(String, String)>,
    status: String,
    dialog: Option<Dialog>,
    settings_open: bool,
    allow_close: bool,
}

impl VideoCutApp {
    pub fn new(cc: &eframe::CreationContext<'_>, initial_file: Option<PathBuf>) -> Self {
        let ctx = cc.egui_ctx.clone();
        setup_fonts(&ctx);
        setup_style(&ctx);
        let data_dir = config::data_dir();
        let config = data_dir.as_deref().map(Config::load).unwrap_or_default();
        let tools = tools::locate(&config, data_dir.as_deref());
        if let (Some(dir), Some(version), tools::ToolOrigin::Managed(_)) =
            (data_dir.as_deref(), config.ffmpeg_version.as_deref(), &tools.origin)
        {
            // En arrière-plan : supprimer ~200 Mo ne doit pas retarder la fenêtre.
            let (dir, version) = (dir.to_path_buf(), version.to_string());
            thread::spawn(move || updater::remove_old_versions(&dir, &version));
        }
        let mut app = Self {
            volume: Volume::new(config.volume_percent),
            grabber: FrameGrabber::new(ctx.clone()),
            icons: Icons::load(&ctx),
            config,
            data_dir,
            tools,
            media: None,
            loading: None,
            generation: 0,
            view: WaveformView::default(),
            playback: None,
            resume_after_drag: false,
            video_texture: None,
            preview_box: (960, 540),
            start_text: format_time(0.0),
            end_text: format_time(0.0),
            export: None,
            update: None,
            update_status: None,
            download_progress: None,
            app_update: None,
            status: "Prêt.".into(),
            dialog: None,
            settings_open: false,
            allow_close: false,
        };
        let ffmpeg_missing = app.tools.ffmpeg.is_none();
        if app.config.auto_update && (ffmpeg_missing || app.config.update_check_due(config::now_unix())) {
            app.start_update(&ctx, true);
        } else if ffmpeg_missing {
            app.status = "FFmpeg est introuvable : ouvre les paramètres pour l'installer.".into();
        }
        if let Some(path) = initial_file {
            app.open_file(path, &ctx);
        }
        app
    }

    fn save_config(&mut self) {
        if let Some(dir) = &self.data_dir {
            if let Err(err) = self.config.save(dir) {
                self.status = format!("Préférences non enregistrées : {err}");
            }
        }
    }

    fn show_dialog(&mut self, kind: DialogKind, title: &str, body: impl Into<String>) {
        self.dialog = Some(Dialog { title: title.into(), body: body.into(), kind, reveal: None });
    }

    // ------------------------------------------------------------------
    // Chargement
    // ------------------------------------------------------------------

    fn choose_file(&mut self, ctx: &Context) {
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

    fn open_file(&mut self, path: PathBuf, ctx: &Context) {
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

    fn poll_loading(&mut self, ctx: &Context) {
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
        let task = self.loading.take().expect("chargement en cours");
        match (finished, task.info.clone()) {
            (Ok(peaks), Some(info)) => self.finish_load(task.path.clone(), info, peaks, ctx),
            (Err(err), _) | (Ok(Err(err)), None) => self.fail_load(&task.path, &err),
            (Ok(Ok(_)), None) => self.fail_load(&task.path, "Analyse incomplète."),
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

    // ------------------------------------------------------------------
    // Lecture et aperçu
    // ------------------------------------------------------------------

    fn preview_size_for(&self, video: &VideoStream) -> (u32, u32) {
        let max = (
            self.preview_box.0.clamp(320, player::PREVIEW_MAX.0),
            self.preview_box.1.clamp(180, player::PREVIEW_MAX.1),
        );
        player::preview_size((video.width, video.height), max)
    }

    fn toggle_playback(&mut self, ctx: &Context) {
        if self.playback.is_some() {
            self.pause();
        } else {
            let (start, end, playhead) = (self.view.start, self.view.end, self.view.playhead);
            let from = if playhead < start || playhead >= end - 0.01 { start } else { playhead };
            self.play_from(from, ctx);
        }
    }

    fn play_from(&mut self, from: f64, ctx: &Context) {
        let (Some(media), Some(ffmpeg)) = (&self.media, self.tools.ffmpeg.clone()) else { return };
        self.playback = None;
        let video_size = media.info.video.as_ref().map(|video| self.preview_size_for(video));
        let request = PlaybackRequest {
            ffmpeg: &ffmpeg,
            path: &media.path,
            info: &media.info,
            from,
            to: self.view.end,
            video_size,
            volume: self.volume.clone(),
            ctx: ctx.clone(),
        };
        match Playback::start(request) {
            Ok(mut playback) => {
                if let Some(warning) = playback.take_warning() {
                    self.status = warning;
                }
                self.view.set_playhead(from);
                self.playback = Some(playback);
            }
            Err(err) => self.show_dialog(DialogKind::Error, "Lecture impossible", err),
        }
    }

    fn pause(&mut self) {
        self.playback = None;
    }

    fn stop(&mut self) {
        self.playback = None;
        self.view.set_playhead(self.view.start);
        self.request_grab(self.view.start);
    }

    fn poll_playback(&mut self, ctx: &Context) {
        let Some(playback) = &mut self.playback else { return };
        let error = playback.take_error();
        let position = playback.position();
        let frame = playback.take_frame();
        let finished = playback.is_finished() || position >= self.view.end - 1e-3;
        if let Some(image) = frame {
            self.set_video_image(ctx, image);
        }
        if let Some(err) = error {
            self.playback = None;
            self.show_dialog(DialogKind::Error, "Lecture interrompue", err);
            return;
        }
        if finished {
            self.playback = None;
            self.view.set_playhead(self.view.end);
        } else {
            self.view.follow_playhead(position);
        }
        ctx.request_repaint();
    }

    fn request_grab(&mut self, time: f64) {
        let (Some(media), Some(ffmpeg)) = (&self.media, &self.tools.ffmpeg) else { return };
        let Some(video) = &media.info.video else { return };
        self.grabber.request(GrabRequest {
            generation: self.generation,
            ffmpeg: ffmpeg.clone(),
            path: media.path.clone(),
            stream_index: video.index,
            time,
            size: self.preview_size_for(video),
        });
    }

    fn poll_grabber(&mut self, ctx: &Context) {
        while let Some(result) = self.grabber.try_recv() {
            if result.generation != self.generation || self.playback.is_some() {
                continue;
            }
            match result.image {
                Ok(image) => self.set_video_image(ctx, image),
                Err(err) => self.status = format!("Aperçu vidéo : {}", err.lines().last().unwrap_or_default()),
            }
        }
    }

    fn set_video_image(&mut self, ctx: &Context, image: ColorImage) {
        match &mut self.video_texture {
            Some(texture) => texture.set(image, TextureOptions::LINEAR),
            None => self.video_texture = Some(ctx.load_texture("video-preview", image, TextureOptions::LINEAR)),
        }
    }

    fn on_wave_event(&mut self, event: WaveEvent, ctx: &Context) {
        match event {
            WaveEvent::DragStarted(_) => {
                if self.playback.take().is_some() {
                    self.resume_after_drag = true;
                }
            }
            WaveEvent::Moved(_, value) => {
                self.sync_time_fields();
                self.request_grab(value);
            }
            WaveEvent::DragFinished(marker) => {
                if std::mem::take(&mut self.resume_after_drag) {
                    // Après un réglage de la fin, réécouter les 2 dernières secondes.
                    let from = match marker {
                        Marker::End => (self.view.end - 2.0).max(self.view.start),
                        _ => self.view.playhead,
                    };
                    self.play_from(from, ctx);
                }
            }
        }
    }

    fn set_marker(&mut self, marker: Marker, seconds: f64) {
        match marker {
            Marker::Start => {
                self.view.set_start(seconds);
                self.view.set_playhead(self.view.start);
            }
            Marker::End => {
                self.view.set_end(seconds);
                self.view.set_playhead(self.view.end);
            }
            Marker::Playhead => self.view.set_playhead(seconds),
        }
        self.sync_time_fields();
        self.request_grab(self.view.playhead);
    }

    fn sync_time_fields(&mut self) {
        self.start_text = format_time(self.view.start);
        self.end_text = format_time(self.view.end);
    }

    fn apply_time_field(&mut self, marker: Marker) {
        let text = if marker == Marker::Start { &self.start_text } else { &self.end_text };
        let parsed = parse_time(text).and_then(|value| {
            let ok = match marker {
                Marker::Start => value < self.view.end,
                _ => value > self.view.start && value <= self.view.duration() + 0.0005,
            };
            if ok { Ok(value) } else { Err("Le début doit précéder la fin, dans la durée du fichier.".into()) }
        });
        match parsed {
            Ok(value) => {
                self.playback = None;
                self.set_marker(marker, value);
            }
            Err(err) => {
                self.status = err;
                self.sync_time_fields();
            }
        }
    }

    // ------------------------------------------------------------------
    // Export
    // ------------------------------------------------------------------

    fn export_clip(&mut self, ctx: &Context) {
        if self.export.is_some() {
            return;
        }
        let (Some(media), Some(ffmpeg)) = (&self.media, self.tools.ffmpeg.clone()) else { return };
        self.playback = None;
        let source = media.path.clone();
        let extension = export::extension_of(&source);
        let stem = source.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "extrait".into());
        let mut dialog = rfd::FileDialog::new()
            .set_title("Enregistrer l'extrait")
            .set_file_name(format!("{stem}_extrait.{}", if extension.is_empty() { "mp4" } else { &extension }));
        if let Some(dir) = source.parent() {
            dialog = dialog.set_directory(dir);
        }
        if !extension.is_empty() {
            dialog = dialog.add_filter(format!("Même format (.{extension})"), &[extension.as_str()]);
        }
        for (label, ext) in FORMATS {
            dialog = dialog.add_filter(*label, &[*ext]);
        }
        let Some(mut output) = dialog.add_filter("Tous les fichiers", &["*"]).save_file() else { return };
        if output.extension().is_none() {
            output.set_extension(if extension.is_empty() { "mp4" } else { &extension });
        }
        let job = ExportJob {
            ffmpeg,
            ffprobe: self.tools.ffprobe.clone(),
            source,
            output,
            start: self.view.start,
            end: self.view.end,
            mode: if self.config.precise_export { ExportMode::Precise } else { ExportMode::Fast },
            info: media.info.clone(),
        };
        if let Err(err) = export::validate(&job) {
            self.show_dialog(DialogKind::Error, "Export impossible", err);
            return;
        }
        self.status = format!("Découpage en cours vers {}…", file_name(&job.output));
        self.export = Some(export::spawn(job, ctx.clone()));
    }

    fn poll_export(&mut self) {
        let Some(outcome) = self.export.as_ref().and_then(ExportHandle::try_outcome) else { return };
        self.export = None;
        match outcome {
            ExportOutcome::Done { output, notes } => {
                self.status = format!("Extrait créé : {}", output.display());
                let mut body = format!("Extrait créé :\n{}", output.display());
                for note in notes {
                    body.push_str("\n\n⚠ ");
                    body.push_str(&note);
                }
                self.dialog =
                    Some(Dialog { title: "Terminé".into(), body, kind: DialogKind::Info, reveal: Some(output) });
            }
            ExportOutcome::Failed(err) => {
                self.status = "Erreur pendant le découpage.".into();
                self.show_dialog(DialogKind::Error, "Erreur FFmpeg", err);
            }
            ExportOutcome::Cancelled => self.status = "Export annulé : aucun fichier n'a été laissé.".into(),
        }
    }

    // ------------------------------------------------------------------
    // Mises à jour
    // ------------------------------------------------------------------

    fn start_update(&mut self, ctx: &Context, check_app: bool) {
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

    fn poll_update(&mut self) {
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
                    self.tools = tools::locate(&self.config, self.data_dir.as_deref());
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
    }

    fn choose_custom_ffmpeg(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Choisir ffmpeg.exe")
            .add_filter("ffmpeg", &[if cfg!(windows) { "exe" } else { "*" }])
            .pick_file()
        else {
            return;
        };
        let mut cmd = crate::process::command(&path);
        cmd.arg("-version");
        let valid = crate::process::run_with_timeout(cmd, Duration::from_secs(15)).is_ok_and(|out| {
            out.status.success() && String::from_utf8_lossy(&out.stdout).starts_with("ffmpeg version")
        });
        if !valid {
            self.show_dialog(DialogKind::Error, "Fichier refusé", "Ce fichier n'est pas un ffmpeg fonctionnel.");
            return;
        }
        self.config.custom_ffmpeg = Some(path);
        self.save_config();
        self.tools = tools::locate(&self.config, self.data_dir.as_deref());
    }

    // ------------------------------------------------------------------
    // Entrées globales
    // ------------------------------------------------------------------

    fn handle_input(&mut self, ctx: &Context) {
        if let Some(path) = ctx.input(|i| i.raw.dropped_files.first().map(|file| file.path().to_path_buf())) {
            self.open_file(path, ctx);
        }
        if ctx.input(|i| i.viewport().close_requested()) && self.export.is_some() && !self.allow_close {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.show_dialog(
                DialogKind::ConfirmQuit,
                "Export en cours",
                "Un découpage est en cours. Quitter maintenant l'annulera.",
            );
        }
        if ctx.egui_wants_keyboard_input() || self.dialog.is_some() {
            return;
        }
        let pressed = |key: Key, modifiers: Modifiers| ctx.input_mut(|i| i.consume_key(modifiers, key));
        if pressed(Key::O, Modifiers::COMMAND) {
            self.choose_file(ctx);
        }
        if self.media.is_none() {
            return;
        }
        if pressed(Key::Space, Modifiers::NONE) {
            self.toggle_playback(ctx);
        }
        if pressed(Key::I, Modifiers::NONE) {
            self.set_marker(Marker::Start, self.view.playhead);
        }
        if pressed(Key::O, Modifiers::NONE) {
            self.set_marker(Marker::End, self.view.playhead);
        }
        if pressed(Key::Home, Modifiers::NONE) {
            self.stop();
        }
        if pressed(Key::S, Modifiers::COMMAND) || pressed(Key::E, Modifiers::COMMAND) {
            self.export_clip(ctx);
        }
    }

    // ------------------------------------------------------------------
    // Interface
    // ------------------------------------------------------------------

    fn header(&mut self, ui: &mut Ui, ctx: &Context) {
        Frame::NONE.fill(PANEL).stroke(Stroke::new(1.0, BORDER)).inner_margin(Margin::symmetric(14, 7)).show(
            ui,
            |ui| {
                ui.set_height(74.0);
                ui.horizontal_centered(|ui| {
                    let select = icon_button(ui, self.icons.select.as_ref(), 64.0, 72.0, "Ouvrir", BUTTON_BG)
                        .on_hover_text("Choisir un fichier audio ou vidéo (Ctrl+O), ou le glisser dans la fenêtre");
                    if select.clicked() {
                        self.choose_file(ctx);
                    }
                    ui.add_space(14.0);
                    let links_width = LINKS.len() as f32 * 44.0 + 10.0;
                    let label_width = (ui.available_width() - links_width).max(80.0);
                    ui.allocate_ui_with_layout(vec2(label_width, 72.0), Layout::top_down(Align::Min), |ui| {
                        ui.add_space(if self.media.is_some() { 14.0 } else { 24.0 });
                        let (title, subtitle) = match (&self.media, &self.loading) {
                            (Some(media), _) => (file_name(&media.path), parent_name(&media.path)),
                            (None, Some(task)) => (file_name(&task.path), Some("Analyse en cours…".into())),
                            (None, None) => ("<-- Sélectionner un fichier".to_string(), None),
                        };
                        ui.add(egui::Label::new(RichText::new(title).font(bold(18.0)).color(TEXT_BRIGHT)).truncate());
                        if let Some(subtitle) = subtitle {
                            ui.add(egui::Label::new(RichText::new(subtitle).size(12.0).color(TEXT_DIM)).truncate());
                        }
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        for (link, icon) in LINKS.iter().zip(&self.icons.links).rev() {
                            let response = icon_button(ui, icon.as_ref(), 32.0, 38.0, link.fallback, PANEL)
                                .on_hover_text(link.label);
                            if response.clicked() {
                                ctx.open_url(egui::OpenUrl::new_tab(link.url));
                            }
                        }
                    });
                });
            },
        );
    }

    fn central(&mut self, ui: &mut Ui, ctx: &Context) {
        let is_video = self.media.as_ref().is_some_and(|m| m.info.is_video());
        let available = ui.available_height();
        let video_height = if is_video {
            let max = (available - MIN_WAVE_HEIGHT - HANDLE_HEIGHT).max(MIN_VIDEO_HEIGHT);
            self.config.video_height.clamp(MIN_VIDEO_HEIGHT, max)
        } else {
            0.0
        };
        let wave_height = if is_video { available - video_height - HANDLE_HEIGHT } else { available };

        let loading_text;
        let placeholder = match (&self.media, &self.loading) {
            (Some(_), _) => None,
            (None, Some(task)) => {
                let percent = task.progress.load(Ordering::Relaxed) as f32 / 10.0;
                loading_text = format!("Analyse en cours… {percent:.0} %");
                Some(loading_text.as_str())
            }
            (None, None) => Some("Clique sur l'icône en haut à gauche ou glisse un fichier audio/vidéo ici"),
        };
        let peaks = self.media.as_ref().and_then(|m| m.peaks.as_ref());
        let view = &mut self.view;
        let events =
            ui.allocate_ui(vec2(ui.available_width(), wave_height), |ui| view.show(ui, peaks, placeholder)).inner;
        for event in events {
            self.on_wave_event(event, ctx);
        }
        if is_video {
            self.resize_handle(ui, available);
            self.video_panel(ui, video_height);
        }
    }

    fn resize_handle(&mut self, ui: &mut Ui, available: f32) {
        let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), HANDLE_HEIGHT), Sense::drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0, HANDLE_BG);
        let y = rect.center().y;
        painter.hline(rect.x_range(), y, Stroke::new(2.0, HANDLE_LINE));
        let cx = rect.center().x;
        painter.add(egui::Shape::convex_polygon(
            vec![pos2(cx - 7.0, y - 2.0), pos2(cx, y - 8.0), pos2(cx + 7.0, y - 2.0)],
            TEXT_BRIGHT,
            Stroke::NONE,
        ));
        painter.add(egui::Shape::convex_polygon(
            vec![pos2(cx - 7.0, y + 2.0), pos2(cx + 7.0, y + 2.0), pos2(cx, y + 8.0)],
            TEXT_BRIGHT,
            Stroke::NONE,
        ));
        painter.text(pos2(cx + 48.0, y), Align2::CENTER_CENTER, "glisser", bold(11.0), TEXT);
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
        }
        if response.dragged() {
            let max = (available - MIN_WAVE_HEIGHT - HANDLE_HEIGHT).max(MIN_VIDEO_HEIGHT);
            self.config.video_height =
                (self.config.video_height - response.drag_delta().y).clamp(MIN_VIDEO_HEIGHT, max);
        }
        if response.drag_stopped() {
            self.save_config();
            self.request_grab(self.view.playhead);
        }
    }

    fn video_panel(&mut self, ui: &mut Ui, height: f32) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let pixels = ui.ctx().pixels_per_point();
        self.preview_box = ((rect.width() * pixels) as u32, (rect.height() * pixels) as u32);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0, VIDEO_BG);
        match &self.video_texture {
            Some(texture) => {
                let [w, h] = texture.size();
                let target = waveform::fit_rect(rect, w as f32, h as f32);
                let uv = egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                painter.image(texture.id(), target, uv, Color32::WHITE);
            }
            None => {
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    "Aperçu vidéo",
                    FontId::proportional(15.0),
                    TEXT_DIM,
                );
            }
        }
        painter.rect_stroke(rect, 0, Stroke::new(1.0, BORDER), StrokeKind::Inside);
    }

    fn bottom(&mut self, ui: &mut Ui, ctx: &Context) {
        let ready = self.media.is_some();
        ui.add_space(6.0);
        // Progression : analyse, export ou téléchargement de FFmpeg.
        let (fraction, label) = if let Some(export) = &self.export {
            (export.progress(), format!("Découpage : {:.0} %", export.progress() * 100.0))
        } else if let Some(task) = &self.loading {
            let ratio = task.progress.load(Ordering::Relaxed) as f32 / 1000.0;
            (ratio, format!("Analyse du fichier : {:.0} %", ratio * 100.0))
        } else if let Some(ratio) = self.download_progress {
            (ratio, format!("FFmpeg : {:.0} %", ratio * 100.0))
        } else if ready {
            (1.0, "Analyse terminée".to_string())
        } else {
            (0.0, String::new())
        };
        ui.horizontal(|ui| {
            let reserved = if self.export.is_some() { 300.0 } else { 200.0 };
            ui.add(
                egui::ProgressBar::new(fraction)
                    .desired_width((ui.available_width() - reserved).max(50.0))
                    .desired_height(10.0),
            );
            ui.label(RichText::new(label).color(TEXT));
            if let Some(export) = &self.export {
                if ui.button("Annuler").clicked() {
                    export.cancel();
                }
            }
        });
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.set_min_height(60.0);
            let playing = self.playback.is_some();
            let play_label = if playing { "⏸  Pause" } else { "▶  Lecture" };
            if ui
                .add_enabled(ready, egui::Button::new(play_label).min_size(vec2(110.0, 34.0)))
                .on_hover_text("Espace")
                .clicked()
            {
                self.toggle_playback(ctx);
            }
            if ui
                .add_enabled(ready, egui::Button::new("⏹  Stop").min_size(vec2(90.0, 34.0)))
                .on_hover_text("Origine")
                .clicked()
            {
                self.stop();
            }
            ui.add_space(18.0);
            let volume_label = ui.label("Volume");
            let mut percent = self.config.volume_percent;
            let slider =
                ui.add(egui::Slider::new(&mut percent, 0..=200).show_value(false)).labelled_by(volume_label.id);
            ui.label(format!("{percent} %"));
            if slider.changed() {
                self.config.volume_percent = percent;
                self.volume.set_percent(percent);
            }
            if slider.drag_stopped() || slider.lost_focus() {
                self.save_config();
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let enabled = ready && self.export.is_none();
                let button = ui.add_enabled_ui(enabled, |ui| {
                    icon_button(ui, self.icons.export.as_ref(), 48.0, 58.0, "Exporter", BG)
                });
                if button.inner.on_hover_text("Ctrl+S").clicked() {
                    self.export_clip(ctx);
                }
                ui.label(RichText::new("Découpage et/ou conversion -->").font(bold(14.0)).color(TEXT_BRIGHT));
            });
        });
        ui.add_space(4.0);

        ui.horizontal(|ui| {
            ui.add_enabled_ui(ready, |ui| {
                let start_label = ui.label(RichText::new("Début").color(egui_start()));
                self.time_field(ui, Marker::Start, start_label.id);
                ui.add_space(10.0);
                let end_label = ui.label(RichText::new("Fin").color(egui_end()));
                self.time_field(ui, Marker::End, end_label.id);
                ui.add_space(10.0);
                ui.label("Durée extrait");
                let clip = if ready { self.view.end - self.view.start } else { 0.0 };
                ui.label(RichText::new(format_time(clip)).font(bold(14.0)));
            })
            .response
            .on_hover_text("Raccourcis : I place le début, O place la fin à la tête de lecture.");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let mut precise = self.config.precise_export;
                let combo = egui::ComboBox::from_id_salt("export-mode")
                    .width(250.0)
                    .selected_text(if precise { "Précise (réencodage)" } else { "Rapide (sans réencodage)" })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut precise, true, "Précise (réencodage)")
                            .on_hover_text("Coupe à l'image près, comme dans l'aperçu. Plus lent.");
                        ui.selectable_value(&mut precise, false, "Rapide (sans réencodage)")
                            .on_hover_text("Aucune perte de qualité, quasi instantané, mais le début d'une vidéo se cale sur l'image-clé précédente.");
                    });
                if precise != self.config.precise_export {
                    self.config.precise_export = precise;
                    self.save_config();
                }
                let mode_label = ui.label("Coupe :");
                combo.response.labelled_by(mode_label.id);
            });
        });
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            let status_color = if self.status.starts_with("Erreur") { ERROR } else { TEXT };
            let tools_label = format!("⚙ {}", self.tools.describe());
            let right_width = 260.0 + if self.app_update.is_some() { 230.0 } else { 0.0 };
            let status_size = vec2((ui.available_width() - right_width).max(80.0), 18.0);
            ui.allocate_ui_with_layout(status_size, Layout::left_to_right(Align::Center), |ui| {
                ui.add(egui::Label::new(RichText::new(&self.status).color(status_color)).truncate());
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let tools_color = if self.tools.ffmpeg.is_some() { TEXT_DIM } else { ERROR };
                let settings =
                    ui.add(egui::Button::new(RichText::new(tools_label).size(12.0).color(tools_color)).frame(false));
                if settings
                    .on_hover_text(self.update_status.clone().unwrap_or_else(|| "Paramètres et mises à jour".into()))
                    .clicked()
                {
                    self.settings_open = true;
                }
                if let Some((version, url)) = &self.app_update {
                    if ui.link(format!("Nouvelle version {version} disponible")).clicked() {
                        ctx.open_url(egui::OpenUrl::new_tab(url));
                    }
                }
            });
        });
        ui.add_space(4.0);
    }

    fn time_field(&mut self, ui: &mut Ui, marker: Marker, label: Id) {
        let text = if marker == Marker::Start { &mut self.start_text } else { &mut self.end_text };
        let response = ui
            .add(egui::TextEdit::singleline(text).desired_width(96.0).font(FontId::monospace(14.0)))
            .labelled_by(label);
        if response.lost_focus() {
            self.apply_time_field(marker);
        }
    }

    fn dialogs(&mut self, ctx: &Context) {
        let Some(dialog) = &self.dialog else { return };
        let mut close = false;
        let mut quit = false;
        let mut reveal = None;
        let response = egui::Modal::new(Id::unique("videocut-dialog")).show(ctx, |ui| {
            ui.set_max_width(520.0);
            let color = if matches!(dialog.kind, DialogKind::Error) { ERROR } else { TEXT_BRIGHT };
            ui.label(RichText::new(&dialog.title).font(bold(18.0)).color(color));
            ui.add_space(8.0);
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(&dialog.body).color(TEXT)).selectable(true));
            });
            ui.add_space(12.0);
            ui.horizontal(|ui| match dialog.kind {
                DialogKind::ConfirmQuit => {
                    if ui.button("Quitter et annuler l'export").clicked() {
                        quit = true;
                    }
                    if ui.button("Continuer l'export").clicked() {
                        close = true;
                    }
                }
                _ => {
                    if ui.button("  OK  ").clicked() {
                        close = true;
                    }
                    if let Some(path) = &dialog.reveal {
                        if ui.button("Ouvrir le dossier").clicked() {
                            reveal = Some(path.clone());
                            close = true;
                        }
                    }
                }
            });
        });
        if response.should_close() && !matches!(dialog.kind, DialogKind::ConfirmQuit) {
            close = true;
        }
        if let Some(path) = reveal {
            reveal_in_folder(&path);
        }
        if quit {
            self.allow_close = true;
            self.dialog = None;
            ctx.send_viewport_cmd(ViewportCommand::Close);
        } else if close {
            self.dialog = None;
        }
    }

    fn settings(&mut self, ctx: &Context) {
        if !self.settings_open {
            return;
        }
        let mut open = true;
        let mut check_now = false;
        let mut choose = false;
        let mut reset_custom = false;
        egui::Window::new("Paramètres")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_width(460.0);
                ui.label(RichText::new("Composants externes").font(bold(15.0)));
                ui.add_space(4.0);
                ui.label(self.tools.describe());
                if let Some(path) = &self.tools.ffmpeg {
                    ui.label(RichText::new(path.display().to_string()).size(11.0).color(TEXT_DIM));
                }
                ui.add_space(6.0);
                let mut auto = self.config.auto_update;
                if ui.checkbox(&mut auto, "Mettre FFmpeg à jour automatiquement (vérification quotidienne)").changed()
                {
                    self.config.auto_update = auto;
                    self.save_config();
                }
                ui.horizontal(|ui| {
                    if ui.add_enabled(self.update.is_none(), egui::Button::new("Vérifier maintenant")).clicked() {
                        check_now = true;
                    }
                    if ui.button("Choisir un ffmpeg.exe…").clicked() {
                        choose = true;
                    }
                    if self.config.custom_ffmpeg.is_some() && ui.button("Revenir au FFmpeg automatique").clicked() {
                        reset_custom = true;
                    }
                });
                if let Some(status) = &self.update_status {
                    ui.label(RichText::new(status).size(12.0).color(TEXT_DIM));
                }
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "FFmpeg provient des versions officielles gyan.dev (GPL) ; chaque archive est vérifiée \
                         par empreinte SHA-256 avant installation.",
                    )
                    .size(11.0)
                    .color(TEXT_DIM),
                );
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(format!("VideoCut {}", env!("CARGO_PKG_VERSION")));
                    if ui.link("Page du projet sur GitHub").clicked() {
                        ctx.open_url(egui::OpenUrl::new_tab(REPOSITORY_URL));
                    }
                });
            });
        if check_now {
            self.start_update(ctx, true);
        }
        if choose {
            self.choose_custom_ffmpeg();
        }
        if reset_custom {
            self.config.custom_ffmpeg = None;
            self.save_config();
            self.tools = tools::locate(&self.config, self.data_dir.as_deref());
        }
        self.settings_open = open;
    }

    fn drop_overlay(&self, ctx: &Context) {
        if ctx.input(|i| i.raw.hovered_files.is_empty()) {
            return;
        }
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::unique("drop-overlay")));
        let rect = ctx.content_rect();
        painter.rect_filled(rect, 0, Color32::from_black_alpha(170));
        painter.text(rect.center(), Align2::CENTER_CENTER, "Dépose le fichier pour l'ouvrir", bold(22.0), TEXT_BRIGHT);
    }
}

impl eframe::App for VideoCutApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_loading(&ctx);
        self.poll_playback(&ctx);
        self.poll_grabber(&ctx);
        self.poll_export();
        self.poll_update();
        self.handle_input(&ctx);

        egui::Panel::top("header")
            .frame(Frame::NONE.fill(BG).inner_margin(Margin { left: 14, right: 14, top: 12, bottom: 10 }))
            .show(ui, |ui| self.header(ui, &ctx));
        egui::Panel::bottom("controls")
            .frame(Frame::NONE.fill(BG).inner_margin(Margin::symmetric(14, 4)))
            .show(ui, |ui| self.bottom(ui, &ctx));
        egui::CentralPanel::no_frame()
            .frame(Frame::NONE.fill(BG).inner_margin(Margin { left: 14, right: 14, top: 0, bottom: 4 }))
            .show(ui, |ui| self.central(ui, &ctx));

        self.settings(&ctx);
        self.dialogs(&ctx);
        self.drop_overlay(&ctx);
        if self.loading.is_some() || self.export.is_some() || self.update.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

impl Drop for VideoCutApp {
    fn drop(&mut self) {
        self.playback = None;
        self.save_config();
        // Laisse au découpage annulé le temps de tuer FFmpeg et d'effacer
        // son fichier temporaire avant la fin du processus.
        if let Some(export) = self.export.take() {
            export.cancel();
            for _ in 0..60 {
                if export.try_outcome().is_some() {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

// ----------------------------------------------------------------------
// Utilitaires d'interface
// ----------------------------------------------------------------------

fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

fn egui_start() -> Color32 {
    waveform::START_COLOR
}

fn egui_end() -> Color32 {
    waveform::END_COLOR
}

/// Bouton icône ; si l'icône manque, un libellé texte le remplace.
fn icon_button(
    ui: &mut Ui,
    icon: Option<&TextureHandle>,
    icon_size: f32,
    size: f32,
    fallback: &str,
    fill: Color32,
) -> egui::Response {
    let button = match icon {
        Some(texture) => egui::Button::image(
            egui::Image::from_texture(egui::load::SizedTexture::from_handle(texture))
                .fit_to_exact_size(vec2(icon_size, icon_size)),
        ),
        None => egui::Button::new(RichText::new(fallback).color(TEXT_BRIGHT)),
    };
    ui.add(button.fill(fill).min_size(vec2(size, size))).on_hover_cursor(CursorIcon::PointingHand)
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

fn parent_name(path: &Path) -> Option<String> {
    path.parent().map(|p| p.display().to_string())
}

/// Ouvre l'Explorateur sur le fichier exporté.
fn reveal_in_folder(path: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let windir = std::env::var_os("WINDIR").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        let _ = std::process::Command::new(windir.join("explorer.exe"))
            .raw_arg(format!("/select,\"{}\"", path.display()))
            .spawn();
    }
    #[cfg(not(windows))]
    if let Some(dir) = path.parent() {
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}

fn setup_fonts(ctx: &Context) {
    let mut fonts = egui::FontDefinitions::default();
    let windir = std::env::var_os("WINDIR").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    let mut load = |name: &str, file: &str| -> bool {
        match std::fs::read(windir.join("Fonts").join(file)) {
            Ok(bytes) => {
                fonts.font_data.insert(name.to_string(), Arc::new(egui::FontData::from_owned(bytes)));
                true
            }
            Err(_) => false,
        }
    };
    let has_regular = cfg!(windows) && load("segoe-ui", "segoeui.ttf");
    let has_bold = cfg!(windows) && load("segoe-ui-bold", "segoeuib.ttf");
    // egui n'embarque plus de police de symboles : ▶ ⏸ ⏹ ⚙ ⚠ viennent de Windows.
    let symbol_fonts: Vec<String> = [("segoe-ui-symbol", "seguisym.ttf"), ("segoe-ui-emoji", "seguiemj.ttf")]
        .into_iter()
        .filter(|(name, file)| cfg!(windows) && load(name, file))
        .map(|(name, _)| name.to_string())
        .collect();
    let monospace = fonts.families.entry(FontFamily::Monospace).or_default();
    monospace.extend(symbol_fonts.iter().cloned());
    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    if has_regular {
        proportional.insert(0, "segoe-ui".to_string());
    }
    proportional.extend(symbol_fonts);
    let mut bold_family = proportional.clone();
    if has_bold {
        bold_family.insert(0, "segoe-ui-bold".to_string());
    }
    fonts.families.insert(FontFamily::Name("bold".into()), bold_family);
    ctx.set_fonts(fonts);
}

fn setup_style(ctx: &Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        let visuals = &mut style.visuals;
        visuals.panel_fill = BG;
        visuals.window_fill = PANEL;
        visuals.window_stroke = Stroke::new(1.0, BORDER);
        visuals.extreme_bg_color = PANEL;
        visuals.override_text_color = Some(TEXT);
        visuals.widgets.inactive.weak_bg_fill = BUTTON_BG;
        visuals.widgets.inactive.bg_fill = BUTTON_BG;
        visuals.widgets.hovered.weak_bg_fill = BUTTON_HOVER;
        visuals.widgets.hovered.bg_fill = BUTTON_HOVER;
        visuals.selection.bg_fill = Color32::from_rgb(0x1f, 0x6f, 0x99);
        style.spacing.button_padding = vec2(10.0, 6.0);
        style.spacing.item_spacing = vec2(8.0, 6.0);
        style.text_styles.insert(egui::TextStyle::Body, FontId::proportional(14.0));
        style.text_styles.insert(egui::TextStyle::Button, FontId::proportional(14.0));
    });
}
