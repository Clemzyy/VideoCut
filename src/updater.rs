//! Mise à jour automatique des composants externes.
//!
//! FFmpeg est récupéré depuis les versions officielles de gyan.dev publiées
//! sur GitHub (`GyanD/codexffmpeg`). L'archive est vérifiée (taille et
//! empreinte SHA-256 annoncées par GitHub), extraite dans un dossier
//! temporaire, testée, puis activée par renommage. L'ancienne version reste
//! utilisable jusqu'au bout et n'est supprimée qu'au lancement suivant.
//!
//! VideoCut lui-même vérifie s'il existe une version plus récente sur son
//! dépôt GitHub et le signale (sans rien remplacer automatiquement).

use crate::fsutil;
use crate::process;
use crate::tools::{exe_name, managed_dir, managed_root};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::thread;
use std::time::Duration;

pub(crate) const FFMPEG_RELEASE_API: &str = "https://api.github.com/repos/GyanD/codexffmpeg/releases/latest";
pub(crate) const APP_RELEASE_API: &str = "https://api.github.com/repos/Clemzyy/VideoCut/releases/latest";
pub(crate) const APP_RELEASES_PAGE: &str = "https://github.com/Clemzyy/VideoCut/releases/latest";
const ASSET_SUFFIX: &str = "-essentials_build.zip";
/// Taille maximale d'un fichier extrait (ffmpeg.exe pèse ~100 Mo).
const MAX_EXTRACTED_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Asset {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    pub size: u64,
    pub digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UpdateEvent {
    Checking,
    Downloading {
        version: String,
        received: u64,
        total: u64,
    },
    Installing {
        version: String,
    },
    /// `installed` : une nouvelle version vient d'être mise en place.
    FfmpegReady {
        version: String,
        installed: bool,
    },
    AppUpdateAvailable {
        version: String,
        url: String,
    },
    Failed(String),
    Finished,
}

pub struct UpdateRequest {
    pub data_dir: PathBuf,
    pub installed_version: Option<String>,
    pub check_app: bool,
}

pub struct UpdateHandle {
    rx: Receiver<UpdateEvent>,
}

impl UpdateHandle {
    /// Un thread interrompu sans prévenir est traité comme terminé.
    pub fn try_recv(&self) -> Option<UpdateEvent> {
        match self.rx.try_recv() {
            Ok(event) => Some(event),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(UpdateEvent::Finished),
        }
    }
}

pub fn spawn(request: UpdateRequest, ctx: egui::Context) -> UpdateHandle {
    let (tx, rx) = channel();
    thread::spawn(move || {
        let send = |event: UpdateEvent| {
            let _ = tx.send(event);
            ctx.request_repaint();
        };
        send(UpdateEvent::Checking);
        if cfg!(windows) {
            match update_ffmpeg(&request, &send) {
                Ok((version, installed)) => send(UpdateEvent::FfmpegReady { version, installed }),
                Err(err) => send(UpdateEvent::Failed(err)),
            }
        }
        if request.check_app {
            if let Ok(Some((version, url))) = check_app_update() {
                send(UpdateEvent::AppUpdateAvailable { version, url });
            }
        }
        send(UpdateEvent::Finished);
    });
    UpdateHandle { rx }
}

fn agent(timeout: Option<Duration>) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(timeout)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_per_read(Some(Duration::from_secs(30)))
        .https_only(true)
        .user_agent(format!("VideoCut/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

fn fetch_release(url: &str) -> Result<Release, ureq::Error> {
    agent(Some(Duration::from_secs(30)))
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .call()?
        .body_mut()
        .read_json::<Release>()
}

fn update_ffmpeg(request: &UpdateRequest, send: &dyn Fn(UpdateEvent)) -> Result<(String, bool), String> {
    let release = fetch_release(FFMPEG_RELEASE_API).map_err(|err| network_error("FFmpeg", &err))?;
    let version = sanitize_tag(&release.tag_name)
        .ok_or_else(|| format!("Numéro de version FFmpeg inattendu : {}", release.tag_name))?;
    let target = managed_dir(&request.data_dir, &version);
    if request.installed_version.as_deref() == Some(version.as_str()) && is_complete(&target) {
        return Ok((version, false));
    }
    let asset = pick_asset(&release).ok_or("Aucune archive « essentials » dans la dernière version de FFmpeg.")?;
    let root = managed_root(&request.data_dir);
    fs::create_dir_all(&root).map_err(|err| format!("Dossier {} : {err}", root.display()))?;

    let archive = root.join(format!("{version}.download.zip"));
    download(asset, &archive, &|received, total| {
        send(UpdateEvent::Downloading { version: version.clone(), received, total });
    })?;
    send(UpdateEvent::Installing { version: version.clone() });

    let staging = root.join(format!("{version}.staging"));
    let result = install_from_archive(&archive, &staging, &target);
    let _ = fs::remove_file(&archive);
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result.map(|()| (version, true))
}

fn install_from_archive(archive: &Path, staging: &Path, target: &Path) -> Result<(), String> {
    let _ = fs::remove_dir_all(staging);
    fs::create_dir_all(staging).map_err(|err| err.to_string())?;
    extract_tools(archive, staging)?;
    verify_ffmpeg(&staging.join(exe_name("ffmpeg")))?;
    if target.exists() {
        fs::remove_dir_all(target).map_err(|err| format!("Ancienne copie de FFmpeg verrouillée : {err}"))?;
    }
    fsutil::rename_with_retry(staging, target)
        .map_err(|err| format!("Activation de la nouvelle version impossible : {err}"))
}

fn is_complete(dir: &Path) -> bool {
    dir.join(exe_name("ffmpeg")).is_file() && dir.join(exe_name("ffprobe")).is_file()
}

fn network_error(what: &str, err: &ureq::Error) -> String {
    match err {
        ureq::Error::StatusCode(403 | 429) => {
            format!("Vérification de {what} reportée : limite de requêtes GitHub atteinte.")
        }
        ureq::Error::StatusCode(code) => format!("Vérification de {what} impossible (HTTP {code})."),
        other => format!("Vérification de {what} impossible (connexion internet ?) : {other}"),
    }
}

/// N'accepte que des numéros de version sûrs pour un nom de dossier.
pub(crate) fn sanitize_tag(tag: &str) -> Option<String> {
    let tag = tag.trim().trim_start_matches(['v', 'V']);
    let valid = !tag.is_empty()
        && tag.len() <= 40
        && !tag.starts_with('.')
        && !tag.contains("..")
        && tag.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    valid.then(|| tag.to_string())
}

pub(crate) fn pick_asset(release: &Release) -> Option<&Asset> {
    release.assets.iter().find(|asset| asset.name.ends_with(ASSET_SUFFIX))
}

/// Télécharge en calculant l'empreinte au fil de l'eau, puis vérifie
/// la taille et le SHA-256 annoncés par GitHub.
fn download(asset: &Asset, destination: &Path, progress: &dyn Fn(u64, u64)) -> Result<(), String> {
    let partial = destination.with_extension("part");
    let mut response =
        agent(None).get(&asset.browser_download_url).call().map_err(|err| network_error("FFmpeg", &err))?;
    let total = response.body().content_length().unwrap_or(asset.size);
    let mut reader = response.body_mut().as_reader();
    let mut file = File::create(&partial).map_err(|err| format!("{} : {err}", partial.display()))?;
    let mut hasher = Sha256::new();
    let mut received = 0u64;
    let mut last_report = 0u64;
    let mut buf = vec![0u8; 256 * 1024];
    let copy_result = (|| -> io::Result<()> {
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                return Ok(());
            }
            received += n as u64;
            if asset.size > 0 && received > asset.size {
                return Err(io::Error::other("le serveur envoie plus de données qu'annoncé"));
            }
            file.write_all(&buf[..n])?;
            hasher.update(&buf[..n]);
            if received - last_report >= 1024 * 1024 || received == total {
                last_report = received;
                progress(received, total);
            }
        }
    })();
    drop(file);
    let verified = copy_result
        .map_err(|err| format!("Téléchargement interrompu : {err}"))
        .and_then(|()| verify_download(asset, received, &hasher.finalize()));
    match verified {
        Ok(()) => fs::rename(&partial, destination).map_err(|err| err.to_string()),
        Err(err) => {
            let _ = fs::remove_file(&partial);
            Err(err)
        }
    }
}

fn verify_download(asset: &Asset, received: u64, sha256: &[u8]) -> Result<(), String> {
    if asset.size > 0 && received != asset.size {
        return Err(format!("Téléchargement incomplet : {received} octets reçus sur {}.", asset.size));
    }
    // Sans empreinte SHA-256 publiée, l'archive est refusée.
    let Some(expected) = asset.digest.as_deref().and_then(|d| d.strip_prefix("sha256:")) else {
        return Err("GitHub ne fournit pas d'empreinte SHA-256 pour cette archive : installation refusée.".into());
    };
    if !to_hex(sha256).eq_ignore_ascii_case(expected.trim()) {
        return Err("L'empreinte SHA-256 de FFmpeg ne correspond pas : archive rejetée.".into());
    }
    Ok(())
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Extrait uniquement `bin/ffmpeg.exe`, `bin/ffprobe.exe` et la licence,
/// sous des noms fixés par VideoCut (aucun chemin de l'archive n'est utilisé).
fn extract_tools(archive: &Path, destination: &Path) -> Result<(), String> {
    let file = File::open(archive).map_err(|err| err.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|err| format!("Archive FFmpeg illisible : {err}"))?;
    let wanted = [exe_name("ffmpeg"), exe_name("ffprobe")];
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(|err| err.to_string())?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().map_err(|err| err.to_string())?.replace('\\', "/");
        let mut parts = name.rsplit('/');
        let file_name = parts.next().unwrap_or_default().to_string();
        let parent = parts.next().unwrap_or_default();
        let output_name = if parent == "bin" && wanted.contains(&file_name) {
            file_name
        } else if file_name == "LICENSE" && name.matches('/').count() <= 1 {
            "LICENSE.txt".to_string()
        } else {
            continue;
        };
        let mut output = File::create(destination.join(&output_name)).map_err(|err| err.to_string())?;
        let copied = io::copy(&mut Read::by_ref(&mut entry).take(MAX_EXTRACTED_BYTES), &mut output)
            .map_err(|err| format!("Extraction de {output_name} : {err}"))?;
        if copied >= MAX_EXTRACTED_BYTES {
            return Err(format!("{output_name} est anormalement volumineux : installation annulée."));
        }
    }
    if is_complete(destination) { Ok(()) } else { Err("L'archive ne contient pas ffmpeg et ffprobe.".into()) }
}

fn verify_ffmpeg(ffmpeg: &Path) -> Result<(), String> {
    let mut cmd = process::command(ffmpeg);
    cmd.arg("-version");
    let output = process::run_with_timeout(cmd, Duration::from_secs(20))?;
    let text = String::from_utf8_lossy(&output.stdout);
    if output.status.success() && text.starts_with("ffmpeg version") {
        Ok(())
    } else {
        Err("Le FFmpeg téléchargé ne démarre pas : installation annulée.".into())
    }
}

/// Supprime les anciennes versions et les restes de téléchargement.
/// Les fichiers temporaires récents sont épargnés : une autre fenêtre de
/// VideoCut est peut-être en train de les utiliser.
pub(crate) fn remove_old_versions(data_dir: &Path, keep: &str) {
    const IN_PROGRESS_GRACE: Duration = Duration::from_secs(3600);
    let Ok(entries) = fs::read_dir(managed_root(data_dir)) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == keep {
            continue;
        }
        let temporary = [".staging", ".part", ".download.zip"].iter().any(|suffix| name.ends_with(suffix));
        let age = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
        if temporary && age.is_none_or(|age| age < IN_PROGRESS_GRACE) {
            continue;
        }
        let path = entry.path();
        let _ = if path.is_dir() { fs::remove_dir_all(&path) } else { fs::remove_file(&path) };
    }
}

