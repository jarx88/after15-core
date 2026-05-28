use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveTime, Weekday};

const CYCLE_LENGTH_DAYS: i64 = 21;

struct ShiftAnchor {
    start: (i32, u32, u32),
    end: (i32, u32, u32),
    valid_from: Option<(i32, u32, u32)>,
}

const ANCHORS: &[ShiftAnchor] = &[
    ShiftAnchor {
        start: (2025, 7, 28),
        end: (2025, 8, 2),
        valid_from: None,
    },
    ShiftAnchor {
        start: (2026, 4, 20),
        end: (2026, 4, 25),
        valid_from: Some((2026, 4, 6)),
    },
];

fn anchor_date(ymd: (i32, u32, u32)) -> NaiveDate {
    NaiveDate::from_ymd_opt(ymd.0, ymd.1, ymd.2).unwrap()
}

fn active_anchor(date: NaiveDate) -> &'static ShiftAnchor {
    for anchor in ANCHORS.iter().rev() {
        if let Some(vf) = anchor.valid_from {
            if date >= anchor_date(vf) {
                return anchor;
            }
        }
    }
    &ANCHORS[0]
}

pub fn is_afternoon_shift_period(date: NaiveDate) -> bool {
    let anchor = active_anchor(date);
    let first_start = anchor_date(anchor.start);
    let first_end = anchor_date(anchor.end);

    let days_since_first = (date - first_start).num_days();
    if days_since_first >= 0 {
        let cycle_number = days_since_first / CYCLE_LENGTH_DAYS;
        let cycle_start = first_start + Duration::days(cycle_number * CYCLE_LENGTH_DAYS);
        let cycle_end = first_end + Duration::days(cycle_number * CYCLE_LENGTH_DAYS);
        return date >= cycle_start && date <= cycle_end;
    }
    false
}

pub fn is_weekend(date: NaiveDate) -> bool {
    matches!(date.weekday(), Weekday::Sat | Weekday::Sun)
}

pub fn is_saturday(date: NaiveDate) -> bool {
    date.weekday() == Weekday::Sat
}

