//! Lecteur intégré, sans VLC : FFmpeg décode, cpal joue le son et sert
//! d'horloge maîtresse, les images vidéo se calent sur cette horloge.
//! L'aperçu à l'arrêt passe par un extracteur d'image unique où la
//! dernière demande l'emporte toujours.

use crate::media::{MediaInfo, VideoStream};
use crate::process::{self, KillOnDrop};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use egui::ColorImage;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Taille maximale des images d'aperçu (au-delà, FFmpeg réduit l'image).
pub(crate) const PREVIEW_MAX: (u32, u32) = (1280, 720);
const MAX_PREVIEW_FPS: f64 = 60.0;
const GRAB_TIMEOUT: Duration = Duration::from_secs(20);

/// Volume partagé avec le thread audio (0.0 à 2.0), modifiable à la volée.
#[derive(Clone)]
pub struct Volume(Arc<AtomicU32>);

impl Volume {
    pub fn new(percent: u32) -> Self {
        let volume = Self(Arc::new(AtomicU32::new(0)));
        volume.set_percent(percent);
        volume
    }

    pub(crate) fn set_percent(&self, percent: u32) {
        let gain = percent.min(200) as f32 / 100.0;
        self.0.store(gain.to_bits(), Ordering::Relaxed);
    }

    fn gain(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
}

/// Dimensions paires qui tiennent dans `max` sans agrandir la source.
pub(crate) fn preview_size(source: (u32, u32), max: (u32, u32)) -> (u32, u32) {
    let (sw, sh) = (f64::from(source.0.max(2)), f64::from(source.1.max(2)));
    let scale = (f64::from(max.0) / sw).min(f64::from(max.1) / sh).min(1.0);
    let even = |value: f64| ((value * scale / 2.0).round() as u32 * 2).max(2);
    (even(sw), even(sh))
}

pub(crate) struct VideoFrame {
    pub pts: f64,
    pub image: ColorImage,
}

/// File d'images : ne rend que l'image la plus récente dont l'heure est venue.
pub(crate) struct FrameQueue {
    rx: Receiver<VideoFrame>,
    pending: Option<VideoFrame>,
    finished: bool,
}

impl FrameQueue {
    pub fn new(rx: Receiver<VideoFrame>) -> Self {
        Self { rx, pending: None, finished: false }
    }

    pub(crate) fn take_due(&mut self, position: f64) -> Option<ColorImage> {
        let mut newest = None;
        loop {
            if self.pending.is_none() {
                match self.rx.try_recv() {
                    Ok(frame) => self.pending = Some(frame),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.finished = true;
                        break;
                    }
                }
            }
            match &self.pending {
                Some(frame) if frame.pts <= position + 0.005 => {
                    newest = self.pending.take().map(|frame| frame.image);
                }
                _ => break,
            }
        }
        newest
    }

    pub(crate) fn has_frame_ready(&mut self) -> bool {
        if self.pending.is_none() {
            if let Ok(frame) = self.rx.try_recv() {
                self.pending = Some(frame);
            }
        }
        self.pending.is_some()
    }

    pub fn is_finished(&self) -> bool {
        self.finished && self.pending.is_none()
    }
}

#[derive(Default)]
struct AudioShared {
    frames_played: AtomicU64,
    finished: AtomicBool,
}

enum Clock {
    /// Horloge de la carte son : échantillons réellement envoyés.
    Audio { shared: Arc<AudioShared>, rate: f64 },
    /// Horloge murale, démarrée à la première image (fichier sans son).
    Wall { started: Option<Instant> },
}

type ErrorSlot = Arc<Mutex<Option<String>>>;

pub struct Playback {
    from: f64,
    to: f64,
    clock: Clock,
    frames: Option<FrameQueue>,
    error: ErrorSlot,
    warning: Option<String>,
    // Libérés à la destruction : le flux audio s'arrête, FFmpeg est tué.
    _stream: Option<cpal::Stream>,
    _processes: Vec<KillOnDrop>,
}

pub struct PlaybackRequest<'a> {
    pub ffmpeg: &'a Path,
    pub path: &'a Path,
    pub info: &'a MediaInfo,
    pub from: f64,
    pub to: f64,
    pub video_size: Option<(u32, u32)>,
    pub volume: Volume,
    pub ctx: egui::Context,
}

