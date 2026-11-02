//! Test réel de la mise à jour automatique (réseau, ~115 Mo).
//! Lancement explicite : `cargo test --test updater_live -- --ignored`

use std::time::{Duration, Instant};
use videocut::tools;
use videocut::updater::{self, UpdateEvent, UpdateRequest};

#[test]
#[ignore = "télécharge FFmpeg depuis GitHub"]
fn downloads_verifies_and_installs_the_latest_ffmpeg() {
    let data_dir = std::env::temp_dir().join("videocut-live-update");
    let handle = updater::spawn(
        UpdateRequest { data_dir: data_dir.clone(), installed_version: None, check_app: true },
        egui::Context::default(),
    );
    let deadline = Instant::now() + Duration::from_secs(900);
    let mut ready = None;
    let mut downloaded = false;
    loop {
        match handle.try_recv() {
            Some(UpdateEvent::Downloading { received, total, .. }) => downloaded |= received == total && total > 0,
            Some(UpdateEvent::FfmpegReady { version, installed }) => ready = Some((version, installed)),
            Some(UpdateEvent::Failed(err)) => panic!("{err}"),
            Some(UpdateEvent::Finished) => break,
            Some(_) => {}
            None => {
                assert!(Instant::now() < deadline, "mise à jour trop longue");
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    let (version, installed) = ready.expect("FFmpeg prêt");
    assert!(installed && downloaded);
    let dir = tools::managed_dir(&data_dir, &version);
    for name in ["ffmpeg", "ffprobe"] {
        assert!(dir.join(tools::exe_name(name)).is_file(), "{name} manquant");
    }
    assert!(dir.join("LICENSE.txt").is_file());
    println!("FFmpeg {version} installé dans {}", dir.display());

    // Deuxième passage : déjà à jour, rien n'est retéléchargé.
    let handle = updater::spawn(
        UpdateRequest { data_dir, installed_version: Some(version.clone()), check_app: false },
        egui::Context::default(),
    );
    loop {
        match handle.try_recv() {
            Some(UpdateEvent::Downloading { .. }) => panic!("téléchargement inutile"),
            Some(UpdateEvent::FfmpegReady { installed, .. }) => assert!(!installed),
            Some(UpdateEvent::Failed(err)) => panic!("{err}"),
            Some(UpdateEvent::Finished) => break,
            _ => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}
