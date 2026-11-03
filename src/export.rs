//! Export : découpe et conversion par FFmpeg, avec progression, annulation,
//! écriture atomique (fichier temporaire puis renommage) et contrôle du
//! fichier produit. Tout repli est signalé à l'utilisateur.

use crate::fsutil;
use crate::media::{self, MediaInfo};
use crate::process::{self, KillOnDrop};
use crate::timefmt::format_time;
use crate::tools::Tools;
use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError, channel};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportMode {
    /// Réencodage : coupe à l'image près.
    Precise,
    /// Copie sans perte : rapide, mais le début se cale sur une image-clé.
    Fast,
}

/// Formats proposés dans la fenêtre d'enregistrement : (libellé, extension).
pub(crate) const FORMATS: &[(&str, &str)] = &[
    ("Vidéo MP4", "mp4"),
    ("Vidéo MKV", "mkv"),
    ("Vidéo MOV", "mov"),
    ("Vidéo WebM", "webm"),
    ("Vidéo AVI", "avi"),
    ("Audio MP3", "mp3"),
    ("Audio M4A", "m4a"),
    ("Audio WAV", "wav"),
    ("Audio FLAC", "flac"),
    ("Audio OGG", "ogg"),
    ("Audio Opus", "opus"),
    ("Audio AAC", "aac"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputKind {
    Video,
    Audio,
    Other,
}

pub(crate) fn output_kind(extension: &str) -> OutputKind {
    match extension {
        "mp4" | "m4v" | "mkv" | "mov" | "webm" | "avi" => OutputKind::Video,
        "mp3" | "m4a" | "aac" | "wav" | "flac" | "ogg" | "opus" => OutputKind::Audio,
        _ => OutputKind::Other,
    }
}

pub(crate) fn extension_of(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

#[derive(Debug, Clone)]
pub struct ExportJob {
    pub ffmpeg: PathBuf,
    pub ffprobe: Option<PathBuf>,
    pub source: PathBuf,
    pub output: PathBuf,
    pub start: f64,
    pub end: f64,
    pub mode: ExportMode,
    pub info: MediaInfo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Attempt {
    Copy,
    Encode,
    EncodeDefaults,
}

/// Vérifie la demande avant de lancer quoi que ce soit.
pub fn validate(job: &ExportJob) -> Result<(), String> {
    if job.end - job.start < 0.02 {
        return Err("La fin doit être après le début.".into());
    }
    if same_file(&job.source, &job.output) || same_file(&job.source, &temp_path(&job.output)) {
        return Err(
            "Le fichier de destination est le fichier source : choisis un autre nom pour ne pas l'écraser.".into()
        );
    }
    let extension = extension_of(&job.output);
    if output_kind(&extension) == OutputKind::Audio && !job.info.has_audio() {
        return Err(format!(
            "Ce fichier n'a pas de piste audio : impossible de l'exporter en {}.",
            extension.to_uppercase()
        ));
    }
    Ok(())
}

/// Compare deux chemins comme le fait le système de fichiers.
pub(crate) fn same_file(a: &Path, b: &Path) -> bool {
    fn normalize(path: &Path) -> PathBuf {
        if let Ok(full) = path.canonicalize() {
            return full;
        }
        let parent = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        };
        match (parent.canonicalize(), path.file_name()) {
            (Ok(dir), Some(name)) => dir.join(name),
            _ => path.to_path_buf(),
        }
    }
    let (x, y) = (normalize(a), normalize(b));
    if cfg!(windows) { x.to_string_lossy().to_lowercase() == y.to_string_lossy().to_lowercase() } else { x == y }
}

/// Fichier temporaire à côté de la destination, avec la même extension
/// pour que FFmpeg choisisse le bon format.
pub fn temp_path(output: &Path) -> PathBuf {
    let stem = output.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let extension = extension_of(output);
    output.with_file_name(format!("{stem}.videocut-part.{extension}"))
}

fn codec_args(extension: &str) -> &'static [&'static str] {
    match extension {
        "mp4" | "m4v" | "mov" => &[
            "-c:v",
            "libx264",
            "-preset",
            "fast",
            "-crf",
            "18",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-b:a",
            "192k",
            "-movflags",
            "+faststart",
        ],
        "mkv" => {
            &["-c:v", "libx264", "-preset", "fast", "-crf", "18", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "192k"]
        }
        "webm" => &[
            "-c:v",
            "libvpx-vp9",
            "-crf",
            "32",
            "-b:v",
            "0",
            "-row-mt",
            "1",
            "-deadline",
            "good",
            "-cpu-used",
            "4",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "libopus",
            "-b:a",
            "160k",
        ],
        "avi" => &["-c:v", "mpeg4", "-q:v", "3", "-c:a", "libmp3lame", "-q:a", "2"],
        "mp3" => &["-c:a", "libmp3lame", "-q:a", "2"],
        "m4a" => &["-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart"],
        "aac" => &["-c:a", "aac", "-b:a", "192k"],
        "wav" => &["-c:a", "pcm_s16le"],
        "flac" => &["-c:a", "flac"],
        "ogg" => &["-c:a", "libvorbis", "-q:a", "5"],
        "opus" => &["-c:a", "libopus", "-b:a", "160k"],
        _ => &[],
    }
}

fn build_args(job: &ExportJob, temp: &Path, attempt: Attempt) -> Vec<OsString> {
    let duration = job.end - job.start;
    let extension = extension_of(&job.output);
    let kind = output_kind(&extension);
    let mut args: Vec<OsString> =
        ["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-ss"].into_iter().map(OsString::from).collect();
    args.push(format!("{:.3}", job.start).into());
    args.extend(process::input_args(&job.source));
    args.push("-t".into());
    args.push(format!("{duration:.3}").into());

    let mut push = |values: &[&str]| args.extend(values.iter().map(OsString::from));
    let audio_map = job.info.audio_index.map(|index| format!("0:{index}"));
    match (attempt, kind) {
        (Attempt::Copy, OutputKind::Audio) => {
            push(&["-map", audio_map.as_deref().unwrap_or("0:a?")]);
            // Pochette d'album conservée si la source n'a pas de vraie vidéo.
            if job.info.video.is_none() && matches!(extension.as_str(), "mp3" | "m4a" | "flac") {
                push(&["-map", "0:v?"]);
            } else {
                push(&["-vn"]);
            }
            push(&["-dn", "-c", "copy"]);
        }
        (Attempt::Copy, _) => {
            push(&[
                "-map",
                "0:v?",
                "-map",
                "0:a?",
                "-map",
                "0:s?",
                "-dn",
                "-c",
                "copy",
                "-avoid_negative_ts",
                "make_zero",
            ]);
        }
        (_, OutputKind::Audio) => {
            push(&["-map", audio_map.as_deref().unwrap_or("0:a:0"), "-vn", "-sn", "-dn"]);
        }
        (_, OutputKind::Video) => {
            if let Some(video) = &job.info.video {
                push(&["-map", &format!("0:{}", video.index)]);
            }
            push(&["-map", "0:a?", "-sn", "-dn"]);
        }
        (_, OutputKind::Other) => push(&["-sn", "-dn"]),
    }
    if attempt == Attempt::Encode {
        push(codec_args(&extension));
    }
    push(&["-progress", "pipe:1", "-nostats"]);
    args.push(process::file_arg(temp));
    args
}

/// Lit une ligne de `-progress` et renvoie le temps écoulé en secondes.
pub(crate) fn parse_progress_line(line: &str) -> Option<f64> {
    let (key, value) = line.trim().split_once('=')?;
    match key {
        "out_time_us" | "out_time_ms" => value.parse::<i64>().ok().filter(|v| *v >= 0).map(|v| v as f64 / 1e6),
        _ => None,
    }
}

pub enum ExportOutcome {
    Done { output: PathBuf, notes: Vec<String> },
    Failed(String),
    Cancelled,
}

pub struct ExportHandle {
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
    rx: Receiver<ExportOutcome>,
}

impl ExportHandle {
    /// Avancement entre 0 et 1.
    pub(crate) fn progress(&self) -> f32 {
        self.progress.load(Ordering::Relaxed) as f32 / 1000.0
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn try_outcome(&self) -> Option<ExportOutcome> {
        match self.rx.try_recv() {
            Ok(outcome) => Some(outcome),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                Some(ExportOutcome::Failed("Le découpage s'est interrompu de façon inattendue.".into()))
            }
        }
    }
}

impl Drop for ExportHandle {
    /// Fermer l'application pendant un export l'annule proprement.
    fn drop(&mut self) {
        self.cancel();
    }
}

pub fn spawn(job: ExportJob, ctx: egui::Context) -> ExportHandle {
    let progress = Arc::new(AtomicU32::new(0));
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = channel();
    let worker_progress = Arc::clone(&progress);
    let worker_cancel = Arc::clone(&cancel);
    thread::spawn(move || {
        let outcome = run_export(&job, &worker_cancel, &worker_progress, &ctx);
        let _ = tx.send(outcome);
        ctx.request_repaint();
    });
    ExportHandle { progress, cancel, rx }
}

enum AttemptError {
    Cancelled,
    Failed(String),
}

fn run_export(job: &ExportJob, cancel: &AtomicBool, progress: &AtomicU32, ctx: &egui::Context) -> ExportOutcome {
    let temp = temp_path(&job.output);
    let mut notes = Vec::new();
    let first = if job.mode == ExportMode::Fast { Attempt::Copy } else { Attempt::Encode };
    let mut result = run_attempt(job, &temp, first, cancel, progress, ctx);
    if first == Attempt::Copy {
        if let Err(AttemptError::Failed(_)) = &result {
            notes.push(
                "La copie directe a échoué avec ce format : l'extrait a été réencodé (coupe précise).".to_string(),
            );
            result = run_attempt(job, &temp, Attempt::Encode, cancel, progress, ctx);
        }
    }
    if let Err(AttemptError::Failed(message)) = &result {
        let lower = message.to_lowercase();
        if lower.contains("unknown encoder") || lower.contains("encoder not found") {
            notes.push("L'encodeur recommandé est absent de ce FFmpeg : réglages par défaut utilisés.".to_string());
            result = run_attempt(job, &temp, Attempt::EncodeDefaults, cancel, progress, ctx);
        }
    }
    match result {
        Err(AttemptError::Cancelled) => {
            let _ = fs::remove_file(&temp);
            ExportOutcome::Cancelled
        }
        Err(AttemptError::Failed(message)) => {
            let _ = fs::remove_file(&temp);
            ExportOutcome::Failed(message)
        }
        Ok(()) => {
            if let Err(message) = check_output(job, &temp, &mut notes) {
                let _ = fs::remove_file(&temp);
                return ExportOutcome::Failed(message);
            }
            match fsutil::rename_with_retry(&temp, &job.output) {
                Ok(()) => ExportOutcome::Done { output: job.output.clone(), notes },
                // Le travail n'est pas perdu : le fichier terminé est conservé.
                Err(err) => ExportOutcome::Failed(format!(
                    "L'extrait est prêt mais n'a pas pu être renommé ({err}) : un antivirus ou OneDrive le \
                     verrouille peut-être. Il est conservé ici :\n{}",
                    temp.display()
                )),
            }
        }
    }
}

fn run_attempt(
    job: &ExportJob,
    temp: &Path,
    attempt: Attempt,
    cancel: &AtomicBool,
    progress: &AtomicU32,
    ctx: &egui::Context,
) -> Result<(), AttemptError> {
    progress.store(0, Ordering::Relaxed);
    let duration = (job.end - job.start).max(0.001);
    let mut cmd = process::command(&job.ffmpeg);
    cmd.args(build_args(job, temp, attempt)).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child =
        process::spawn(&mut cmd).map_err(|err| AttemptError::Failed(format!("Impossible de lancer FFmpeg : {err}")))?;
    let stdout = child.stdout.take().expect("stdout capturé");
    let stderr = process::drain_tail(child.stderr.take().expect("stderr capturé"));
    let guard = KillOnDrop::new(child);
    // Les lignes arrivent par un canal : « Annuler » reste réactif même si
    // FFmpeg n'écrit plus rien (source lente, fichier OneDrive à télécharger).
    let (line_tx, line_rx) = channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if line_tx.send(line).is_err() {
                break;
            }
        }
    });
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(AttemptError::Cancelled);
        }
        match line_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => {
                if let Some(seconds) = parse_progress_line(&line) {
                    let ratio = (seconds / duration).clamp(0.0, 1.0);
                    progress.store((ratio * 1000.0) as u32, Ordering::Relaxed);
                    ctx.request_repaint();
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = guard.wait().map_err(|err| AttemptError::Failed(err.to_string()))?;
    let message = stderr.join().unwrap_or_default();
    if status.success() {
        progress.store(1000, Ordering::Relaxed);
        Ok(())
    } else {
        Err(AttemptError::Failed(process::summarize_error(&message, "FFmpeg a échoué sans message.")))
    }
}

/// Contrôle le fichier produit avant de le mettre à sa place définitive.
fn check_output(job: &ExportJob, temp: &Path, notes: &mut Vec<String>) -> Result<(), String> {
    let size = fs::metadata(temp).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        return Err("FFmpeg n'a produit aucune donnée : vérifie que le début se trouve avant la fin du fichier.".into());
    }
    if let Some(ffprobe) = &job.ffprobe {
        let tools = Tools {
            ffmpeg: Some(job.ffmpeg.clone()),
            ffprobe: Some(ffprobe.clone()),
            origin: crate::tools::ToolOrigin::Custom,
        };
        let expected = job.end - job.start;
        match media::probe(&tools, temp).map(|info| info.duration) {
            Ok(Some(actual)) if (actual - expected).abs() > (expected * 0.1).max(1.0) => {
                notes.push(format!("Durée obtenue : {} (attendue : {}).", format_time(actual), format_time(expected)))
            }
            Ok(_) => {}
            Err(err) => notes.push(format!("Le fichier produit n'a pas pu être relu : {err}")),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::VideoStream;

    fn job(output: &str, mode: ExportMode, with_video: bool) -> ExportJob {
        ExportJob {
            ffmpeg: PathBuf::from("ffmpeg"),
            ffprobe: None,
            source: PathBuf::from("source.mp4"),
            output: PathBuf::from(output),
            start: 1.5,
            end: 4.0,
            mode,
            info: MediaInfo {
                duration: Some(10.0),
                audio_index: Some(1),
                video: with_video.then_some(VideoStream { index: 0, width: 1280, height: 720, fps: Some(25.0) }),
            },
        }
    }

    fn joined(args: &[OsString]) -> String {
        args.iter().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn precise_video_export_reencodes_with_explicit_streams() {
        let j = job("out.mp4", ExportMode::Precise, true);
        let args = joined(&build_args(&j, Path::new("out.videocut-part.mp4"), Attempt::Encode));
        assert!(args.contains("-ss 1.500 -protocol_whitelist file,pipe -i file:source.mp4 -t 2.500"));
        assert!(args.contains("-map 0:0 -map 0:a?"));
        assert!(args.contains("-c:v libx264"));
        assert!(args.contains("-movflags +faststart"));
        assert!(args.ends_with("-progress pipe:1 -nostats file:out.videocut-part.mp4"));
    }

    #[test]
    fn audio_export_from_video_drops_the_picture() {
        let j = job("out.mp3", ExportMode::Precise, true);
        let args = joined(&build_args(&j, Path::new("t.mp3"), Attempt::Encode));
        assert!(args.contains("-map 0:1 -vn -sn -dn"));
        assert!(args.contains("-c:a libmp3lame"));
        assert!(!args.contains("libx264"));
    }

    #[test]
    fn fast_mode_copies_streams() {
        let j = job("out.mkv", ExportMode::Fast, true);
        let args = joined(&build_args(&j, Path::new("t.mkv"), Attempt::Copy));
        assert!(args.contains("-c copy -avoid_negative_ts make_zero"));
        assert!(!args.contains("libx264"));
    }

    #[test]
    fn fast_audio_copy_keeps_cover_art_only_without_real_video() {
        let audio_only = job("out.mp3", ExportMode::Fast, false);
        assert!(joined(&build_args(&audio_only, Path::new("t.mp3"), Attempt::Copy)).contains("-map 0:v?"));
        let from_video = job("out.mp3", ExportMode::Fast, true);
        let args = joined(&build_args(&from_video, Path::new("t.mp3"), Attempt::Copy));
        assert!(!args.contains("-map 0:v?") && args.contains("-vn"));
    }

    #[test]
    fn default_encoder_attempt_has_no_codec_choice() {
        let j = job("out.webm", ExportMode::Precise, true);
        let args = joined(&build_args(&j, Path::new("t.webm"), Attempt::EncodeDefaults));
        assert!(!args.contains("-c:v") && !args.contains("-c:a"));
    }

    #[test]
    fn every_listed_format_has_tuned_codecs() {
        for (_, extension) in FORMATS {
            assert!(!codec_args(extension).is_empty(), "{extension}");
            assert_ne!(output_kind(extension), OutputKind::Other, "{extension}");
        }
    }

    #[test]
    fn validation_refuses_overwriting_the_source() {
        let mut j = job("source.mp4", ExportMode::Precise, true);
        assert!(validate(&j).is_err());
        j.output = PathBuf::from("SOURCE.MP4");
        assert_eq!(validate(&j).is_err(), cfg!(windows));
        j.output = PathBuf::from("autre.mp4");
        assert!(validate(&j).is_ok());
        j.end = j.start;
        assert!(validate(&j).is_err());
    }

    #[test]
    fn validation_refuses_audio_export_without_audio() {
        let mut j = job("out.wav", ExportMode::Precise, true);
        j.info.audio_index = None;
        assert!(validate(&j).unwrap_err().contains("WAV"));
    }

    #[test]
    fn temp_file_keeps_the_extension() {
        let temp = temp_path(Path::new(r"C:\clips\mon extrait.MP4"));
        assert_eq!(temp.file_name().unwrap(), "mon extrait.videocut-part.mp4");
        assert_eq!(temp.parent(), Path::new(r"C:\clips\mon extrait.MP4").parent());
    }

    #[test]
    fn progress_lines_are_parsed() {
        assert_eq!(parse_progress_line("out_time_us=2500000"), Some(2.5));
        assert_eq!(parse_progress_line("out_time_ms=1000000"), Some(1.0));
        assert_eq!(parse_progress_line("out_time_us=N/A"), None);
        assert_eq!(parse_progress_line("out_time_us=-5"), None);
        assert_eq!(parse_progress_line("progress=end"), None);
    }
}