impl Playback {
    pub fn start(request: PlaybackRequest<'_>) -> Result<Self, String> {
        let from = request.from.max(0.0);
        let to = request.to.max(from + 0.02);
        let duration = to - from;
        let error: ErrorSlot = Arc::default();
        let mut processes = Vec::new();
        let mut warning = None;

        let mut frames = None;
        if let (Some(video), Some(size)) = (&request.info.video, request.video_size) {
            let (child, rx) =
                start_video(request.ffmpeg, request.path, video, from, duration, size, &request.ctx, &error)?;
            processes.push(child);
            frames = Some(FrameQueue::new(rx));
        }

        let mut stream = None;
        let mut clock = Clock::Wall { started: None };
        if let Some(audio_index) = request.info.audio_index {
            match start_audio(request.ffmpeg, request.path, audio_index, from, duration, request.volume, &error) {
                Ok(audio) => {
                    processes.push(audio.process);
                    stream = Some(audio.stream);
                    clock = Clock::Audio { shared: audio.shared, rate: audio.rate };
                }
                Err(err) if frames.is_some() => warning = Some(format!("Lecture sans le son : {err}")),
                Err(err) => return Err(err),
            }
        }
        if frames.is_none() && stream.is_none() {
            return Err("Rien à lire dans ce fichier.".into());
        }

        Ok(Self { from, to, clock, frames, error, warning, _stream: stream, _processes: processes })
    }

    /// Position actuelle dans le média, en secondes.
    pub fn position(&self) -> f64 {
        let elapsed = match &self.clock {
            Clock::Audio { shared, rate } => shared.frames_played.load(Ordering::Relaxed) as f64 / rate,
            Clock::Wall { started } => started.map_or(0.0, |s| s.elapsed().as_secs_f64()),
        };
        (self.from + elapsed).min(self.to)
    }

    pub fn is_finished(&self) -> bool {
        match &self.clock {
            Clock::Audio { shared, .. } => shared.finished.load(Ordering::Relaxed),
            Clock::Wall { .. } => {
                self.position() >= self.to - 1e-3 || self.frames.as_ref().is_some_and(FrameQueue::is_finished)
            }
        }
    }

    /// Image à afficher maintenant, s'il y en a une nouvelle.
    pub fn take_frame(&mut self) -> Option<ColorImage> {
        if let Clock::Wall { started } = &mut self.clock {
            if started.is_none() && self.frames.as_mut().is_some_and(FrameQueue::has_frame_ready) {
                *started = Some(Instant::now());
            }
        }
        let position = self.position();
        self.frames.as_mut()?.take_due(position)
    }

    pub fn take_error(&self) -> Option<String> {
        self.error.lock().ok()?.take()
    }

    pub(crate) fn take_warning(&mut self) -> Option<String> {
        self.warning.take()
    }
}

struct AudioOutput {
    stream: cpal::Stream,
    process: KillOnDrop,
    shared: Arc<AudioShared>,
    rate: f64,
}

fn start_audio(
    ffmpeg: &Path,
    path: &Path,
    audio_index: usize,
    from: f64,
    duration: f64,
    volume: Volume,
    error: &ErrorSlot,
) -> Result<AudioOutput, String> {
    let device = cpal::default_host().default_output_device().ok_or("Aucune sortie audio n'est disponible.")?;
    let supported = device.default_output_config().map_err(|err| format!("Sortie audio indisponible : {err}"))?;
    let rate = supported.sample_rate();
    let decode_channels = usize::from(supported.channels()).clamp(1, 2);

    let mut cmd = process::quiet_command(ffmpeg);
    cmd.args(["-ss", &format!("{from:.3}")])
        .args(process::input_args(path))
        .args(["-t", &format!("{duration:.3}"), "-map", &format!("0:{audio_index}"), "-vn", "-sn", "-dn"])
        .args(["-ac", &decode_channels.to_string(), "-ar", &rate.to_string(), "-f", "f32le", "pipe:1"]);
    let mut child = process::spawn_piped(&mut cmd)?;
    let stdout = child.stdout.take().expect("stdout capturé");
    let stderr = process::drain_tail(child.stderr.take().expect("stderr capturé"));
    // Protégé tout de suite : si la carte son refuse le flux, FFmpeg est tué.
    let process = KillOnDrop::new(child);

    let (tx, rx) = sync_channel::<Vec<f32>>(16);
    spawn_audio_reader(stdout, stderr, tx, decode_channels, Arc::clone(error));

    let shared = Arc::new(AudioShared::default());
    let config = supported.config();
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => build_stream::<f32>(&device, config, decode_channels, rx, &shared, volume),
        cpal::SampleFormat::I16 => build_stream::<i16>(&device, config, decode_channels, rx, &shared, volume),
        cpal::SampleFormat::U16 => build_stream::<u16>(&device, config, decode_channels, rx, &shared, volume),
        cpal::SampleFormat::I32 => build_stream::<i32>(&device, config, decode_channels, rx, &shared, volume),
        other => Err(format!("Format de la carte son non pris en charge : {other:?}")),
    }?;
    stream.play().map_err(|err| format!("Impossible de démarrer le son : {err}"))?;
    Ok(AudioOutput { stream, process, shared, rate: f64::from(rate) })
}

