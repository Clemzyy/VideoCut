//! Formatage et analyse des horodatages affichés dans l'interface.

/// Formate une durée en `MM:SS.mmm`, ou `HH:MM:SS.mmm` au-delà d'une heure.
///
/// L'arrondi se fait sur les millisecondes entières, si bien que 59,9996 s
/// donne `01:00.000` et jamais `00:60.000`.
pub fn format_time(seconds: f64) -> String {
    let total_ms = if seconds.is_finite() && seconds > 0.0 { (seconds * 1000.0).round() as u64 } else { 0 };
    let ms = total_ms % 1000;
    let total_s = total_ms / 1000;
    let s = total_s % 60;
    let m = (total_s / 60) % 60;
    let h = total_s / 3600;
    if h > 0 { format!("{h:02}:{m:02}:{s:02}.{ms:03}") } else { format!("{m:02}:{s:02}.{ms:03}") }
}

/// Analyse une saisie utilisateur : `SS`, `MM:SS` ou `HH:MM:SS`, avec
/// décimales optionnelles (point ou virgule).
pub fn parse_time(text: &str) -> Result<f64, String> {
    let text = text.trim().replace(',', ".");
    if text.is_empty() {
        return Err("Saisis une heure, par exemple 01:23.500.".into());
    }
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() > 3 {
        return Err("Utilise SS, MM:SS ou HH:MM:SS.".into());
    }
    let mut seconds = 0.0;
    for (index, part) in parts.iter().enumerate() {
        let valid =
            !part.is_empty() && part.chars().all(|c| c.is_ascii_digit() || c == '.') && part.matches('.').count() <= 1;
        if !valid {
            return Err(format!("« {part} » n'est pas un nombre valide."));
        }
        let value: f64 = part.parse().map_err(|_| format!("« {part} » n'est pas un nombre valide."))?;
        let is_last = index == parts.len() - 1;
        if index > 0 && value >= 60.0 {
            return Err("Les minutes et les secondes doivent être inférieures à 60.".into());
        }
        if !is_last && part.contains('.') {
            return Err("Seules les secondes peuvent avoir des décimales.".into());
        }
        seconds = seconds * 60.0 + value;
    }
    Ok(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_short_and_long_durations() {
        assert_eq!(format_time(0.0), "00:00.000");
        assert_eq!(format_time(83.5), "01:23.500");
        assert_eq!(format_time(3723.042), "01:02:03.042");
    }

    #[test]
    fn rounding_never_produces_sixty_seconds() {
        assert_eq!(format_time(59.9996), "01:00.000");
        assert_eq!(format_time(3599.9999), "01:00:00.000");
    }

    #[test]
    fn invalid_inputs_format_as_zero() {
        assert_eq!(format_time(-3.0), "00:00.000");
        assert_eq!(format_time(f64::NAN), "00:00.000");
        assert_eq!(format_time(f64::INFINITY), "00:00.000");
    }

    #[test]
    fn parses_all_supported_forms() {
        assert_eq!(parse_time("42").unwrap(), 42.0);
        assert_eq!(parse_time("1:02,5").unwrap(), 62.5);
        assert_eq!(parse_time(" 01:00:01.250 ").unwrap(), 3601.25);
        assert_eq!(parse_time("125.5").unwrap(), 125.5);
    }

    #[test]
    fn round_trips_formatted_values() {
        for value in [0.0, 1.001, 59.999, 3600.5, 7322.123] {
            let parsed = parse_time(&format_time(value)).unwrap();
            assert!((parsed - value).abs() < 0.0006, "{value} -> {parsed}");
        }
    }

    #[test]
    fn rejects_invalid_inputs() {
        for bad in ["", "abc", "-5", "nan", "inf", "1:2:3:4", "1:75", "1.5:10", "1..2", "1:"] {
            assert!(parse_time(bad).is_err(), "{bad:?} aurait dû être refusé");
        }
    }
}
