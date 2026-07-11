use chrono::{Duration, NaiveDate, NaiveTime};
use chrono_tz::Europe::Warsaw;
use std::collections::HashMap;

use crate::config::Config;
use crate::jsonl::Session;
use crate::schedule::{get_regular_work_window, get_shift_type, ShiftType};

pub fn calculate_session_overtime(
    session: &Session,
    _filter_date: NaiveDate,
    config: &Config,
    debug: bool,
) -> HashMap<NaiveDate, f64> {
    let mut daily: HashMap<NaiveDate, f64> = HashMap::new();

    let start_utc = session.start_time;
    let end_utc = session.end_time;

    let start_local = start_utc.and_utc().with_timezone(&Warsaw).naive_local();
    let end_local = end_utc.and_utc().with_timezone(&Warsaw).naive_local();

    let mut current_date = start_local.date();
    let end_date = end_local.date();

    while current_date <= end_date {
        let day_start = current_date.and_hms_opt(0, 0, 0).unwrap();
        let day_end = current_date.and_hms_opt(23, 59, 59).unwrap();

        let block_start = start_local.max(day_start);
        let block_end = end_local.min(day_end);

        if block_end > block_start {
            let overtime_seconds = calculate_overtime_for_day(
                current_date,
                block_start.time(),
                block_end.time(),
                config,
            );

            if overtime_seconds > 0.0 {
                let hours = overtime_seconds / 3600.0;
                *daily.entry(current_date).or_insert(0.0) += hours;

                if debug {
                    eprintln!("[DEBUG] {} overtime: {:.2}h", current_date, hours);
                }
            }
        }

        current_date += Duration::days(1);
    }

    daily
}

pub fn calculate_session_regular(
    session: &Session,
    config: &Config,
) -> HashMap<NaiveDate, f64> {
    let mut daily: HashMap<NaiveDate, f64> = HashMap::new();

    let start_local = session
        .start_time
        .and_utc()
        .with_timezone(&Warsaw)
        .naive_local();
    let end_local = session
        .end_time
        .and_utc()
        .with_timezone(&Warsaw)
        .naive_local();

    let mut current_date = start_local.date();
    let end_date = end_local.date();

    while current_date <= end_date {
        let day_start = current_date.and_hms_opt(0, 0, 0).unwrap();
        let day_end = current_date.and_hms_opt(23, 59, 59).unwrap();

        let block_start = start_local.max(day_start);
        let block_end = end_local.min(day_end);

        if block_end > block_start {
            let regular_seconds = calculate_regular_for_day(
                current_date,
                block_start.time(),
                block_end.time(),
                config,
            );

            if regular_seconds > 0.0 {
                *daily.entry(current_date).or_insert(0.0) += regular_seconds / 3600.0;
            }
        }

        current_date += Duration::days(1);
    }

    daily
}

fn calculate_regular_for_day(
    date: NaiveDate,
    start: NaiveTime,
    end: NaiveTime,
    config: &Config,
) -> f64 {
    let work_window = config.effective_work_window(date);

    if let Some(window) = work_window {
        let regular_start = start.max(window.start);
        let regular_end = end.min(window.end);
        if regular_end > regular_start {
            (regular_end - regular_start).num_seconds() as f64
        } else {
            0.0
        }
    } else {
        0.0
    }
}

