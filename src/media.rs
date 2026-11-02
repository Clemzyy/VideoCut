//! Analyse des médias : un seul appel ffprobe (JSON) et calcul en flux
//! continu d'une pyramide de pics min/max pour la forme d'onde.

use crate::process::{self, KillOnDrop};
use crate::tools::Tools;
use serde::Deserialize;
use std::io::Read;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Fréquence d'échantillonnage utilisée pour la forme d'onde.
pub const ANALYSIS_RATE: u32 = 16_000;
/// Nombre maximal de cases au niveau le plus fin (environ 16 Mo de pics).
const MAX_BASE_BINS: f64 = 4_000_000.0;
/// Une case fait au minimum 1 ms à 16 kHz.
const MIN_SAMPLES_PER_BIN: u32 = 16;
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
pub const CANCELLED: &str = "Analyse annulée.";

#[derive(Debug, Clone, PartialEq)]
pub struct VideoStream {
    pub index: usize,
    pub width: u32,
    pub height: u32,
    pub fps: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MediaInfo {
    pub duration: Option<f64>,
    pub audio_index: Option<usize>,
    pub video: Option<VideoStream>,
}

impl MediaInfo {
    pub fn has_audio(&self) -> bool {
        self.audio_index.is_some()
    }

    pub fn is_video(&self) -> bool {
        self.video.is_some()
    }
}

// ---------------------------------------------------------------------------
// Sonde
// ---------------------------------------------------------------------------

pub fn probe(tools: &Tools, path: &Path) -> Result<MediaInfo, String> {
    if let Some(ffprobe) = &tools.ffprobe {
        let mut cmd = process::command(ffprobe);
        cmd.args([
            "-v",
            "error",
            "-show_entries",
            "format=duration:stream=index,codec_type,width,height,avg_frame_rate,r_frame_rate:stream_disposition=attached_pic",
            "-of",
            "json",
        ])
        .args(process::input_args(path));
        // Si ffprobe ne démarre même pas (fichier bloqué, supprimé…), on
        // se rabat sur la bannière de ffmpeg plutôt que d'échouer.
        if let Ok(output) = process::run_with_timeout(cmd, PROBE_TIMEOUT) {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if output.status.success() {
                return parse_ffprobe_json(&stdout);
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(process::summarize_error(&stderr, "ffprobe n'a pas pu lire ce fichier."));
        }
    }
    let ffmpeg = tools.ffmpeg.as_ref().ok_or("FFmpeg est introuvable.")?;
    let mut cmd = process::command(ffmpeg);
    cmd.arg("-hide_banner").args(process::input_args(path));
    let output = process::run_with_timeout(cmd, PROBE_TIMEOUT)?;
    let banner = String::from_utf8_lossy(&output.stderr);
    let info = parse_ffmpeg_banner(&banner);
    if info.audio_index.is_none() && info.video.is_none() {
        return Err(process::summarize_error(&banner, "FFmpeg n'a trouvé aucun flux audio ou vidéo."));
    }
    Ok(info)
}

#[derive(Deserialize)]
struct ProbeJson {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    format: Option<ProbeFormat>,
}

#[derive(Deserialize)]
struct ProbeStream {
    index: usize,
    codec_type: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    avg_frame_rate: Option<String>,
    r_frame_rate: Option<String>,
    disposition: Option<ProbeDisposition>,
}

#[derive(Deserialize)]
struct ProbeDisposition {
    #[serde(default)]
    attached_pic: u8,
}

#[derive(Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
}

pub fn parse_ffprobe_json(text: &str) -> Result<MediaInfo, String> {
    let json: ProbeJson = serde_json::from_str(text).map_err(|err| format!("Réponse de ffprobe illisible : {err}"))?;
    let mut info = MediaInfo {
        duration: json
            .format
            .and_then(|f| f.duration)
            .and_then(|d| d.trim().parse::<f64>().ok())
            .filter(|d| d.is_finite() && *d > 0.0),
        ..MediaInfo::default()
    };
    for stream in json.streams {
        match stream.codec_type.as_deref() {
            Some("audio") if info.audio_index.is_none() => info.audio_index = Some(stream.index),
            Some("video") if info.video.is_none() => {
                let attached = stream.disposition.is_some_and(|d| d.attached_pic != 0);
                let (Some(width), Some(height)) = (stream.width, stream.height) else { continue };
                if attached || width == 0 || height == 0 {
                    continue;
                }
                let fps = stream
                    .avg_frame_rate
                    .as_deref()
                    .and_then(parse_rate)
                    .or_else(|| stream.r_frame_rate.as_deref().and_then(parse_rate))
                    .filter(|fps| *fps <= 240.0);
                info.video = Some(VideoStream { index: stream.index, width, height, fps });
            }
            _ => {}
        }
    }
    if info.audio_index.is_none() && info.video.is_none() {
        return Err("Ce fichier ne contient ni audio ni vidéo lisible.".into());
    }
    Ok(info)
}

/// Convertit `30000/1001` ou `25` en images par seconde.
pub fn parse_rate(value: &str) -> Option<f64> {
    let value = value.trim();
    let rate = match value.split_once('/') {
        Some((num, den)) => {
            let den: f64 = den.trim().parse().ok()?;
            if den == 0.0 {
                return None;
            }
            num.trim().parse::<f64>().ok()? / den
        }
        None => value.parse().ok()?,
    };
    (rate.is_finite() && rate > 0.0).then_some(rate)
}

/// Repli quand ffprobe est absent : analyse de la bannière de `ffmpeg -i`.
pub fn parse_ffmpeg_banner(text: &str) -> MediaInfo {
    let mut info = MediaInfo::default();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Duration:") {
            let stamp = rest.split(',').next().unwrap_or("").trim();
            info.duration = parse_clock(stamp).filter(|d| *d > 0.0);
            continue;
        }
        let Some(rest) = line.strip_prefix("Stream #0:") else { continue };
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        let Ok(index) = digits.parse::<usize>() else { continue };
        if line.contains(": Audio:") && info.audio_index.is_none() {
            info.audio_index = Some(index);
        } else if line.contains(": Video:") && info.video.is_none() && !line.contains("(attached pic)") {
            let Some((width, height)) = find_resolution(line) else { continue };
            info.video = Some(VideoStream { index, width, height, fps: find_fps(line) });
        }
    }
    info
}

