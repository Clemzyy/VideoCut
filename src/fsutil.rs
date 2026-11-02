//! Opérations sur les fichiers tolérantes aux verrous temporaires posés
//! par l'antivirus ou OneDrive juste après l'écriture d'un fichier.

use std::path::Path;
use std::time::Duration;
use std::{fs, io, thread};

/// Renomme (et remplace la destination), en réessayant pendant ~2 s.
pub fn rename_with_retry(from: &Path, to: &Path) -> io::Result<()> {
    let mut attempt = 0;
    loop {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(err) if attempt < 10 && err.kind() != io::ErrorKind::NotFound => {
                attempt += 1;
                thread::sleep(Duration::from_millis(200));
            }
            Err(err) => return Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_and_replaces_the_destination() {
        let dir = std::env::temp_dir().join(format!("videocut-fsutil-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.txt"), dir.join("b.txt"));
        fs::write(&a, "nouveau").unwrap();
        fs::write(&b, "ancien").unwrap();
        rename_with_retry(&a, &b).unwrap();
        assert_eq!(fs::read_to_string(&b).unwrap(), "nouveau");
        assert!(!a.exists());
        // Une source absente échoue tout de suite, sans attendre.
        let started = std::time::Instant::now();
        assert!(rename_with_retry(&a, &b).is_err());
        assert!(started.elapsed() < Duration::from_millis(150));
        let _ = fs::remove_dir_all(dir);
    }
}
