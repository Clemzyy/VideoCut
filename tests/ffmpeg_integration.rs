//! Tests de bout en bout avec un vrai FFmpeg.
//!
//! FFmpeg est cherché dans `VIDEOCUT_FFMPEG_DIR`, puis comme le ferait
//! l'application. S'il est absent, les tests s'arrêtent sans échouer.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};
use videocut::config::Config;
use videocut::export::{self, ExportJob, ExportMode, ExportOutcome};
use videocut::media::{self, MediaInfo};
use videocut::player::{FrameGrabber, GrabRequest, Playback, PlaybackRequest, Volume};
use videocut::process;
use videocut::tools::{self, ToolOrigin, Tools};

fn find_tools() -> Option<Tools> {
    if let Some(dir) = std::env::var_os("VIDEOCUT_FFMPEG_DIR").map(PathBuf::from) {
        let ffmpeg = dir.join(tools::exe_name("ffmpeg"));
        let ffprobe = dir.join(tools::exe_name("ffprobe"));
        if ffmpeg.is_file() {
            return Some(Tools {
                ffmpeg: Some(ffmpeg),
                ffprobe: ffprobe.is_file().then_some(ffprobe),
                origin: ToolOrigin::Custom,
            });
        }
    }
    let found = tools::locate(&Config::default(), None);
    found.ffmpeg.is_some().then_some(found)
}