fn check_app_update() -> Result<Option<(String, String)>, String> {
    let release = match fetch_release(APP_RELEASE_API) {
        Ok(release) => release,
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(err) => return Err(network_error("VideoCut", &err)),
    };
    let Some(latest) = sanitize_tag(&release.tag_name) else { return Ok(None) };
    if !is_newer(&latest, env!("CARGO_PKG_VERSION")) {
        return Ok(None);
    }
    let url = if release.html_url.starts_with("https://github.com/") {
        release.html_url
    } else {
        APP_RELEASES_PAGE.to_string()
    };
    Ok(Some((latest, url)))
}

/// Compare des versions numériques `1.2.10` > `1.2.9`.
pub(crate) fn is_newer(candidate: &str, current: &str) -> bool {
    fn parse(version: &str) -> Vec<u64> {
        version
            .split(['.', '-'])
            .map_while(|part| {
                let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
                digits.parse().ok()
            })
            .collect()
    }
    let (a, b) = (parse(candidate), parse(current));
    let len = a.len().max(b.len());
    let pad = |v: &[u64], i: usize| v.get(i).copied().unwrap_or(0);
    (0..len).map(|i| pad(&a, i).cmp(&pad(&b, i))).find(|o| o.is_ne()) == Some(std::cmp::Ordering::Greater)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(names: &[&str]) -> Release {
        Release {
            tag_name: "9.0.2".into(),
            html_url: String::new(),
            assets: names
                .iter()
                .map(|name| Asset { name: (*name).into(), browser_download_url: String::new(), size: 0, digest: None })
                .collect(),
        }
    }

    #[test]
    fn picks_the_essentials_zip() {
        let r = release(&[
            "ffmpeg-9.0.2-essentials_build.7z",
            "ffmpeg-9.0.2-full_build.zip",
            "ffmpeg-9.0.2-essentials_build.zip",
        ]);
        assert_eq!(pick_asset(&r).unwrap().name, "ffmpeg-9.0.2-essentials_build.zip");
        assert!(pick_asset(&release(&["ffmpeg-9.0.2-full_build.zip"])).is_none());
    }

    #[test]
    fn tags_are_sanitized_for_folder_names() {
        assert_eq!(sanitize_tag("9.0.2").as_deref(), Some("9.0.2"));
        assert_eq!(sanitize_tag("v1.4.0").as_deref(), Some("1.4.0"));
        for bad in ["", "..", "../x", "9.0/2", "a\\b", ".hidden", "1 2"] {
            assert!(sanitize_tag(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn version_comparison_is_numeric() {
        assert!(is_newer("1.0.1", "1.0.0"));
        assert!(is_newer("1.10.0", "1.9.9"));
        assert!(is_newer("2", "1.9"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("1.0", "1.0.0"));
        assert!(!is_newer("0.9.9", "1.0.0"));
    }

    #[test]
    fn downloads_are_checked_against_size_and_digest() {
        let data = b"archive ffmpeg";
        let hash = Sha256::digest(data);
        let mut asset = Asset {
            name: "a.zip".into(),
            browser_download_url: String::new(),
            size: data.len() as u64,
            digest: Some(format!("sha256:{}", to_hex(&hash).to_uppercase())),
        };
        assert!(verify_download(&asset, data.len() as u64, &hash).is_ok());
        assert!(verify_download(&asset, 3, &hash).unwrap_err().contains("incomplet"));
        asset.digest = Some(format!("sha256:{}", "0".repeat(64)));
        assert!(verify_download(&asset, data.len() as u64, &hash).unwrap_err().contains("SHA-256"));
        // Sans empreinte publiée : refus (aucune vérification « au mieux »).
        asset.digest = None;
        assert!(verify_download(&asset, data.len() as u64, &hash).unwrap_err().contains("SHA-256"));
        asset.digest = Some("md5:abcdef".into());
        assert!(verify_download(&asset, data.len() as u64, &hash).is_err());
    }

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("videocut-updater-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn make_zip(path: &Path, files: &[(&str, &[u8])]) {
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        let mut writer = zip::ZipWriter::new(File::create(path).unwrap());
        for (name, data) in files {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn extraction_keeps_only_the_needed_files() {
        let dir = test_dir("extract");
        let archive = dir.join("a.zip");
        let ffmpeg = format!("ffmpeg-9.0.2-essentials_build/bin/{}", exe_name("ffmpeg"));
        let ffprobe = format!("ffmpeg-9.0.2-essentials_build/bin/{}", exe_name("ffprobe"));
        make_zip(
            &archive,
            &[
                (ffmpeg.as_str(), b"ff"),
                (ffprobe.as_str(), b"fp"),
                ("ffmpeg-9.0.2-essentials_build/LICENSE", b"GPL"),
                ("ffmpeg-9.0.2-essentials_build/doc/ffmpeg.html", b"doc"),
                ("ffmpeg-9.0.2-essentials_build/bin/ffplay.exe", b"play"),
            ],
        );
        let out = dir.join("out");
        fs::create_dir_all(&out).unwrap();
        extract_tools(&archive, &out).unwrap();
        let mut names: Vec<String> =
            fs::read_dir(&out).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        let mut expected = vec![exe_name("ffmpeg"), exe_name("ffprobe"), "LICENSE.txt".to_string()];
        expected.sort();
        assert_eq!(names, expected);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn extraction_fails_without_both_tools() {
        let dir = test_dir("incomplete");
        let archive = dir.join("a.zip");
        let ffmpeg = format!("x/bin/{}", exe_name("ffmpeg"));
        make_zip(&archive, &[(ffmpeg.as_str(), b"ff")]);
        assert!(extract_tools(&archive, &dir).is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn old_versions_are_cleaned_up() {
        let dir = test_dir("cleanup");
        let root = managed_root(&dir);
        for name in ["8.1", "9.0.2", "9.0.2.staging"] {
            fs::create_dir_all(root.join(name)).unwrap();
        }
        fs::write(root.join("9.0.2.download.part"), b"x").unwrap();
        remove_old_versions(&dir, "9.0.2");
        let mut left: Vec<String> =
            fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        // L'ancienne version part ; les fichiers temporaires récents restent
        // (une autre fenêtre de VideoCut peut être en train de s'en servir).
        assert_eq!(left, ["9.0.2", "9.0.2.download.part", "9.0.2.staging"]);
        let _ = fs::remove_dir_all(dir);
    }
}