fn spawn_audio_reader(
    mut stdout: impl Read + Send + 'static,
    stderr: thread::JoinHandle<String>,
    tx: SyncSender<Vec<f32>>,
    channels: usize,
    error: ErrorSlot,
) {
    thread::spawn(move || {
        let mut buf = vec![0u8; 4096 * channels * 4];
        let mut carry: Vec<u8> = Vec::new();
        let mut received = 0usize;
        loop {
            let n = match stdout.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            received += n;
            carry.extend_from_slice(&buf[..n]);
            // Paquets de trames complètes : gauche/droite ne s'inversent jamais.
            let frame_bytes = 4 * channels;
            let usable = carry.len() / frame_bytes * frame_bytes;
            let samples: Vec<f32> =
                carry[..usable].chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
            carry.drain(..usable);
            if tx.send(samples).is_err() {
                return;
            }
        }
        drop(tx);
        let message = stderr.join().unwrap_or_default();
        if received == 0 {
            if let Ok(mut slot) = error.lock() {
                *slot = Some(process::summarize_error(&message, "FFmpeg n'a fourni aucun son."));
            }
        }
    });
}

fn build_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    decode_channels: usize,
    rx: Receiver<Vec<f32>>,
    shared: &Arc<AudioShared>,
    volume: Volume,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let out_channels = usize::from(config.channels).max(1);
    let callback_shared = Arc::clone(shared);
    let mut current: Vec<f32> = Vec::new();
    let mut pos = 0usize;
    let mut disconnected = false;
    let data_callback = move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
        let gain = volume.gain();
        let mut played = 0u64;
        for frame in data.chunks_mut(out_channels) {
            while pos + decode_channels > current.len() && !disconnected {
                match rx.try_recv() {
                    Ok(chunk) => {
                        current = chunk;
                        pos = 0;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => disconnected = true,
                }
            }
            if pos + decode_channels <= current.len() {
                let samples = &current[pos..pos + decode_channels];
                for (channel, out) in frame.iter_mut().enumerate() {
                    let sample = match (channel < decode_channels, decode_channels) {
                        (true, _) => samples[channel],
                        (false, 1) => samples[0],
                        (false, _) => 0.0,
                    };
                    *out = T::from_sample((sample * gain).clamp(-1.0, 1.0));
                }
                pos += decode_channels;
                played += 1;
            } else {
                frame.fill(T::EQUILIBRIUM);
                if disconnected {
                    callback_shared.finished.store(true, Ordering::Relaxed);
                }
            }
        }
        callback_shared.frames_played.fetch_add(played, Ordering::Relaxed);
    };
    let error_shared = Arc::clone(shared);
    device
        .build_output_stream::<T, _, _>(
            config,
            data_callback,
            move |_err| error_shared.finished.store(true, Ordering::Relaxed),
            None,
        )
        .map_err(|err| format!("Impossible d'ouvrir la sortie audio : {err}"))
}