fn parse_clock(stamp: &str) -> Option<f64> {
    let mut parts = stamp.split(':');
    let h: f64 = parts.next()?.parse().ok()?;
    let m: f64 = parts.next()?.parse().ok()?;
    let s: f64 = parts.next()?.parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + s)
}

fn find_resolution(line: &str) -> Option<(u32, u32)> {
    line.split([' ', ',', '[', ']']).find_map(|token| {
        let (w, h) = token.split_once('x')?;
        if w.len() < 2
            || h.len() < 2
            || !w.chars().all(|c| c.is_ascii_digit())
            || !h.chars().all(|c| c.is_ascii_digit())
        {
            return None;
        }
        Some((w.parse().ok()?, h.parse().ok()?))
    })
}

fn find_fps(line: &str) -> Option<f64> {
    let tokens: Vec<&str> = line.split([' ', ',']).filter(|t| !t.is_empty()).collect();
    tokens.windows(2).find(|pair| pair[1] == "fps").and_then(|pair| parse_rate(pair[0]))
}

// ---------------------------------------------------------------------------
// Forme d'onde
// ---------------------------------------------------------------------------

/// Pyramide de pics : chaque niveau regroupe deux cases du niveau inférieur.
/// L'affichage lit le niveau adapté au zoom, donc O(largeur) par image.
#[derive(Debug, Clone)]
pub struct Peaks {
    levels: Vec<Vec<[i16; 2]>>,
    samples_per_bin: u32,
    sample_rate: u32,
    total_samples: u64,
}

impl Peaks {
    pub fn from_base(base: Vec<[i16; 2]>, samples_per_bin: u32, sample_rate: u32, total_samples: u64) -> Self {
        let mut levels = vec![if base.is_empty() { vec![[0, 0]] } else { base }];
        while levels.last().is_some_and(|level| level.len() > 1) {
            let previous = levels.last().expect("niveau présent");
            let next = previous
                .chunks(2)
                .map(|pair| pair.iter().fold([i16::MAX, i16::MIN], |acc, p| [acc[0].min(p[0]), acc[1].max(p[1])]))
                .collect();
            levels.push(next);
        }
        Self { levels, samples_per_bin, sample_rate, total_samples }
    }

    /// Durée réellement décodée.
    pub fn duration(&self) -> f64 {
        self.total_samples as f64 / f64::from(self.sample_rate)
    }