fn calculate_overtime_for_day(
    date: NaiveDate,
    start: NaiveTime,
    end: NaiveTime,
    config: &Config,
) -> f64 {
    let work_window = config.effective_work_window(date);

    if let Some(window) = work_window {
        let mut overtime_secs = 0.0;

        if start < window.start {
            let overtime_end = end.min(window.start);
            overtime_secs += (overtime_end - start).num_seconds() as f64;
        }

        if end > window.end {
            let overtime_start = start.max(window.end);
            overtime_secs += (end - overtime_start).num_seconds() as f64;
        }

        overtime_secs
    } else {
        match get_shift_type(date) {
            ShiftType::Weekend => (end - start).num_seconds() as f64,
            ShiftType::Regular | ShiftType::Afternoon | ShiftType::SaturdayAfternoon => {
                (end - start).num_seconds() as f64
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, WorkWindowOverride};

    fn config_with_override(date: NaiveDate, start: NaiveTime, end: NaiveTime) -> Config {
        Config {
            work_window_overrides: vec![WorkWindowOverride { date, start, end }],
            ..Config::default()
        }
    }

    fn config_with_shift(date: NaiveDate, shift: &str) -> Config {
        Config {
            shift_overrides: vec![crate::config::ShiftOverride {
                from: date,
                to: date,
                shift: shift.to_string(),
            }],
            ..Config::default()
        }
    }

    #[test]
    fn shift_override_weekend_makes_weekday_fully_overtime() {
        // Monday 2025-08-04 is a regular day; override marks it weekend-like.
        let date = NaiveDate::from_ymd_opt(2025, 8, 4).unwrap();
        let config = config_with_shift(date, "weekend");
        let start = NaiveTime::from_hms_opt(8, 0, 0).unwrap();
        let end = NaiveTime::from_hms_opt(12, 0, 0).unwrap();
        assert_eq!(
            calculate_overtime_for_day(date, start, end, &config),
            4.0 * 3600.0
        );
    }

    #[test]
    fn shift_override_afternoon_moves_window() {
        // Regular Monday forced to afternoon (15-21): morning work is overtime.
        let date = NaiveDate::from_ymd_opt(2025, 8, 4).unwrap();
        let config = config_with_shift(date, "afternoon");
        let start = NaiveTime::from_hms_opt(8, 0, 0).unwrap();
        let end = NaiveTime::from_hms_opt(16, 0, 0).unwrap();
        assert_eq!(
            calculate_overtime_for_day(date, start, end, &config),
            7.0 * 3600.0
        );
    }

    #[test]
    fn test_regular_day_no_overtime() {
        let date = NaiveDate::from_ymd_opt(2025, 8, 4).unwrap();
        let start = NaiveTime::from_hms_opt(8, 0, 0).unwrap();
        let end = NaiveTime::from_hms_opt(14, 0, 0).unwrap();

        let overtime = calculate_overtime_for_day(date, start, end, &Config::default());
        assert_eq!(overtime, 0.0);
    }

    #[test]
    fn test_regular_day_with_overtime() {
        let date = NaiveDate::from_ymd_opt(2025, 8, 4).unwrap();
        let start = NaiveTime::from_hms_opt(14, 0, 0).unwrap();
        let end = NaiveTime::from_hms_opt(17, 0, 0).unwrap();

        let overtime = calculate_overtime_for_day(date, start, end, &Config::default());
        assert_eq!(overtime, 2.0 * 3600.0);
    }

    #[test]
    fn test_weekend_all_overtime() {
        let date = NaiveDate::from_ymd_opt(2025, 8, 10).unwrap();
        let start = NaiveTime::from_hms_opt(10, 0, 0).unwrap();
        let end = NaiveTime::from_hms_opt(14, 0, 0).unwrap();

        let overtime = calculate_overtime_for_day(date, start, end, &Config::default());
        assert_eq!(overtime, 4.0 * 3600.0);
    }

    #[test]
    fn test_afternoon_shift_before_15() {
        let date = NaiveDate::from_ymd_opt(2025, 7, 28).unwrap();
        let start = NaiveTime::from_hms_opt(10, 0, 0).unwrap();
        let end = NaiveTime::from_hms_opt(14, 0, 0).unwrap();

        let overtime = calculate_overtime_for_day(date, start, end, &Config::default());
        assert_eq!(overtime, 4.0 * 3600.0);
    }

    #[test]
    fn test_work_window_override_disables_overtime_for_matching_window() {
        let date = NaiveDate::from_ymd_opt(2026, 3, 11).unwrap();
        let start = NaiveTime::from_hms_opt(15, 0, 0).unwrap();
        let end = NaiveTime::from_hms_opt(21, 0, 0).unwrap();
        let config = config_with_override(date, start, end);

        let overtime = calculate_overtime_for_day(date, start, end, &config);

        assert_eq!(overtime, 0.0);
    }

    #[test]
    fn test_work_window_override_applies_on_weekend() {
        let date = NaiveDate::from_ymd_opt(2026, 3, 15).unwrap();
        let config = config_with_override(
            date,
            NaiveTime::from_hms_opt(10, 0, 0).unwrap(),
            NaiveTime::from_hms_opt(14, 0, 0).unwrap(),
        );

        let overtime = calculate_overtime_for_day(
            date,
            NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            NaiveTime::from_hms_opt(15, 0, 0).unwrap(),
            &config,
        );

        assert_eq!(overtime, 2.0 * 3600.0);
    }
}