#[allow(clippy::too_many_arguments)]
fn start_video(
    ffmpeg: &Path,
    path: &Path,
    video: &VideoStream,
    from: f64,
    duration: f64,
    size: (u32, u32),
    ctx: &egui::Context,
    error: &ErrorSlot,
) -> Result<(KillOnDrop, Receiver<VideoFrame>), String> {
    let fps = video.fps.unwrap_or(25.0).clamp(1.0, MAX_PREVIEW_FPS);
    let (width, height) = size;
    let mut cmd = process::quiet_command(ffmpeg);
    cmd.args(["-ss", &format!("{from:.3}")])
        .args(process::input_args(path))
        .args(["-t", &format!("{duration:.3}"), "-map", &format!("0:{}", video.index), "-an", "-sn", "-dn"])
        .args(["-vf", &format!("fps={fps:.4},scale={width}:{height}:flags=bilinear")])
        .args(["-pix_fmt", "rgb24", "-f", "rawvideo", "pipe:1"]);
    let mut child = process::spawn_piped(&mut cmd)?;
    let mut stdout = child.stdout.take().expect("stdout capturé");
    let stderr = process::drain_tail(child.stderr.take().expect("stderr capturé"));
    let (tx, rx) = sync_channel::<VideoFrame>(3);
    let ctx = ctx.clone();
    let error = Arc::clone(error);
    thread::spawn(move || {
        let frame_bytes = width as usize * height as usize * 3;
        let mut buf = vec![0u8; frame_bytes];
        let mut index = 0u64;
        while stdout.read_exact(&mut buf).is_ok() {
            let image = ColorImage::from_rgb([width as usize, height as usize], &buf);
            let pts = from + index as f64 / fps;
            if tx.send(VideoFrame { pts, image }).is_err() {
                return;
            }
            index += 1;
            ctx.request_repaint();
        }
        drop(tx);
        let message = stderr.join().unwrap_or_default();
        if index == 0 {
            if let Ok(mut slot) = error.lock() {
                slot.get_or_insert(process::summarize_error(&message, "FFmpeg n'a fourni aucune image."));
            }
        }
    });
    Ok((KillOnDrop::new(child), rx))
}

// ---------------------------------------------------------------------------
// Aperçu à l'arrêt
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct GrabRequest {
    pub generation: u64,
    pub ffmpeg: PathBuf,
    pub path: PathBuf,
    pub stream_index: usize,
    pub time: f64,
    pub size: (u32, u32),
}

pub struct GrabResult {
    pub generation: u64,
    pub image: Result<ColorImage, String>,
}

#[derive(Default)]
struct GrabSlot {
    request: Option<GrabRequest>,
    shutdown: bool,
}

/// Extracteur d'image en arrière-plan : seule la demande la plus récente est
/// traitée, si bien que glisser un marqueur ne crée jamais de file d'attente.
pub struct FrameGrabber {
    slot: Arc<(Mutex<GrabSlot>, Condvar)>,
    results: Receiver<GrabResult>,
    /// Génération (fichier) de la dernière demande : une extraction en cours
    /// pour un fichier précédent est abandonnée immédiatement.
    latest_generation: Arc<AtomicU64>,
}

