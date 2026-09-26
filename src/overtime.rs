use chrono::{Duration, NaiveDate, NaiveTime, TimeZone};
use chrono_tz::Europe::Warsaw;
use std::collections::HashMap;

use crate::config::Config;
use crate::jsonl::Session;

pub fn session_exclusion_key(session: &Session, date: NaiveDate) -> String {
    let midnight = |date: NaiveDate| {
        Warsaw
            .from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
            .single()
            .unwrap()
            .naive_utc()
    };
    let start = session.start_time.max(midnight(date));
    let end = session.end_time.min(midnight(date.succ_opt().unwrap()));
    format!(
        "{}→{}",
        start.and_utc().to_rfc3339(),
        end.and_utc().to_rfc3339()
    )
}

pub fn session_exclusion_range(
    date: NaiveDate,
    key: &str,
) -> Option<(chrono::NaiveDateTime, chrono::NaiveDateTime)> {
    let (start, end) = key.split_once('→')?;
    if start.contains('T') || end.contains('T') {
        let start = chrono::DateTime::parse_from_rfc3339(start)
            .ok()?
            .with_timezone(&Warsaw);
        let next_midnight = date.succ_opt()?.and_hms_opt(0, 0, 0)?;
        let end = if end == "*" {
            Warsaw.from_local_datetime(&next_midnight).single()?
        } else {
            chrono::DateTime::parse_from_rfc3339(end)
                .ok()?
                .with_timezone(&Warsaw)
        };
        return (end > start
            && start.date_naive() == date
            && (end.date_naive() == date || end.naive_local() == next_midnight))
            .then_some((start.naive_utc(), end.naive_utc()));
    }
    let parse = |value: &str| {
        NaiveTime::parse_from_str(value, "%H:%M:%S%.f")
            .or_else(|_| NaiveTime::parse_from_str(value, "%H:%M"))
    };
    let start = parse(start).ok()?;
    let midnight = date.succ_opt()?.and_hms_opt(0, 0, 0)?;
    let end = if end == "24:00" {
        midnight
    } else {
        let time = parse(end).ok()?;
        let local = date.and_time(time);
        if end.len() == 5 {
            (local + Duration::minutes(1)).min(midnight)
        } else {
            local
        }
    };
    let start = Warsaw
        .from_local_datetime(&date.and_time(start))
        .earliest()?;
    let end = Warsaw.from_local_datetime(&end).latest()?;
    let (start, end) = (start.naive_utc(), end.naive_utc());
    (end > start).then_some((start, end))
}

pub fn exclude_session_ranges(
    session: &Session,
    exclusions: &[(NaiveDate, String)],
) -> Vec<Session> {
    let mut parts = vec![session.clone()];
    for (date, key) in exclusions {
        let Some((start, end)) = session_exclusion_range(*date, key) else {
            continue;
        };
        if key.ends_with("→*") && !(session.start_time <= start && start < session.end_time) {
            continue;
        }
        parts = parts
            .into_iter()
            .flat_map(|part| {
                if end <= part.start_time || start >= part.end_time {
                    return vec![part];
                }
                [
                    (part.start_time, start.min(part.end_time)),
                    (end.max(part.start_time), part.end_time),
                ]
                .into_iter()
                .filter(|(start, end)| end > start)
                .map(|(start_time, end_time)| Session {
                    start_time,
                    end_time,
                    duration_seconds: (end_time - start_time).num_seconds(),
                    ..part.clone()
                })
                .collect()
            })
            .collect();
    }
    parts
}

/// Whether the stored exclusion `key` is what removed the excluded segment `start..end`.
pub fn exclusion_covers_segment(
    date: NaiveDate,
    key: &str,
    start: chrono::NaiveDateTime,
    end: chrono::NaiveDateTime,
) -> bool {
    let Some((from, to)) = session_exclusion_range(date, key) else {
        return false;
    };
    if key.ends_with("→*") {
        // Anchored keys run to midnight but only cut their own session, from the anchor on.
        return start <= from && from < end;
    }
    from < end && to > start
}

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

