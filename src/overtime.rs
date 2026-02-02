use chrono::{NaiveDate, NaiveTime, Duration};
use chrono_tz::Europe::Warsaw;
use std::collections::HashMap;

use crate::schedule::{get_shift_type, get_regular_work_window, ShiftType};
use crate::jsonl::Session;

pub fn calculate_session_overtime(session: &Session, _filter_date: NaiveDate, debug: bool) -> HashMap<NaiveDate, f64> {
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

fn calculate_overtime_for_day(date: NaiveDate, start: NaiveTime, end: NaiveTime) -> f64 {
    let shift_type = get_shift_type(date);
    
    match shift_type {
        ShiftType::Weekend => {
            (end - start).num_seconds() as f64
        }
        ShiftType::Regular | ShiftType::Afternoon | ShiftType::SaturdayAfternoon => {
            if let Some(window) = get_regular_work_window(date) {
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
                (end - start).num_seconds() as f64
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_regular_day_no_overtime() {
        let date = NaiveDate::from_ymd_opt(2025, 8, 4).unwrap();
        let start = NaiveTime::from_hms_opt(8, 0, 0).unwrap();
        let end = NaiveTime::from_hms_opt(14, 0, 0).unwrap();
        
        let overtime = calculate_overtime_for_day(date, start, end);
        assert_eq!(overtime, 0.0);
    }
    
    #[test]
    fn test_regular_day_with_overtime() {
        let date = NaiveDate::from_ymd_opt(2025, 8, 4).unwrap();
        let start = NaiveTime::from_hms_opt(14, 0, 0).unwrap();
        let end = NaiveTime::from_hms_opt(17, 0, 0).unwrap();
        
        let overtime = calculate_overtime_for_day(date, start, end);
        assert_eq!(overtime, 2.0 * 3600.0);
    }
    
    #[test]
    fn test_weekend_all_overtime() {
        let date = NaiveDate::from_ymd_opt(2025, 8, 10).unwrap();
        let start = NaiveTime::from_hms_opt(10, 0, 0).unwrap();
        let end = NaiveTime::from_hms_opt(14, 0, 0).unwrap();
        
        let overtime = calculate_overtime_for_day(date, start, end);
        assert_eq!(overtime, 4.0 * 3600.0);
    }
    
    #[test]
    fn test_afternoon_shift_before_15() {
        let date = NaiveDate::from_ymd_opt(2025, 7, 28).unwrap();
        let start = NaiveTime::from_hms_opt(10, 0, 0).unwrap();
        let end = NaiveTime::from_hms_opt(14, 0, 0).unwrap();
        
        let overtime = calculate_overtime_for_day(date, start, end);
        assert_eq!(overtime, 4.0 * 3600.0);
    }
}