pub fn is_saturday_regular_hours(date: NaiveDate) -> bool {
    is_saturday(date) && is_afternoon_shift_period(date)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShiftType {
    Regular,
    Afternoon,
    Weekend,
    SaturdayAfternoon,
}

pub fn get_shift_type(date: NaiveDate) -> ShiftType {
    if is_weekend(date) {
        if is_saturday_regular_hours(date) {
            ShiftType::SaturdayAfternoon
        } else {
            ShiftType::Weekend
        }
    } else if is_afternoon_shift_period(date) {
        ShiftType::Afternoon
    } else {
        ShiftType::Regular
    }
}

#[derive(Clone, Copy)]
pub struct WorkWindow {
    pub start: NaiveTime,
    pub end: NaiveTime,
}

pub fn get_regular_work_window(date: NaiveDate) -> Option<WorkWindow> {
    match get_shift_type(date) {
        ShiftType::Regular => Some(WorkWindow {
            start: NaiveTime::from_hms_opt(6, 0, 0).unwrap(),
            end: NaiveTime::from_hms_opt(15, 0, 0).unwrap(),
        }),
        ShiftType::Afternoon => Some(WorkWindow {
            start: NaiveTime::from_hms_opt(15, 0, 0).unwrap(),
            end: NaiveTime::from_hms_opt(21, 0, 0).unwrap(),
        }),
        ShiftType::SaturdayAfternoon => Some(WorkWindow {
            start: NaiveTime::from_hms_opt(8, 0, 0).unwrap(),
            end: NaiveTime::from_hms_opt(14, 0, 0).unwrap(),
        }),
        ShiftType::Weekend => None,
    }
}

pub fn is_overtime_hour(dt: DateTime<Local>, work_window_override: Option<WorkWindow>) -> bool {
    let date = dt.date_naive();
    let time = dt.time();

    match work_window_override.or_else(|| get_regular_work_window(date)) {
        Some(window) => time < window.start || time >= window.end,
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_old_anchor_first_period() {
        let mon = NaiveDate::from_ymd_opt(2025, 7, 28).unwrap();
        let sat = NaiveDate::from_ymd_opt(2025, 8, 2).unwrap();
        let sun = NaiveDate::from_ymd_opt(2025, 8, 3).unwrap();

        assert!(is_afternoon_shift_period(mon));
        assert!(is_afternoon_shift_period(sat));
        assert!(!is_afternoon_shift_period(sun));
    }

    #[test]
    fn test_old_anchor_second_cycle() {
        let second_cycle_start = NaiveDate::from_ymd_opt(2025, 8, 18).unwrap();
        assert!(is_afternoon_shift_period(second_cycle_start));
    }

    #[test]
    fn test_old_anchor_last_valid_period() {
        // March 16-21 was the last afternoon shift under old anchor
        let mar16 = NaiveDate::from_ymd_opt(2026, 3, 16).unwrap();
        let mar21 = NaiveDate::from_ymd_opt(2026, 3, 21).unwrap();
        assert!(is_afternoon_shift_period(mar16));
        assert!(is_afternoon_shift_period(mar21));
    }

    #[test]
    fn test_morning_shift_apr6_to_apr19() {
        // April 6-19: new anchor is active but Apr 20 cycle hasn't started
        let apr7 = NaiveDate::from_ymd_opt(2026, 4, 7).unwrap();
        let apr19 = NaiveDate::from_ymd_opt(2026, 4, 19).unwrap();
        assert!(!is_afternoon_shift_period(apr7));
        assert!(!is_afternoon_shift_period(apr19));
    }

    #[test]
    fn test_new_anchor_starts_apr20() {
        let apr20 = NaiveDate::from_ymd_opt(2026, 4, 20).unwrap();
        let apr25 = NaiveDate::from_ymd_opt(2026, 4, 25).unwrap();
        let apr26 = NaiveDate::from_ymd_opt(2026, 4, 26).unwrap();
        let apr27 = NaiveDate::from_ymd_opt(2026, 4, 27).unwrap();

        assert!(is_afternoon_shift_period(apr20));
        assert!(is_afternoon_shift_period(apr25));
        assert!(!is_afternoon_shift_period(apr26)); // Sunday
        assert!(!is_afternoon_shift_period(apr27));
    }

    #[test]
    fn test_new_anchor_second_cycle() {
        // 21 days after Apr 20 = May 11, 2026
        let may11 = NaiveDate::from_ymd_opt(2026, 5, 11).unwrap();
        let may16 = NaiveDate::from_ymd_opt(2026, 5, 16).unwrap();
        let may17 = NaiveDate::from_ymd_opt(2026, 5, 17).unwrap();

        assert!(is_afternoon_shift_period(may11));
        assert!(is_afternoon_shift_period(may16));
        assert!(!is_afternoon_shift_period(may17));
    }

    #[test]
    fn test_gap_between_anchors() {
        // Apr 13-19 should NOT be afternoon (between old and new schedule)
        let apr13 = NaiveDate::from_ymd_opt(2026, 4, 13).unwrap();
        let apr19 = NaiveDate::from_ymd_opt(2026, 4, 19).unwrap();
        assert!(!is_afternoon_shift_period(apr13));
        assert!(!is_afternoon_shift_period(apr19));
    }

    #[test]
    fn test_regular_week() {
        let regular_day = NaiveDate::from_ymd_opt(2025, 8, 4).unwrap();
        assert!(!is_afternoon_shift_period(regular_day));
        assert_eq!(get_shift_type(regular_day), ShiftType::Regular);
    }

    #[test]
    fn test_weekend() {
        let sunday = NaiveDate::from_ymd_opt(2025, 8, 10).unwrap();
        assert!(is_weekend(sunday));
        assert_eq!(get_shift_type(sunday), ShiftType::Weekend);
    }

    #[test]
    fn test_saturday_during_afternoon_shift() {
        let sat = NaiveDate::from_ymd_opt(2025, 8, 2).unwrap();
        assert!(is_saturday_regular_hours(sat));
        assert_eq!(get_shift_type(sat), ShiftType::SaturdayAfternoon);
    }
}
