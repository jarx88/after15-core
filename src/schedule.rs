use chrono::{NaiveDate, NaiveTime, DateTime, Local, Datelike, Weekday, Duration};

const FIRST_AFTERNOON_START: (i32, u32, u32) = (2025, 7, 28);
const FIRST_AFTERNOON_END: (i32, u32, u32) = (2025, 8, 2);
const CYCLE_LENGTH_DAYS: i64 = 21;

pub fn is_afternoon_shift_period(date: NaiveDate) -> bool {
    let first_start = NaiveDate::from_ymd_opt(
        FIRST_AFTERNOON_START.0,
        FIRST_AFTERNOON_START.1,
        FIRST_AFTERNOON_START.2
    ).unwrap();
    let first_end = NaiveDate::from_ymd_opt(
        FIRST_AFTERNOON_END.0,
        FIRST_AFTERNOON_END.1,
        FIRST_AFTERNOON_END.2
    ).unwrap();
    
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

pub fn is_overtime_hour(dt: DateTime<Local>) -> bool {
    let date = dt.date_naive();
    let time = dt.time();
    
    match get_regular_work_window(date) {
        Some(window) => time < window.start || time >= window.end,
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_afternoon_shift_first_period() {
        let mon = NaiveDate::from_ymd_opt(2025, 7, 28).unwrap();
        let sat = NaiveDate::from_ymd_opt(2025, 8, 2).unwrap();
        let sun = NaiveDate::from_ymd_opt(2025, 8, 3).unwrap();
        
        assert!(is_afternoon_shift_period(mon));
        assert!(is_afternoon_shift_period(sat));
        assert!(!is_afternoon_shift_period(sun));
    }
    
    #[test]
    fn test_afternoon_shift_second_cycle() {
        let second_cycle_start = NaiveDate::from_ymd_opt(2025, 8, 18).unwrap();
        assert!(is_afternoon_shift_period(second_cycle_start));
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