impl FrameGrabber {
    pub fn new(ctx: egui::Context) -> Self {
        let slot: Arc<(Mutex<GrabSlot>, Condvar)> = Arc::default();
        let (tx, results) = sync_channel(4);
        let worker_slot = Arc::clone(&slot);
        let latest_generation = Arc::new(AtomicU64::new(0));
        let worker_generation = Arc::clone(&latest_generation);
        thread::spawn(move || {
            loop {
                let request = {
                    let (lock, cvar) = &*worker_slot;
                    let Ok(mut guard) = lock.lock() else { return };
                    loop {
                        if guard.shutdown {
                            return;
                        }
                        if let Some(request) = guard.request.take() {
                            break request;
                        }
                        let Ok(next) = cvar.wait(guard) else { return };
                        guard = next;
                    }
                };
                let stale = || worker_generation.load(Ordering::Relaxed) != request.generation;
                if stale() {
                    continue;
                }
                let image = grab_frame(&request, &stale);
                if tx.send(GrabResult { generation: request.generation, image }).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        });
        Self { slot, results, latest_generation }
    }

    pub fn request(&self, request: GrabRequest) {
        self.latest_generation.store(request.generation, Ordering::Relaxed);
        let (lock, cvar) = &*self.slot;
        if let Ok(mut guard) = lock.lock() {
            guard.request = Some(request);
            cvar.notify_one();
        }
    }

    pub fn try_recv(&self) -> Option<GrabResult> {
        self.results.try_recv().ok()
    }
}

impl Drop for FrameGrabber {
    fn drop(&mut self) {
        let (lock, cvar) = &*self.slot;
        if let Ok(mut guard) = lock.lock() {
            guard.shutdown = true;
            cvar.notify_one();
        }
    }
}

fn grab_frame(request: &GrabRequest, stale: &dyn Fn() -> bool) -> Result<ColorImage, String> {
    let (width, height) = request.size;
    let frame_bytes = width as usize * height as usize * 3;
    let run = |seek: f64, window: Option<f64>| -> Result<(Vec<u8>, String), String> {
        let mut cmd = process::quiet_command(&request.ffmpeg);
        cmd.args(["-ss", &format!("{seek:.3}")]).args(process::input_args(&request.path));
        // Fenêtre de repli : 10 images/s au plus, pour borner la mémoire.
        let filter = match window {
            Some(window) => {
                cmd.args(["-t", &format!("{window:.3}")]);
                format!("fps=10,scale={width}:{height}:flags=bilinear")
            }
            None => {
                cmd.args(["-frames:v", "1"]);
                format!("scale={width}:{height}:flags=bilinear")
            }
        };
        cmd.args(["-map", &format!("0:{}", request.stream_index), "-an", "-sn", "-dn"])
            .args(["-vf", &filter])
            .args(["-pix_fmt", "rgb24", "-f", "rawvideo", "pipe:1"]);
        let output = process::run_with_timeout_or_abort(cmd, GRAB_TIMEOUT, stale)?;
        Ok((output.stdout, String::from_utf8_lossy(&output.stderr).into_owned()))
    };
    let (data, stderr) = run(request.time.max(0.0), None)?;
    if data.len() >= frame_bytes {
        return Ok(ColorImage::from_rgb([width as usize, height as usize], &data[..frame_bytes]));
    }
    // Au-delà de la dernière image (marqueur de fin) : dernière image avant.
    if request.time > 0.0 {
        let seek = (request.time - 2.0).max(0.0);
        let (data, _) = run(seek, Some(request.time - seek + 0.05))?;
        let frames = data.len() / frame_bytes;
        if frames > 0 {
            let last = &data[(frames - 1) * frame_bytes..frames * frame_bytes];
            return Ok(ColorImage::from_rgb([width as usize, height as usize], last));
        }
    }
    Err(process::summarize_error(&stderr, "Aucune image à cette position."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(pts: f64) -> VideoFrame {
        VideoFrame { pts, image: ColorImage::filled([2, 2], egui::Color32::from_gray((pts * 10.0) as u8)) }
    }

    #[test]
    fn preview_size_fits_box_and_stays_even() {
        assert_eq!(preview_size((1920, 1080), (1280, 720)), (1280, 720));
        assert_eq!(preview_size((640, 360), (1280, 720)), (640, 360));
        assert_eq!(preview_size((1080, 1920), (1280, 720)), (406, 720));
        let (w, h) = preview_size((1001, 777), (500, 500));
        assert!(w % 2 == 0 && h % 2 == 0 && w <= 500 && h <= 500);
        assert_eq!(preview_size((0, 0), (100, 100)), (2, 2));
    }

    #[test]
    fn frame_queue_returns_newest_due_frame_only() {
        let (tx, rx) = sync_channel(10);
        for pts in [0.0, 0.04, 0.08, 0.12] {
            tx.send(frame(pts)).unwrap();
        }
        let mut queue = FrameQueue::new(rx);
        let image = queue.take_due(0.09).unwrap();
        assert_eq!(image.pixels[0], egui::Color32::from_gray(0));
        // L'image à 0,08 s est la plus récente : 0,12 s n'est pas encore due.
        assert!(queue.take_due(0.09).is_none());
        assert!(queue.take_due(0.2).is_some());
        drop(tx);
        assert!(queue.take_due(0.3).is_none());
        assert!(queue.is_finished());
    }

    #[test]
    fn frame_queue_reports_pending_frames() {
        let (tx, rx) = sync_channel(2);
        let mut queue = FrameQueue::new(rx);
        assert!(!queue.has_frame_ready());
        tx.send(frame(1.0)).unwrap();
        assert!(queue.has_frame_ready());
        assert!(queue.take_due(0.5).is_none());
        assert!(!queue.is_finished());
    }

    #[test]
    fn volume_is_clamped_to_two_hundred_percent() {
        let volume = Volume::new(150);
        assert!((volume.gain() - 1.5).abs() < f32::EPSILON);
        volume.set_percent(900);
        assert!((volume.gain() - 2.0).abs() < f32::EPSILON);
    }
}
