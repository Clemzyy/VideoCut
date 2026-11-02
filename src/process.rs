//! Lancement des processus FFmpeg : console masquée, arguments sûrs,
//! arrêt garanti et lecture de stderr sans risque de blocage.

use std::ffi::OsString;
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Protocoles autorisés en entrée : jamais de réseau ni de playlist distante.
pub const PROTOCOL_WHITELIST: &str = "file,pipe";

/// Taille maximale conservée de stderr pour les messages d'erreur.
const STDERR_TAIL_BYTES: usize = 8 * 1024;

/// Prépare une commande sans fenêtre console et sans stdin.
pub fn command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Préfixe `file:` : FFmpeg n'interprète alors jamais le chemin comme une
/// option, un protocole (`concat:`, `http:`…) ou un motif.
pub fn file_arg(path: &Path) -> OsString {
    let mut arg = OsString::from("file:");
    arg.push(path.as_os_str());
    arg
}

/// Arguments communs à toutes les lectures d'un fichier média.
pub fn input_args(path: &Path) -> Vec<OsString> {
    vec!["-protocol_whitelist".into(), PROTOCOL_WHITELIST.into(), "-i".into(), file_arg(path)]
}

/// Lance la commande et rattache le processus au « job » Windows de
/// VideoCut : si VideoCut se ferme, même brutalement, Windows tue FFmpeg.
pub fn spawn(cmd: &mut Command) -> std::io::Result<Child> {
    let child = cmd.spawn()?;
    #[cfg(windows)]
    job::assign(&child);
    Ok(child)
}

#[cfg(windows)]
mod job {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
    };

    struct Job(HANDLE);
    // Le handle n'est jamais fermé ni modifié : Windows le ferme à la sortie.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    fn job() -> Option<&'static Job> {
        static JOB: OnceLock<Option<Job>> = OnceLock::new();
        JOB.get_or_init(|| {
            // SAFETY: appels Win32 documentés, structure initialisée à zéro
            // puis complétée, taille exacte passée à l'API.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return None;
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    (&raw const info).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                (ok != 0).then_some(Job(handle))
            }
        })
        .as_ref()
    }

    pub fn assign(child: &Child) {
        if let Some(job) = job() {
            // SAFETY: le handle du processus enfant est valide tant que `child` vit.
            unsafe {
                AssignProcessToJobObject(job.0, child.as_raw_handle());
            }
        }
    }
}

/// Processus enfant tué automatiquement quand il n'est plus utilisé.
pub struct KillOnDrop(Option<Child>);

impl KillOnDrop {
    pub fn new(child: Child) -> Self {
        Self(Some(child))
    }

    pub fn child_mut(&mut self) -> &mut Child {
        self.0.as_mut().expect("processus déjà libéré")
    }

    /// Attend la fin normale du processus.
    pub fn wait(mut self) -> std::io::Result<std::process::ExitStatus> {
        let mut child = self.0.take().expect("processus déjà libéré");
        child.wait()
    }
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            if matches!(child.try_wait(), Ok(None)) {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
    }
}

/// Vide un flux dans un thread et renvoie ses derniers octets en texte.
pub fn drain_tail<R: Read + Send + 'static>(mut reader: R) -> JoinHandle<String> {
    thread::spawn(move || {
        let mut tail: Vec<u8> = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    tail.extend_from_slice(&buf[..n]);
                    if tail.len() > STDERR_TAIL_BYTES * 2 {
                        tail.drain(..tail.len() - STDERR_TAIL_BYTES);
                    }
                }
            }
        }
        if tail.len() > STDERR_TAIL_BYTES {
            tail.drain(..tail.len() - STDERR_TAIL_BYTES);
        }
        String::from_utf8_lossy(&tail).trim().to_string()
    })
}

/// Exécute une commande en capturant stdout et stderr, avec délai maximal.
pub fn run_with_timeout(cmd: Command, timeout: Duration) -> Result<Output, String> {
    run_with_timeout_or_abort(cmd, timeout, &|| false)
}

/// Comme [`run_with_timeout`], mais le processus est aussi tué dès que
/// `abort` renvoie `true` (demande devenue inutile).
pub fn run_with_timeout_or_abort(
    mut cmd: Command,
    timeout: Duration,
    abort: &dyn Fn() -> bool,
) -> Result<Output, String> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = spawn(&mut cmd).map_err(|err| format!("Impossible de lancer {:?} : {err}", cmd.get_program()))?;
    let stdout = child.stdout.take().expect("stdout capturé");
    let stderr = child.stderr.take().expect("stderr capturé");
    let out_thread = thread::spawn(move || {
        let mut data = Vec::new();
        let mut stdout = stdout;
        let _ = stdout.read_to_end(&mut data);
        data
    });
    let err_thread = drain_tail(stderr);
    let mut guard = KillOnDrop::new(child);
    let deadline = Instant::now() + timeout;
    let status = loop {
        match guard.child_mut().try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                drop(guard);
                return Err(format!("FFmpeg ne répond plus après {} s : opération abandonnée.", timeout.as_secs()));
            }
            Ok(None) if abort() => {
                drop(guard);
                return Err("Opération abandonnée.".into());
            }
            Ok(None) => thread::sleep(Duration::from_millis(15)),
            Err(err) => return Err(format!("Erreur d'attente du processus : {err}")),
        }
    };
    let stdout = out_thread.join().unwrap_or_default();
    let stderr = err_thread.join().unwrap_or_default();
    Ok(Output { status, stdout, stderr: stderr.into_bytes() })
}

/// Résume un message d'erreur FFmpeg pour l'afficher à l'utilisateur.
pub fn summarize_error(stderr: &str, fallback: &str) -> String {
    let lines: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return fallback.to_string();
    }
    let start = lines.len().saturating_sub(12);
    lines[start..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_arg_neutralizes_protocols_and_options() {
        assert_eq!(file_arg(Path::new("-y.mp4")), OsString::from("file:-y.mp4"));
        assert_eq!(file_arg(Path::new(r"C:\vidéos\concat:a.mp4")), OsString::from(r"file:C:\vidéos\concat:a.mp4"));
    }

    #[test]
    fn input_args_always_whitelist_protocols() {
        let args = input_args(Path::new("a.mkv"));
        assert_eq!(args[0], "-protocol_whitelist");
        assert_eq!(args[1], PROTOCOL_WHITELIST);
        assert_eq!(args[2], "-i");
        assert_eq!(args[3], "file:a.mkv");
    }

    #[test]
    fn summarize_keeps_last_meaningful_lines() {
        let text = (0..20).map(|i| format!("ligne {i}")).collect::<Vec<_>>().join("\n\n");
        let summary = summarize_error(&text, "x");
        assert!(summary.starts_with("ligne 8"));
        assert!(summary.ends_with("ligne 19"));
        assert_eq!(summarize_error("  \n ", "défaut"), "défaut");
    }

    #[test]
    fn drain_tail_keeps_only_the_end() {
        let data = vec![b'a'; STDERR_TAIL_BYTES * 3];
        let mut with_marker = data.clone();
        with_marker.extend_from_slice(b"FIN");
        let tail = drain_tail(std::io::Cursor::new(with_marker)).join().unwrap();
        assert!(tail.ends_with("FIN"));
        assert!(tail.len() <= STDERR_TAIL_BYTES);
    }
}