    /// Minimum et maximum normalisés (-1..1) sur l'intervalle `[t0, t1)`.
    pub fn range(&self, t0: f64, t1: f64) -> Option<(f32, f32)> {
        if t0.is_nan() || t1.is_nan() || t1 <= t0 || t1 <= 0.0 {
            return None;
        }
        let base_bin_secs = f64::from(self.samples_per_bin) / f64::from(self.sample_rate);
        let span_bins = (t1 - t0) / base_bin_secs;
        let level = if span_bins <= 4.0 { 0 } else { (span_bins / 2.0).log2().floor() as usize };
        let level = level.min(self.levels.len() - 1);
        let bins = &self.levels[level];
        let bin_secs = base_bin_secs * f64::from(1u32 << level.min(31));
        let first = (t0.max(0.0) / bin_secs).floor() as usize;
        if first >= bins.len() {
            return None;
        }
        let last = ((t1 / bin_secs).ceil() as usize).clamp(first + 1, bins.len());
        let [min, max] =
            bins[first..last].iter().fold([i16::MAX, i16::MIN], |acc, p| [acc[0].min(p[0]), acc[1].max(p[1])]);
        Some((f32::from(min) / 32768.0, f32::from(max) / 32768.0))
    }

    pub fn memory_bytes(&self) -> usize {
        self.levels.iter().map(|l| l.len() * 4).sum()
    }
}

/// Taille de case adaptée à la durée pour plafonner la mémoire.
pub fn samples_per_bin_for(duration: Option<f64>) -> u32 {
    let total = duration.unwrap_or(0.0).max(0.0) * f64::from(ANALYSIS_RATE);
    let needed = (total / MAX_BASE_BINS).ceil();
    (needed.min(f64::from(u32::MAX)) as u32).max(MIN_SAMPLES_PER_BIN)
}

/// Accumulateur en flux continu : aucun échantillon brut n'est conservé.
pub struct PeakBuilder {
    samples_per_bin: u32,
    count: u32,
    current: [i16; 2],
    base: Vec<[i16; 2]>,
    total_samples: u64,
    pending_byte: Option<u8>,
}

impl PeakBuilder {
    pub fn new(samples_per_bin: u32) -> Self {
        Self {
            samples_per_bin: samples_per_bin.max(1),
            count: 0,
            current: [i16::MAX, i16::MIN],
            base: Vec::new(),
            total_samples: 0,
            pending_byte: None,
        }
    }

    /// Ajoute des octets PCM s16le, même découpés au milieu d'un échantillon.
    pub fn push_bytes(&mut self, mut bytes: &[u8]) {
        if let Some(low) = self.pending_byte.take() {
            let Some((&high, rest)) = bytes.split_first() else {
                self.pending_byte = Some(low);
                return;
            };
            self.push_sample(i16::from_le_bytes([low, high]));
            bytes = rest;
        }
        let mut pairs = bytes.chunks_exact(2);
        for pair in &mut pairs {
            self.push_sample(i16::from_le_bytes([pair[0], pair[1]]));
        }
        if let [last] = pairs.remainder() {
            self.pending_byte = Some(*last);
        }
    }

    fn push_sample(&mut self, sample: i16) {
        self.current[0] = self.current[0].min(sample);
        self.current[1] = self.current[1].max(sample);
        self.count += 1;
        self.total_samples += 1;
        if self.count >= self.samples_per_bin {
            self.base.push(self.current);
            self.current = [i16::MAX, i16::MIN];
            self.count = 0;
        }
    }

    pub fn total_samples(&self) -> u64 {
        self.total_samples
    }

    pub fn finish(mut self, sample_rate: u32) -> Peaks {
        if self.count > 0 {
            self.base.push(self.current);
        }
        Peaks::from_base(self.base, self.samples_per_bin, sample_rate, self.total_samples)
    }
}

