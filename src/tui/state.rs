// EditState i logika — uzupełniane w kolejnych taskach.

/// Parsuje godziny z formatu "H:MM" lub ułamka dziesiętnego ("2.5").
/// Zwraca wartość zaokrągloną do 2 miejsc, w zakresie 0..=24.
pub fn parse_hours(input: &str) -> Result<f64, String> {
    let s = input.trim();
    if s.is_empty() {
        return Err("pusta wartość".to_string());
    }
    let val = if let Some((h, m)) = s.split_once(':') {
        let h: i64 = h
            .trim()
            .parse()
            .map_err(|_| "zły format godzin".to_string())?;
        let m: i64 = m.trim().parse().map_err(|_| "złe minuty".to_string())?;
        if !(0..=59).contains(&m) {
            return Err("minuty muszą być 0-59".to_string());
        }
        h as f64 + (m as f64) / 60.0
    } else {
        s.parse::<f64>().map_err(|_| "zły format liczby".to_string())?
    };
    if !(0.0..=24.0).contains(&val) {
        return Err("zakres 0-24h".to_string());
    }
    Ok((val * 100.0).round() / 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hm_format() {
        assert!((parse_hours("1:15").unwrap() - 1.25).abs() < 1e-9);
    }

    #[test]
    fn parse_decimal_format() {
        assert!((parse_hours("2.5").unwrap() - 2.5).abs() < 1e-9);
    }

    #[test]
    fn parse_rounds_to_two_decimals() {
        assert!((parse_hours("0:20").unwrap() - 0.33).abs() < 1e-9);
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(parse_hours("abc").is_err());
        assert!(parse_hours("").is_err());
        assert!(parse_hours("1:90").is_err());
    }

    #[test]
    fn parse_rejects_out_of_range() {
        assert!(parse_hours("25").is_err());
        assert!(parse_hours("-1").is_err());
    }
}
