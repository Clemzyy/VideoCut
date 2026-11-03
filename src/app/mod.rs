//! Fenêtre principale de VideoCut.

mod export;
mod input;
mod loading;
mod playback;
mod style;
mod ui;
mod updates;

use self::loading::LoadEvent;
use self::style::{BG, setup_fonts, setup_style};
use crate::assets::Icons;
use crate::config::{self, Config};
use crate::export::ExportHandle;
use crate::media::{MediaInfo, Peaks};
use crate::player::{FrameGrabber, Playback, Volume};
use crate::timefmt::format_time;
use crate::tools::{self, Tools};
use crate::updater::{self, UpdateHandle};
use crate::waveform::WaveformView;
use egui::{Frame, Margin, TextureHandle, Ui};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Receiver;
use std::thread;
use std::time::Duration;

/// Titre de la fenêtre, comme StreamDL : nom et version (tirée de Cargo.toml).
pub const APP_TITLE: &str = concat!("VideoCut v", env!("CARGO_PKG_VERSION"));
pub(crate) const REPOSITORY_URL: &str = "https://github.com/Clemzyy/VideoCut";

struct LoadedMedia {
    path: PathBuf,
    info: MediaInfo,
    peaks: Option<Peaks>,
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

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}