pub fn calculate_session_regular(session: &Session, config: &Config) -> HashMap<NaiveDate, f64> {
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
        // Brak okna pracy (weekend, B2B weekend) = caly czas to godziny dodatkowe.
        (end - start).num_seconds() as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, WorkWindowOverride};

    #[test]
    fn exclusions_keep_new_tail_and_segments_without_minimum_duration() {
        let date = NaiveDate::from_ymd_opt(2026, 9, 8).unwrap();
        let session = Session {
            id: String::new(),
            project: "proj".into(),
            project_counts: HashMap::from([("proj".into(), 4)]),
            start_time: date.and_hms_opt(14, 0, 0).unwrap(),
            end_time: date.and_hms_opt(15, 2, 0).unwrap(),
            duration_seconds: 3720,
            has_claude: true,
            has_codex: false,
        };
        let parts = exclude_session_ranges(
            &session,
            &[
                (date, "16:00:00→16:40:00".into()),
                (date, "16:30:00→17:00:00".into()),
            ],
        );
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].start_time, date.and_hms_opt(15, 0, 0).unwrap());
        assert_eq!(parts[0].duration_seconds, 120);
        assert_eq!(parts[0].project_counts, session.project_counts);
        let later = Session {
            start_time: date.and_hms_opt(16, 0, 0).unwrap(),
            end_time: date.and_hms_opt(17, 0, 0).unwrap(),
            ..session
        };
        assert_eq!(
            exclude_session_ranges(&later, &[(date, "16:00:00→17:00:00".into())])[0].start_time,
            later.start_time
        );
    }

    #[test]
    fn anchored_exclusion_follows_growth_but_keeps_next_session() {
        let date = NaiveDate::from_ymd_opt(2026, 9, 8).unwrap();
        let session = Session {
            id: String::new(),
            project: "proj".into(),
            project_counts: HashMap::new(),
            start_time: date.and_hms_opt(14, 0, 0).unwrap(),
            end_time: date.and_hms_opt(16, 0, 0).unwrap(),
            duration_seconds: 7200,
            has_claude: true,
            has_codex: false,
        };
        let exclusions = [(date, "2026-09-08T14:00:00+00:00→*".into())];
        assert!(exclude_session_ranges(&session, &exclusions).is_empty());
        let later = Session {
            start_time: date.and_hms_opt(17, 0, 0).unwrap(),
            end_time: date.and_hms_opt(18, 0, 0).unwrap(),
            duration_seconds: 3600,
            ..session
        };
        assert_eq!(
            exclude_session_ranges(&later, &exclusions)[0].start_time,
            later.start_time
        );
    }

    #[test]
    fn restoring_segment_matches_only_its_own_exclusion() {
        let date = NaiveDate::from_ymd_opt(2026, 9, 8).unwrap();
        let at = |h, m| date.and_hms_opt(h, m, 0).unwrap();
        let first = "2026-09-08T14:00:00+00:00→*";
        let second = "2026-09-08T17:00:00+00:00→*";
        // The earlier anchor reaches midnight, yet must not be lifted by restoring a later segment.
        assert!(!exclusion_covers_segment(date, first, at(17, 0), at(18, 0)));
        assert!(exclusion_covers_segment(date, second, at(17, 0), at(18, 0)));
        assert!(exclusion_covers_segment(date, first, at(14, 0), at(16, 0)));
        // Legacy keys are Warsaw wall-clock times (UTC+2 here).
        assert!(exclusion_covers_segment(
            date,
            "18:30:00→19:30:00",
            at(17, 0),
            at(18, 0)
        ));
        assert!(!exclusion_covers_segment(
            date,
            "18:00:00→19:00:00",
            at(17, 0),
            at(18, 0)
        ));
    }

    #[test]
    fn exact_exclusion_does_not_remove_repeated_dst_hour() {
        let date = NaiveDate::from_ymd_opt(2026, 10, 25).unwrap();
        let session = Session {
            id: String::new(),
            project: "proj".into(),
            project_counts: HashMap::new(),
            start_time: date.and_hms_opt(0, 10, 0).unwrap(),
            end_time: date.and_hms_opt(0, 20, 0).unwrap(),
            duration_seconds: 600,
            has_claude: true,
            has_codex: false,
        };
        let key = session_exclusion_key(&session, date);
        let later = Session {
            start_time: session.start_time + Duration::hours(1),
            end_time: session.end_time + Duration::hours(1),
            ..session.clone()
        };
        assert_eq!(
            exclude_session_ranges(&later, &[(date, key.clone())]).len(),
            1
        );
        assert!(exclude_session_ranges(&session, &[(date, key.clone())]).is_empty());
        assert!(session_exclusion_range(date.succ_opt().unwrap(), &key).is_none());
    }

    #[test]
    fn exclusions_support_legacy_minutes_exact_fractions_and_midnight() {
        let date = NaiveDate::from_ymd_opt(2026, 9, 8).unwrap();
        let mut session = Session {
            id: String::new(),
            project: "proj".into(),
            project_counts: HashMap::new(),
            start_time: date.and_hms_milli_opt(21, 59, 30, 125).unwrap(),
            end_time: date.and_hms_opt(22, 2, 0).unwrap(),
            duration_seconds: 149,
            has_claude: true,
            has_codex: false,
        };
        assert_eq!(
            session_exclusion_key(&session, date),
            "2026-09-08T21:59:30.125+00:00→2026-09-08T22:00:00+00:00"
        );
        let parts = exclude_session_ranges(&session, &[(date, "23:59→23:59".into())]);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].start_time, date.and_hms_opt(22, 0, 0).unwrap());
        session.end_time = date.and_hms_milli_opt(21, 59, 50, 250).unwrap();
        let key = session_exclusion_key(&session, date);
        assert_eq!(
            key,
            "2026-09-08T21:59:30.125+00:00→2026-09-08T21:59:50.250+00:00"
        );
        assert!(exclude_session_ranges(&session, &[(date, key)]).is_empty());
        assert!(
            exclude_session_ranges(&session, &[(date, "23:59:30.125→24:00".into())]).is_empty()
        );
    }

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

    fn b2b_config() -> Config {
        Config {
            billing: crate::config::BillingConfig {
                b2b_from: Some(NaiveDate::from_ymd_opt(2026, 9, 2).unwrap()),
                ..Default::default()
            },
            ..Config::default()
        }
    }

    #[test]
    fn b2b_weekday_counts_hours_outside_07_15() {
        let date = NaiveDate::from_ymd_opt(2026, 9, 3).unwrap(); // czwartek
        let start = NaiveTime::from_hms_opt(5, 30, 0).unwrap();
        let end = NaiveTime::from_hms_opt(16, 30, 0).unwrap();
        let cfg = b2b_config();
        assert_eq!(
            calculate_overtime_for_day(date, start, end, &cfg),
            3.0 * 3600.0
        );
        assert_eq!(
            calculate_regular_for_day(date, start, end, &cfg),
            8.0 * 3600.0
        );
    }

    #[test]
    fn b2b_weekend_is_all_extra() {
        let date = NaiveDate::from_ymd_opt(2026, 9, 5).unwrap(); // sobota
        let start = NaiveTime::from_hms_opt(10, 0, 0).unwrap();
        let end = NaiveTime::from_hms_opt(12, 0, 0).unwrap();
        let cfg = b2b_config();
        assert_eq!(
            calculate_overtime_for_day(date, start, end, &cfg),
            2.0 * 3600.0
        );
        assert_eq!(calculate_regular_for_day(date, start, end, &cfg), 0.0);
    }

    #[test]
    fn before_cutover_keeps_old_shift_logic() {
        let date = NaiveDate::from_ymd_opt(2026, 8, 27).unwrap();
        let start = NaiveTime::from_hms_opt(5, 30, 0).unwrap();
        let end = NaiveTime::from_hms_opt(16, 30, 0).unwrap();
        let cfg = b2b_config();
        assert!(!cfg.is_b2b(date));
        let expected = match crate::schedule::get_shift_type(date) {
            // regularna 06-15: 0:30 przed + 1:30 po
            crate::schedule::ShiftType::Regular => 2.0 * 3600.0,
            // popoludniowa 15-21: cale 5:30 przed oknem
            crate::schedule::ShiftType::Afternoon => 9.5 * 3600.0,
            other => panic!("nieoczekiwana zmiana w dzien roboczy: {:?}", other),
        };
        assert_eq!(calculate_overtime_for_day(date, start, end, &cfg), expected);
        assert_ne!(
            calculate_overtime_for_day(date, start, end, &cfg),
            3.0 * 3600.0
        );
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
