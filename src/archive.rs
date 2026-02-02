use chrono::{NaiveDate, Local, Datelike};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use crate::jsonl::ProjectHours;
use crate::schedule::{get_shift_type, ShiftType};

#[derive(Serialize, Deserialize, Default)]
pub struct DailySummaryFile {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub days: HashMap<String, DayEntry>,
    #[serde(default)]
    pub months: HashMap<String, MonthEntry>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct DayEntry {
    pub hours: f64,
    pub formatted: String,
    pub shift: String,
    #[serde(default)]
    pub processed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projects: Option<HashMap<String, ProjectHoursEntry>>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ProjectHoursEntry {
    pub weekday_hours: f64,
    pub weekend_hours: f64,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct MonthEntry {
    pub total_hours: f64,
    pub formatted: String,
}

fn get_summary_path() -> Option<PathBuf> {
    dirs::data_dir()
        .or_else(|| dirs::home_dir().map(|p| p.join(".local/share")))
        .map(|p| p.join("claude-overtime/daily_summary.json"))
}

pub fn load_summary() -> DailySummaryFile {
    let Some(path) = get_summary_path() else {
        return DailySummaryFile::default();
    };
    
    if !path.exists() {
        return DailySummaryFile {
            version: 2,
            days: HashMap::new(),
            months: HashMap::new(),
        };
    }
    
    fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_else(|| DailySummaryFile {
            version: 2,
            days: HashMap::new(),
            months: HashMap::new(),
        })
}

pub fn save_summary(summary: &DailySummaryFile) -> Result<(), String> {
    let Some(path) = get_summary_path() else {
        return Err("Cannot find data dir".to_string());
    };
    
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    
    let tmp_path = path.with_extension("json.tmp");
    let content = serde_json::to_string_pretty(summary).map_err(|e| e.to_string())?;
    
    fs::write(&tmp_path, content).map_err(|e| e.to_string())?;
    fs::rename(&tmp_path, &path).map_err(|e| e.to_string())?;
    
    Ok(())
}

fn format_hm(hours: f64) -> String {
    let total_minutes = (hours * 60.0).round() as i64;
    let h = total_minutes / 60;
    let m = total_minutes.abs() % 60;
    format!("{}:{:02}", h, m)
}

fn shift_name(shift_type: ShiftType) -> &'static str {
    match shift_type {
        ShiftType::Regular => "regular",
        ShiftType::Afternoon => "afternoon",
        ShiftType::Weekend => "weekend",
        ShiftType::SaturdayAfternoon => "saturday_afternoon",
    }
}

pub fn archive_overtime(
    daily_hours: &HashMap<NaiveDate, f64>,
    daily_projects: &HashMap<NaiveDate, HashMap<String, ProjectHours>>,
    debug: bool,
) {
    let today = Local::now().date_naive();
    let mut summary = load_summary();
    summary.version = 2;
    
    let mut updated_count = 0;
    
    for (date, hours) in daily_hours {
        if *date == today {
            continue;
        }
        
        let date_str = date.format("%Y-%m-%d").to_string();
        let existing = summary.days.get(&date_str);
        
        let should_update = match existing {
            None => true,
            Some(entry) => !entry.processed || entry.hours == 0.0,
        };
        
        if !should_update {
            continue;
        }
        
        let shift_type = get_shift_type(*date);
        let projects_entry = daily_projects.get(date).map(|projs| {
            projs
                .iter()
                .map(|(name, hours)| {
                    (
                        name.clone(),
                        ProjectHoursEntry {
                            weekday_hours: hours.weekday_hours,
                            weekend_hours: hours.weekend_hours,
                        },
                    )
                })
                .collect()
        });
        
        let entry = DayEntry {
            hours: *hours,
            formatted: format_hm(*hours),
            shift: shift_name(shift_type).to_string(),
            processed: true,
            projects: projects_entry,
        };
        
        summary.days.insert(date_str.clone(), entry);
        updated_count += 1;
        
        if debug {
            eprintln!("[DEBUG] Archived {}: {}h", date_str, format_hm(*hours));
        }
    }
    
    let mut monthly_totals: HashMap<String, f64> = HashMap::new();
    for (date_str, entry) in &summary.days {
        if let Ok(date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
            let month_key = format!("{}-{:02}", date.year(), date.month());
            *monthly_totals.entry(month_key).or_insert(0.0) += entry.hours;
        }
    }
    
    for (month, total) in monthly_totals {
        summary.months.insert(
            month,
            MonthEntry {
                total_hours: total,
                formatted: format_hm(total),
            },
        );
    }
    
    if updated_count > 0 {
        if let Err(e) = save_summary(&summary) {
            eprintln!("[ERROR] Failed to save daily_summary.json: {}", e);
        } else if debug {
            eprintln!("[DEBUG] Saved {} updated days to daily_summary.json", updated_count);
        }
    }
}

pub fn archive_overtime_full(
    daily_hours: &HashMap<NaiveDate, f64>,
    daily_projects: &HashMap<NaiveDate, HashMap<String, crate::jsonl::ProjectHours>>,
    debug: bool,
) {
    let today = Local::now().date_naive();
    let mut summary = DailySummaryFile {
        version: 2,
        days: HashMap::new(),
        months: HashMap::new(),
    };
    
    for (date, hours) in daily_hours {
        if *date == today {
            continue;
        }
        
        let date_str = date.format("%Y-%m-%d").to_string();
        let shift_type = get_shift_type(*date);
        
        let projects_entry = daily_projects.get(date).map(|projs| {
            projs
                .iter()
                .map(|(name, hours)| {
                    (
                        name.clone(),
                        ProjectHoursEntry {
                            weekday_hours: hours.weekday_hours,
                            weekend_hours: hours.weekend_hours,
                        },
                    )
                })
                .collect()
        });
        
        let entry = DayEntry {
            hours: *hours,
            formatted: format_hm(*hours),
            shift: shift_name(shift_type).to_string(),
            processed: true,
            projects: projects_entry,
        };
        
        summary.days.insert(date_str, entry);
    }
    
    let mut monthly_totals: HashMap<String, f64> = HashMap::new();
    for (date_str, entry) in &summary.days {
        if let Ok(date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
            let month_key = format!("{}-{:02}", date.year(), date.month());
            *monthly_totals.entry(month_key).or_insert(0.0) += entry.hours;
        }
    }
    
    for (month, total) in monthly_totals {
        summary.months.insert(
            month,
            MonthEntry {
                total_hours: total,
                formatted: format_hm(total),
            },
        );
    }
    
    if let Err(e) = save_summary(&summary) {
        eprintln!("[ERROR] Failed to save daily_summary.json: {}", e);
    } else if debug {
        eprintln!("[DEBUG] Full sync: saved {} days to daily_summary.json", summary.days.len());
    }
}
