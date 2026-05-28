// EditState i logika — uzupełniane w kolejnych taskach.

use chrono::{Datelike, NaiveDate};
use crate::archive::{format_hm, DailySummaryFile, DayEntry};
use crate::schedule;

#[derive(Clone)]
pub struct DayRow {
    pub date: NaiveDate,
    pub hours: f64,
    pub shift: String,
    pub manual_override: bool,
    pub existed: bool,
    pub dirty: bool,
}

pub fn days_in_month(year: i32, month: u32) -> u32 {
    let (ny, nm) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    let first_next = NaiveDate::from_ymd_opt(ny, nm, 1).unwrap();
    let first_this = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
    first_next.signed_duration_since(first_this).num_days() as u32
}

pub fn build_rows(summary: &DailySummaryFile, year: i32, month: u32) -> Vec<DayRow> {
    let n = days_in_month(year, month);
    let mut rows = Vec::with_capacity(n as usize);
    for d in 1..=n {
        let date = NaiveDate::from_ymd_opt(year, month, d).unwrap();
        let key = date.format("%Y-%m-%d").to_string();
        if let Some(e) = summary.days.get(&key) {
            rows.push(DayRow {
                date,
                hours: e.hours,
                shift: e.shift.clone(),
                manual_override: e.manual_override,
                existed: true,
                dirty: false,
            });
        } else {
            rows.push(DayRow {
                date,
                hours: 0.0,
                shift: schedule::shift_str(schedule::get_shift_type(date)).to_string(),
                manual_override: false,
                existed: false,
                dirty: false,
            });
        }
    }
    rows
}

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
    use crate::archive::{DailySummaryFile, DayEntry};

    fn day(hours: f64, shift: &str, manual: bool) -> DayEntry {
        DayEntry { hours, formatted: format_hm(hours), shift: shift.into(), processed: true, manual_override: manual, projects: None }
    }

    #[test]
    fn days_in_month_handles_february_leap() {
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2026, 2), 28);
        assert_eq!(days_in_month(2026, 12), 31);
    }

    #[test]
    fn build_rows_marks_existing_and_virtual() {
        let mut s = DailySummaryFile::default();
        s.days.insert("2026-05-02".to_string(), day(2.0, "afternoon", true));
        let rows = build_rows(&s, 2026, 5);
        assert_eq!(rows.len(), 31);
        let r2 = &rows[1]; // 2 maja
        assert!(r2.existed);
        assert!(r2.manual_override);
        assert!((r2.hours - 2.0).abs() < 1e-9);
        assert_eq!(r2.shift, "afternoon");
        let r1 = &rows[0]; // 1 maja — wirtualny
        assert!(!r1.existed);
        assert_eq!(r1.hours, 0.0);
    }

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