/// Décode la piste audio en flux et construit la pyramide de pics.
/// `progress` reçoit une valeur entre 0 et 1.
pub fn extract_peaks(
    tools: &Tools,
    path: &Path,
    audio_index: usize,
    duration: Option<f64>,
    cancel: &AtomicBool,
    mut progress: impl FnMut(f32),
) -> Result<Peaks, String> {
    let ffmpeg = tools.ffmpeg.as_ref().ok_or("FFmpeg est introuvable.")?;
    let mut cmd = process::command(ffmpeg);
    cmd.args(["-hide_banner", "-nostdin", "-loglevel", "error"])
        .args(process::input_args(path))
        .args(["-map", &format!("0:{audio_index}"), "-vn", "-sn", "-dn", "-ac", "1"])
        .args(["-ar", &ANALYSIS_RATE.to_string(), "-f", "s16le", "pipe:1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = process::spawn(&mut cmd).map_err(|err| format!("Impossible de lancer FFmpeg : {err}"))?;
    let stderr = process::drain_tail(child.stderr.take().expect("stderr capturé"));
    let mut stdout = child.stdout.take().expect("stdout capturé");
    let guard = KillOnDrop::new(child);

    let expected = duration.map(|d| d * f64::from(ANALYSIS_RATE)).filter(|e| *e > 0.0);
    let mut builder = PeakBuilder::new(samples_per_bin_for(duration));
    let mut buf = vec![0u8; 128 * 1024];
    let mut last_reported = -1.0f32;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(CANCELLED.into());
        }
        let n = match stdout.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(format!("Lecture du flux audio interrompue : {err}")),
        };
        builder.push_bytes(&buf[..n]);
        if let Some(expected) = expected {
            let ratio = (builder.total_samples() as f64 / expected).min(1.0) as f32;
            if ratio - last_reported >= 0.005 {
                last_reported = ratio;
                progress(ratio);
            }
        }
    }
    let status = guard.wait().map_err(|err| err.to_string())?;
    let stderr = stderr.join().unwrap_or_default();
    if !status.success() {
        return Err(process::summarize_error(&stderr, "FFmpeg n'a pas pu décoder l'audio."));
    }
    if builder.total_samples() == 0 {
        return Err(process::summarize_error(&stderr, "Aucun échantillon audio n'a pu être décodé."));
    }
    progress(1.0);
    Ok(builder.finish(ANALYSIS_RATE))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROBE_SAMPLE: &str = r#"{
        "streams": [
            {"index": 0, "codec_type": "video", "width": 600, "height": 600, "avg_frame_rate": "0/0", "r_frame_rate": "90000/1", "disposition": {"attached_pic": 1}},
            {"index": 1, "codec_type": "audio", "disposition": {"attached_pic": 0}},
            {"index": 2, "codec_type": "video", "width": 1920, "height": 1080, "avg_frame_rate": "30000/1001", "r_frame_rate": "30000/1001", "disposition": {"attached_pic": 0}},
            {"index": 3, "codec_type": "audio"}
        ],
        "format": {"duration": "125.250000"}
    }"#;

    #[test]
    fn ffprobe_json_skips_cover_art_and_keeps_first_streams() {
        let info = parse_ffprobe_json(PROBE_SAMPLE).unwrap();
        assert_eq!(info.duration, Some(125.25));
        assert_eq!(info.audio_index, Some(1));
        let video = info.video.unwrap();
        assert_eq!((video.index, video.width, video.height), (2, 1920, 1080));
        assert!((video.fps.unwrap() - 29.97).abs() < 0.01);
    }

    #[test]
    fn ffprobe_json_without_streams_is_an_error() {
        assert!(parse_ffprobe_json(r#"{"streams": [], "format": {}}"#).is_err());
        assert!(parse_ffprobe_json("pas du json").is_err());
    }

    #[test]
    fn unknown_duration_stays_unknown() {
        let info = parse_ffprobe_json(r#"{"streams":[{"index":0,"codec_type":"audio"}],"format":{"duration":"N/A"}}"#)
            .unwrap();
        assert_eq!(info.duration, None);
    }

    #[test]
    fn rates_are_parsed_safely() {
        assert_eq!(parse_rate("25/1"), Some(25.0));
        assert_eq!(parse_rate("24"), Some(24.0));
        assert_eq!(parse_rate("0/0"), None);
        assert_eq!(parse_rate("abc"), None);
        assert_eq!(parse_rate("-5"), None);
    }

    #[test]
    fn banner_fallback_extracts_streams() {
        let banner = "Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'file:a.mp4':\n  Duration: 00:01:30.05, start: 0.000000, bitrate: 723 kb/s\n  Stream #0:0[0x1](und): Video: h264 (High) (avc1 / 0x31637661), yuv420p(progressive), 1280x720 [SAR 1:1 DAR 16:9], 590 kb/s, 29.97 fps, 30 tbr, 12800 tbn (default)\n  Stream #0:1[0x2](und): Audio: aac (LC) (mp4a / 0x6134706D), 44100 Hz, stereo, fltp, 128 kb/s (default)\n";
        let info = parse_ffmpeg_banner(banner);
        assert!((info.duration.unwrap() - 90.05).abs() < 1e-9);
        assert_eq!(info.audio_index, Some(1));
        let video = info.video.unwrap();
        assert_eq!((video.index, video.width, video.height), (0, 1280, 720));
        assert_eq!(video.fps, Some(29.97));
    }

    #[test]
    fn banner_fallback_ignores_cover_art() {
        let banner = "  Duration: N/A, bitrate: N/A\n  Stream #0:0: Audio: mp3, 44100 Hz, stereo, fltp, 320 kb/s\n  Stream #0:1: Video: mjpeg (Baseline), yuvj420p, 600x600, 90k tbr, 90k tbn (attached pic)\n";
        let info = parse_ffmpeg_banner(banner);
        assert_eq!(info.duration, None);
        assert_eq!(info.audio_index, Some(0));
        assert!(info.video.is_none());
    }

    #[test]
    fn bin_size_caps_memory_for_long_files() {
        assert_eq!(samples_per_bin_for(None), MIN_SAMPLES_PER_BIN);
        assert_eq!(samples_per_bin_for(Some(60.0)), MIN_SAMPLES_PER_BIN);
        let three_hours = samples_per_bin_for(Some(3.0 * 3600.0));
        let bins = 3.0 * 3600.0 * f64::from(ANALYSIS_RATE) / f64::from(three_hours);
        assert!(bins <= MAX_BASE_BINS + 1.0);
    }

    fn bytes_of(samples: &[i16]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    #[test]
    fn builder_handles_samples_split_across_reads() {
        let samples: Vec<i16> = (0..100).map(|i| if i % 2 == 0 { i * 100 } else { -i * 100 }).collect();
        let bytes = bytes_of(&samples);
        let mut whole = PeakBuilder::new(10);
        whole.push_bytes(&bytes);
        let mut split = PeakBuilder::new(10);
        for chunk in bytes.chunks(3) {
            split.push_bytes(chunk);
        }
        let (a, b) = (whole.finish(1000), split.finish(1000));
        assert_eq!(a.levels, b.levels);
        assert_eq!(a.total_samples, 100);
        assert_eq!(a.levels[0].len(), 10);
        assert_eq!(a.levels[0][0], [-900, 800]);
    }

    #[test]
    fn pyramid_levels_merge_min_and_max() {
        let base = vec![[-1, 1], [-5, 2], [-3, 9], [0, 0], [-7, 3]];
        let peaks = Peaks::from_base(base, 1, 1, 5);
        assert_eq!(peaks.levels[1], vec![[-5, 2], [-3, 9], [-7, 3]]);
        assert_eq!(peaks.levels.last().unwrap(), &vec![[-7, 9]]);
    }

    #[test]
    fn range_matches_brute_force_at_any_zoom() {
        let rate = 1000;
        let samples: Vec<i16> = (0..20_000).map(|i: i32| (((i * 7919) % 6553) - 3276) as i16 * 10).collect();
        let mut builder = PeakBuilder::new(4);
        builder.push_bytes(&bytes_of(&samples));
        let peaks = builder.finish(rate);
        assert!((peaks.duration() - 20.0).abs() < 1e-9);
        for (t0, t1) in [(0.0, 20.0), (3.2, 3.25), (10.0, 10.004), (19.9, 25.0)] {
            let (min, max) = peaks.range(t0, t1).unwrap();
            let i0 = (t0 * f64::from(rate)) as usize;
            let i1 = ((t1 * f64::from(rate)) as usize).min(samples.len());
            let true_min = f32::from(*samples[i0..i1].iter().min().unwrap()) / 32768.0;
            let true_max = f32::from(*samples[i0..i1].iter().max().unwrap()) / 32768.0;
            // La pyramide peut englober une case voisine, jamais en manquer une.
            assert!(min <= true_min && max >= true_max, "{t0}-{t1}");
        }
        assert!(peaks.range(30.0, 31.0).is_none());
        assert!(peaks.range(5.0, 5.0).is_none());
    }

    #[test]
    fn empty_audio_gives_a_flat_pyramid() {
        let peaks = PeakBuilder::new(16).finish(ANALYSIS_RATE);
        assert_eq!(peaks.duration(), 0.0);
        assert_eq!(peaks.range(0.0, 0.0005), Some((0.0, 0.0)));
        assert!(peaks.memory_bytes() > 0);
    }
}