fn work_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("videocut-it-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Fabrique un média de test (mire + bip) avec les générateurs de FFmpeg.
fn generate(tools: &Tools, output: &Path, video: bool, seconds: u32) {
    let mut cmd = process::command(tools.ffmpeg.as_ref().unwrap());
    cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i"])
        .arg(format!("sine=frequency=440:sample_rate=44100:duration={seconds}"));
    if video {
        cmd.args(["-f", "lavfi", "-i"]).arg(format!("testsrc=size=320x240:rate=25:duration={seconds}")).args([
            "-c:v",
            "libx264",
            "-g",
            "50",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
        ]);
    }
    cmd.arg(output);
    let result = process::run_with_timeout(cmd, Duration::from_secs(60)).unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
}

fn wait_export(job: ExportJob) -> ExportOutcome {
    let handle = export::spawn(job, egui::Context::default());
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if let Some(outcome) = handle.try_outcome() {
            return outcome;
        }
        assert!(Instant::now() < deadline, "export trop long");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn job(tools: &Tools, source: &Path, output: PathBuf, info: &MediaInfo, mode: ExportMode) -> ExportJob {
    ExportJob {
        ffmpeg: tools.ffmpeg.clone().unwrap(),
        ffprobe: tools.ffprobe.clone(),
        source: source.to_path_buf(),
        output,
        start: 1.0,
        end: 3.0,
        mode,
        info: info.clone(),
    }
}

#[test]
fn analyse_export_and_preview_a_real_video() {
    let Some(tools) = find_tools() else {
        eprintln!("FFmpeg introuvable : test ignoré (définir VIDEOCUT_FFMPEG_DIR).");
        return;
    };
    let dir = work_dir("video");
    // Espaces et accents : les chemins réels des utilisateurs.
    let source = dir.join("ma vidéo d'essai.mp4");
    generate(&tools, &source, true, 5);

    let info = media::probe(&tools, &source).unwrap();
    assert!((info.duration.unwrap() - 5.0).abs() < 0.2);
    let video = info.video.clone().expect("flux vidéo");
    assert_eq!((video.width, video.height), (320, 240));
    assert!((video.fps.unwrap() - 25.0).abs() < 0.01);

    let peaks = media::extract_peaks(
        &tools,
        &source,
        info.audio_index.unwrap(),
        info.duration,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    assert!((peaks.duration() - 5.0).abs() < 0.2);
    let (min, max) = peaks.range(1.0, 2.0).unwrap();
    // Le générateur « sine » de FFmpeg produit une amplitude de 1/8.
    assert!((max - 0.125).abs() < 0.02 && (min + 0.125).abs() < 0.02, "amplitude du bip : {min} {max}");

    let precise = dir.join("extrait précis.mp4");
    match wait_export(job(&tools, &source, precise.clone(), &info, ExportMode::Precise)) {
        ExportOutcome::Done { notes, .. } => assert!(notes.is_empty(), "{notes:?}"),
        ExportOutcome::Failed(err) => panic!("{err}"),
        ExportOutcome::Cancelled => panic!("annulé"),
    }
    let out = media::probe(&tools, &precise).unwrap();
    assert!((out.duration.unwrap() - 2.0).abs() < 0.15, "{:?}", out.duration);
    assert!(out.video.is_some() && out.has_audio());
    assert!(!export::temp_path(&precise).exists(), "le fichier temporaire doit disparaître");

    let fast = dir.join("extrait rapide.mkv");
    assert!(matches!(
        wait_export(job(&tools, &source, fast.clone(), &info, ExportMode::Fast)),
        ExportOutcome::Done { .. }
    ));
    assert!(media::probe(&tools, &fast).unwrap().video.is_some());

    let mp3 = dir.join("son.mp3");
    assert!(matches!(
        wait_export(job(&tools, &source, mp3.clone(), &info, ExportMode::Precise)),
        ExportOutcome::Done { .. }
    ));
    let audio = media::probe(&tools, &mp3).unwrap();
    assert!(audio.video.is_none() && audio.has_audio());

    let grabber = FrameGrabber::new(egui::Context::default());
    for time in [2.5, 5.0] {
        grabber.request(GrabRequest {
            generation: 1,
            ffmpeg: tools.ffmpeg.clone().unwrap(),
            path: source.clone(),
            stream_index: video.index,
            time,
            size: (160, 120),
        });
        let deadline = Instant::now() + Duration::from_secs(30);
        let result = loop {
            if let Some(result) = grabber.try_recv() {
                break result;
            }
            assert!(Instant::now() < deadline, "aucune image reçue");
            std::thread::sleep(Duration::from_millis(20));
        };
        // 5,0 s est la toute fin : l'extracteur doit rendre la dernière image.
        let image = result.image.unwrap_or_else(|err| panic!("image à {time} s : {err}"));
        assert_eq!(image.size, [160, 120]);
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn audio_only_file_and_error_reporting() {
    let Some(tools) = find_tools() else { return };
    let dir = work_dir("audio");
    let source = dir.join("bip.wav");
    generate(&tools, &source, false, 4);
    let info = media::probe(&tools, &source).unwrap();
    assert!(info.video.is_none() && info.has_audio());

    let flac = dir.join("bip.flac");
    let mut j = job(&tools, &source, flac.clone(), &info, ExportMode::Fast);
    j.end = 2.0;
    match wait_export(j) {
        // WAV vers FLAC : la copie échoue, le réencodage prend le relais et le dit.
        ExportOutcome::Done { notes, .. } => assert!(notes.iter().any(|n| n.contains("réencodé")), "{notes:?}"),
        ExportOutcome::Failed(err) => panic!("{err}"),
        ExportOutcome::Cancelled => panic!("annulé"),
    }
    assert!((media::probe(&tools, &flac).unwrap().duration.unwrap() - 1.0).abs() < 0.1);

    let bogus = dir.join("pas un média.mp4");
    std::fs::write(&bogus, b"ceci n'est pas une video").unwrap();
    let err = media::probe(&tools, &bogus).unwrap_err();
    assert!(!err.is_empty());

    let video_only = ExportJob {
        info: MediaInfo { audio_index: None, ..info },
        ..job(&tools, &source, dir.join("x.mp3"), &MediaInfo::default(), ExportMode::Precise)
    };
    assert!(export::validate(&video_only).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn cancelling_an_export_leaves_nothing_behind() {
    let Some(tools) = find_tools() else { return };
    let dir = work_dir("cancel");
    let source = dir.join("long.mp4");
    generate(&tools, &source, true, 60);
    let info = media::probe(&tools, &source).unwrap();
    let output = dir.join("annulé.webm");
    let mut j = job(&tools, &source, output.clone(), &info, ExportMode::Precise);
    j.end = 59.0;
    let handle = export::spawn(j, egui::Context::default());
    std::thread::sleep(Duration::from_millis(1500));
    handle.cancel();
    let deadline = Instant::now() + Duration::from_secs(20);
    let outcome = loop {
        if let Some(outcome) = handle.try_outcome() {
            break outcome;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(matches!(outcome, ExportOutcome::Cancelled));
    assert!(!output.exists());
    assert!(!export::temp_path(&output).exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn playback_follows_the_audio_clock_and_stops_at_the_end() {
    let Some(tools) = find_tools() else { return };
    let dir = work_dir("playback");
    let source = dir.join("lecture.mp4");
    generate(&tools, &source, true, 4);
    let info = media::probe(&tools, &source).unwrap();
    let ffmpeg = tools.ffmpeg.clone().unwrap();
    let request = PlaybackRequest {
        ffmpeg: &ffmpeg,
        path: &source,
        info: &info,
        from: 1.0,
        to: 2.5,
        video_size: Some((160, 120)),
        // Volume nul : le test ne fait aucun bruit.
        volume: Volume::new(0),
        ctx: egui::Context::default(),
    };
    let mut playback = match Playback::start(request) {
        Ok(playback) => playback,
        Err(err) => {
            eprintln!("Pas de sortie audio ici : test ignoré ({err}).");
            return;
        }
    };
    let started = Instant::now();
    let mut frames = 0;
    while !playback.is_finished() {
        assert!(started.elapsed() < Duration::from_secs(10), "la lecture ne s'arrête pas");
        if playback.take_frame().is_some() {
            frames += 1;
        }
        assert!(playback.take_error().is_none());
        std::thread::sleep(Duration::from_millis(10));
    }
    let elapsed = started.elapsed().as_secs_f64();
    // 1,5 s d'extrait : l'horloge audio ne doit ni courir ni traîner.
    assert!((1.3..4.0).contains(&elapsed), "durée de lecture {elapsed}");
    assert!((playback.position() - 2.5).abs() < 0.1, "position finale {}", playback.position());
    assert!(frames >= 20, "seulement {frames} images affichées");
    let _ = std::fs::remove_dir_all(dir);
}
