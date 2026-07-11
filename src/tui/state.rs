// EditState i logika — uzupełniane w kolejnych taskach.

use chrono::NaiveDate;
use std::collections::HashSet;
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

pub struct EditState {
    pub summary: DailySummaryFile,
    pub year: i32,
    pub month: u32,
    pub rows: Vec<DayRow>,
    pub cursor: usize,
    pub editing: Option<String>,
    pub status: String,
    pub dirty: bool,
    pub edited: HashSet<String>,
}

impl EditState {
    pub fn new(summary: DailySummaryFile, year: i32, month: u32) -> Self {
        let rows = build_rows(&summary, year, month);
        EditState {
            summary,
            year,
            month,
            rows,
            cursor: 0,
            editing: None,
            status: String::new(),
            dirty: false,
            edited: HashSet::new(),
        }
    }

    fn reload_rows(&mut self) {
        self.rows = build_rows(&self.summary, self.year, self.month);
        if self.cursor >= self.rows.len() {
            self.cursor = self.rows.len().saturating_sub(1);
        }
    }

    pub fn move_up(&mut self) {
        if self.editing.is_some() { return; }
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_down(&mut self) {
        if self.editing.is_some() { return; }
        if self.cursor + 1 < self.rows.len() {
            self.cursor += 1;
        }
    }

    pub fn prev_month(&mut self) {
        if self.editing.is_some() { return; }
        if self.month == 1 { self.year -= 1; self.month = 12; } else { self.month -= 1; }
        self.cursor = 0;
        self.reload_rows();
    }

    pub fn next_month(&mut self) {
        if self.editing.is_some() { return; }
        if self.month == 12 { self.year += 1; self.month = 1; } else { self.month += 1; }
        self.cursor = 0;
        self.reload_rows();
    }

    pub fn begin_edit(&mut self) {
        if self.rows.is_empty() { return; }
        let row = &self.rows[self.cursor];
        // prefill bieżącą wartością w formacie H:MM (pusta dla nowych/wirtualnych dni)
        let prefill = if !row.existed {
            String::new()
        } else {
            format_hm(row.hours)
        };
        self.editing = Some(prefill);
        self.status.clear();
    }

    pub fn input_char(&mut self, c: char) {
        if let Some(buf) = self.editing.as_mut() {
            if c.is_ascii_digit() || c == ':' || c == '.' {
                buf.push(c);
            }
        }
    }

    pub fn backspace(&mut self) {
        if let Some(buf) = self.editing.as_mut() {
            buf.pop();
        }
    }

    pub fn cancel_edit(&mut self) {
        self.editing = None;
        self.status.clear();
    }

    pub fn commit_edit(&mut self) {
        let Some(buf) = self.editing.clone() else { return; };
        match parse_hours(&buf) {
            Ok(hours) => {
                let row = &mut self.rows[self.cursor];
                row.hours = hours;
                row.manual_override = true;
                row.dirty = true;
                row.existed = true;
                let key = row.date.format("%Y-%m-%d").to_string();
                let shift = row.shift.clone();

                self.edited.insert(key.clone());
                let entry = self.summary.days.entry(key).or_insert_with(|| DayEntry {
                    hours: 0.0,
                    formatted: String::new(),
                    shift: shift.clone(),
                    processed: true,
                    manual_override: false,
                    projects: None, ..Default::default() });
                entry.hours = hours;
                entry.formatted = format_hm(hours);
                entry.processed = true;
                entry.manual_override = true;
                // projects pozostają nietknięte (zachowane dla istniejących dni)

                self.dirty = true;
                self.editing = None;
                self.status = format!("zapisano w pamięci: {}", format_hm(hours));
            }
            Err(e) => {
                self.status = format!("błąd: {}", e);
                // editing pozostaje — użytkownik poprawia
            }
        }
    }

    pub fn toggle_manual(&mut self) {
        if self.editing.is_some() || self.rows.is_empty() { return; }
        let row = &mut self.rows[self.cursor];
        let new_val = !row.manual_override;
        row.manual_override = new_val;
        row.dirty = true;
        row.existed = true;
        let key = row.date.format("%Y-%m-%d").to_string();
        let shift = row.shift.clone();
        let hours = row.hours;

        self.edited.insert(key.clone());
        let entry = self.summary.days.entry(key).or_insert_with(|| DayEntry {
            hours,
            formatted: format_hm(hours),
            shift,
            processed: true,
            manual_override: false,
            projects: None, ..Default::default() });
        entry.manual_override = new_val;
        self.dirty = true;
        self.status = if new_val {
            "flaga manual_override = ON".to_string()
        } else {
            "flaga manual_override = OFF".to_string()
        };
    }

    /// Łączy edycje tej sesji na świeży stan wczytany z dysku (pod lockiem),
    /// przelicza sumy miesięczne i czyści dirty. Po wywołaniu self.summary == zwrócony stan.
    /// Caller musi potem przeładować wiersze (refresh_rows).
    pub fn apply_edits(&mut self, fresh: DailySummaryFile) -> &DailySummaryFile {
        let mut fresh = fresh;
        for key in &self.edited {
            if let Some(entry) = self.summary.days.get(key) {
                fresh.days.insert(key.clone(), entry.clone());
            }
        }
        crate::archive::recalc_months(&mut fresh);
        self.summary = fresh;
        self.dirty = false;
        self.edited.clear();
        &self.summary
    }

    pub fn refresh_rows(&mut self) {
        self.reload_rows();
    }
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
        DayEntry { hours, formatted: format_hm(hours), shift: shift.into(), processed: true, manual_override: manual, projects: None, ..Default::default() }
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

    #[test]
    fn new_starts_on_first_day() {
        let s = DailySummaryFile::default();
        let st = EditState::new(s, 2026, 5);
        assert_eq!(st.cursor, 0);
        assert_eq!(st.rows.len(), 31);
        assert!(!st.dirty);
    }

    #[test]
    fn navigation_clamps() {
        let mut st = EditState::new(DailySummaryFile::default(), 2026, 5);
        st.move_up(); // już na 0 — bez zmian
        assert_eq!(st.cursor, 0);
        for _ in 0..100 { st.move_down(); }
        assert_eq!(st.cursor, st.rows.len() - 1);
    }

    #[test]
    fn month_switch_rebuilds_rows_and_resets_cursor() {
        let mut st = EditState::new(DailySummaryFile::default(), 2026, 5);
        st.cursor = 10;
        st.prev_month();
        assert_eq!(st.year, 2026);
        assert_eq!(st.month, 4);
        assert_eq!(st.cursor, 0);
        assert_eq!(st.rows.len(), 30);
        st.next_month();
        st.next_month();
        assert_eq!(st.month, 6);
    }

    #[test]
    fn month_switch_wraps_year() {
        let mut st = EditState::new(DailySummaryFile::default(), 2026, 12);
        st.next_month();
        assert_eq!((st.year, st.month), (2027, 1));
        st.prev_month();
        st.prev_month();
        assert_eq!((st.year, st.month), (2026, 11));
    }

    #[test]
    fn commit_edit_sets_hours_and_manual_flag() {
        let mut st = EditState::new(DailySummaryFile::default(), 2026, 5);
        st.cursor = 0; // 1 maja, wirtualny
        st.begin_edit();
        assert!(st.editing.is_some());
        for c in "3:00".chars() { st.input_char(c); }
        st.commit_edit();
        assert!(st.editing.is_none());
        let row = &st.rows[0];
        assert!((row.hours - 3.0).abs() < 1e-9);
        assert!(row.manual_override);
        assert!(row.dirty);
        assert!(st.dirty);
        // zapisane do summary
        let e = st.summary.days.get("2026-05-01").unwrap();
        assert!(e.manual_override);
        assert!(e.processed);
        assert_eq!(e.formatted, "3:00");
        assert_eq!(e.shift, row.shift); // shift z automatu zachowany
    }

    #[test]
    fn commit_edit_rejects_invalid_keeps_editing() {
        let mut st = EditState::new(DailySummaryFile::default(), 2026, 5);
        st.begin_edit();
        // begin_edit prefilluje "0:00" — wyczyść i wpisz złą wartość
        st.editing = Some(String::new());
        for c in "99".chars() { st.input_char(c); }
        st.commit_edit();
        assert!(st.editing.is_some()); // nadal edycja
        assert!(!st.dirty);
        assert!(!st.status.is_empty());
    }

    #[test]
    fn cancel_edit_discards() {
        let mut st = EditState::new(DailySummaryFile::default(), 2026, 5);
        st.begin_edit();
        st.input_char('5');
        st.cancel_edit();
        assert!(st.editing.is_none());
        assert!(!st.dirty);
        assert_eq!(st.rows[0].hours, 0.0);
    }

    #[test]
    fn commit_preserves_projects_of_existing_day() {
        let mut s = DailySummaryFile::default();
        let mut proj = std::collections::BTreeMap::new();
        proj.insert("farmaster".to_string(), crate::archive::ProjectHoursEntry::default());
        s.days.insert("2026-05-03".to_string(), DayEntry { hours: 1.0, formatted: "1:00".into(), shift: "regular".into(), processed: true, manual_override: false, projects: Some(proj), ..Default::default() });
        let mut st = EditState::new(s, 2026, 5);
        st.cursor = 2; // 3 maja
        st.begin_edit();
        st.editing = Some(String::new());
        for c in "2:30".chars() { st.input_char(c); }
        st.commit_edit();
        let e = st.summary.days.get("2026-05-03").unwrap();
        assert!((e.hours - 2.5).abs() < 1e-9);
        assert!(e.projects.is_some()); // projekty zachowane
    }

    #[test]
    fn toggle_manual_on_existing_day() {
        let mut s = DailySummaryFile::default();
        s.days.insert("2026-05-04".to_string(), DayEntry { hours: 2.0, formatted: "2:00".into(), shift: "regular".into(), processed: true, manual_override: false, projects: None, ..Default::default() });
        let mut st = EditState::new(s, 2026, 5);
        st.cursor = 3;
        st.toggle_manual();
        assert!(st.rows[3].manual_override);
        assert!(st.summary.days.get("2026-05-04").unwrap().manual_override);
        assert!(st.dirty);
        st.toggle_manual();
        assert!(!st.rows[3].manual_override);
        assert!(!st.summary.days.get("2026-05-04").unwrap().manual_override);
    }

    #[test]
    fn toggle_manual_on_virtual_day_creates_entry() {
        let mut st = EditState::new(DailySummaryFile::default(), 2026, 5);
        st.cursor = 0; // wirtualny
        st.toggle_manual();
        assert!(st.rows[0].manual_override);
        let e = st.summary.days.get("2026-05-01").unwrap();
        assert!(e.manual_override);
        assert_eq!(e.hours, 0.0);
    }

    #[test]
    fn apply_edits_recalcs_months() {
        let mut st = EditState::new(DailySummaryFile::default(), 2026, 5);
        st.begin_edit();
        st.editing = Some(String::new());
        for c in "4:00".chars() { st.input_char(c); }
        st.commit_edit(); // 1 maja = 4h
        st.apply_edits(DailySummaryFile::default());
        assert!((st.summary.months.get("2026-05").unwrap().total_hours - 4.0).abs() < 1e-9);
        assert!(!st.dirty); // dirty wyczyszczone po apply_edits
    }

    #[test]
    fn commit_edit_on_existing_day_overwrites_and_preserves_shift_processed() {
        let mut s = DailySummaryFile::default();
        s.days.insert("2026-05-05".to_string(), DayEntry {
            hours: 1.0,
            formatted: "1:00".into(),
            shift: "afternoon".into(),
            processed: true,
            manual_override: false,
            projects: None, ..Default::default() });
        let mut st = EditState::new(s, 2026, 5);
        st.cursor = 4; // 5 maja (index 4)
        st.begin_edit();
        st.editing = Some(String::new());
        for c in "2:30".chars() { st.input_char(c); }
        st.commit_edit();
        let e = st.summary.days.get("2026-05-05").unwrap();
        assert!((e.hours - 2.5).abs() < 1e-9);
        assert_eq!(e.shift, "afternoon");
        assert!(e.processed);
        assert!(e.manual_override);
    }

    #[test]
    fn parse_hours_boundary() {
        assert!((parse_hours("24:00").unwrap() - 24.0).abs() < 1e-9);
        assert!(parse_hours("24:01").is_err());
    }

    #[test]
    fn apply_edits_merges_onto_concurrent_state() {
        let mut st = EditState::new(DailySummaryFile::default(), 2026, 5);
        st.cursor = 0; // 1 maja
        st.begin_edit();
        st.editing = Some(String::new());
        for c in "3:00".chars() { st.input_char(c); }
        st.commit_edit();

        // "concurrent writer" wrote day 20 but NOT day 1
        let mut fresh = DailySummaryFile::default();
        fresh.days.insert("2026-05-20".to_string(), DayEntry {
            hours: 2.0,
            formatted: "2:00".into(),
            shift: "regular".into(),
            processed: true,
            manual_override: false,
            projects: None, ..Default::default() });

        st.apply_edits(fresh);

        let e01 = st.summary.days.get("2026-05-01").expect("edit day 1 must be present");
        assert!((e01.hours - 3.0).abs() < 1e-9, "edited day should be 3h");
        let e20 = st.summary.days.get("2026-05-20").expect("concurrent day 20 must be present");
        assert!((e20.hours - 2.0).abs() < 1e-9, "concurrent day should be 2h");
    }
}
