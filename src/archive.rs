use chrono::{Datelike, Local, NaiveDate};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::path::PathBuf;

use crate::jsonl::ProjectHours;
use crate::schedule::{get_shift_type, ShiftType};

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

#[derive(Serialize, Deserialize, Default)]
pub struct DailySummaryFile {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub days: BTreeMap<String, DayEntry>,
    #[serde(default)]
    pub months: BTreeMap<String, MonthEntry>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct DayEntry {
    pub hours: f64,
    pub formatted: String,
    pub shift: String,
    #[serde(default)]
    pub processed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projects: Option<BTreeMap<String, ProjectHoursEntry>>,
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

pub fn lock_archive() -> Option<File> {
    let lock_path = get_summary_path()?.with_extension("json.lock");
    if let Some(parent) = lock_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&lock_path)
        .ok()?;
    file.lock_exclusive().ok()?;
    Some(file)
}

pub fn load_summary() -> DailySummaryFile {
    let Some(path) = get_summary_path() else {
        return DailySummaryFile::default();
    };

    if !path.exists() {
        return DailySummaryFile {
            version: 2,
            days: BTreeMap::new(),
            months: BTreeMap::new(),
        };
    }

    let content = fs::read_to_string(&path).unwrap_or_else(|e| {
        eprintln!(
            "[BŁĄD] Nie można odczytać daily_summary.json ({}): {}",
            path.display(),
            e
        );
        std::process::exit(1);
    });

    if content.trim().is_empty() {
        return DailySummaryFile {
            version: 2,
            days: BTreeMap::new(),
            months: BTreeMap::new(),
        };
    }

    serde_json::from_str(&content).unwrap_or_else(|e| {
        eprintln!(
            "[BŁĄD] Nie można sparsować daily_summary.json ({}): {}",
            path.display(),
            e
        );
        std::process::exit(1);
    })
}

pub fn force_backup() -> Result<PathBuf, String> {
    let Some(path) = get_summary_path() else {
        return Err("Cannot find data dir".to_string());
    };
    if !path.exists() {
        return Err("Plik daily_summary.json nie istnieje".to_string());
    }
    let bak_dir = path.parent().unwrap().join("backups");
    fs::create_dir_all(&bak_dir)
        .map_err(|e| format!("Nie można utworzyć katalogu backupów: {}", e))?;

    let now = Local::now().format("%Y-%m-%d_%H%M%S").to_string();
    let bak_path = bak_dir.join(format!("daily_summary.{}.json", now));
    fs::copy(&path, &bak_path)
        .map_err(|e| format!("Nie można skopiować do {}: {}", bak_path.display(), e))?;

    backup_to_config(&path);

    Ok(bak_path)
}

const MAX_OVERTIME_PER_DAY: f64 = 24.0;

pub fn save_summary(summary: &DailySummaryFile) -> Result<(), String> {
    for (date_str, entry) in &summary.days {
        if entry.hours > MAX_OVERTIME_PER_DAY {
            return Err(format!(
                "Odmowa zapisu: {} ma {:.2}h nadgodzin (max {}h/dzień)",
                date_str, entry.hours, MAX_OVERTIME_PER_DAY
            ));
        }
    }

    let Some(path) = get_summary_path() else {
        return Err("Cannot find data dir".to_string());
    };

    if path.exists() {
        let content = fs::read_to_string(&path)
            .map_err(|e| format!("Nie można odczytać {}: {}", path.display(), e))?;
        if !content.trim().is_empty() {
            let existing: DailySummaryFile = serde_json::from_str(&content)
                .map_err(|e| format!("Nie można sparsować {}: {}", path.display(), e))?;
            if summary.days.len() < existing.days.len() {
                return Err(format!(
                    "Odmowa zapisu: nowe archiwum ma mniej dni ({} < {})",
                    summary.days.len(),
                    existing.days.len()
                ));
            }
        }
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    if path.exists() {
        let now = Local::now().format("%Y-%m-%d_%H%M%S").to_string();
        let bak_dir = path.parent().unwrap().join("backups");
        fs::create_dir_all(&bak_dir).map_err(|e| {
            format!(
                "Nie można utworzyć katalogu backupów {}: {}",
                bak_dir.display(),
                e
            )
        })?;
        let bak_path = bak_dir.join(format!("daily_summary.{}.json", now));
        fs::copy(&path, &bak_path).map_err(|e| {
            format!(
                "Nie można utworzyć kopii zapasowej {}: {}",
                bak_path.display(),
                e
            )
        })?;
        cleanup_old_backups(&bak_dir, 14);

        backup_to_config(&path);
    }

    let tmp_path = path.with_extension("json.tmp");
    let content = serde_json::to_string_pretty(summary).map_err(|e| e.to_string())?;

    fs::write(&tmp_path, &content).map_err(|e| e.to_string())?;
    fs::rename(&tmp_path, &path).map_err(|e| e.to_string())?;

    Ok(())
}

fn backup_to_config(source: &std::path::Path) {
    let config_backup = dirs::config_dir()
        .or_else(|| dirs::home_dir().map(|p| p.join(".config")))
        .map(|p| p.join("after15/daily_summary.backup.json"));

    if let Some(backup_path) = config_backup {
        if let Some(parent) = backup_path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                eprintln!(
                    "[WARN] Nie można utworzyć katalogu config backup {}: {}",
                    parent.display(),
                    e
                );
                return;
            }
        }
        if let Err(e) = fs::copy(source, &backup_path) {
            eprintln!(
                "[WARN] Backup do {} nie powiódł się: {}",
                backup_path.display(),
                e
            );
        }
    }
}

fn cleanup_old_backups(dir: &std::path::Path, keep: usize) {
    let mut backups: Vec<_> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("daily_summary.")
                && e.file_name().to_string_lossy().ends_with(".json")
        })
        .collect();

    if backups.len() <= keep {
        return;
    }

    backups.sort_by_key(|e| e.file_name().to_string_lossy().to_string());
    let to_remove = backups.len() - keep;
    for entry in backups.into_iter().take(to_remove) {
        if let Err(e) = fs::remove_file(entry.path()) {
            eprintln!(
                "[WARN] Nie można usunąć starego backupu {}: {}",
                entry.path().display(),
                e
            );
        }
    }
}

pub fn format_hm(hours: f64) -> String {
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
    let _lock = lock_archive();
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
                            weekday_hours: round2(hours.weekday_hours),
                            weekend_hours: round2(hours.weekend_hours),
                        },
                    )
                })
                .collect()
        });

        let entry = DayEntry {
            hours: round2(*hours),
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

    let mut monthly_totals: BTreeMap<String, f64> = BTreeMap::new();
    for (date_str, entry) in &summary.days {
        if let Ok(date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
            let month_key = format!("{}-{:02}", date.year(), date.month());
            *monthly_totals.entry(month_key).or_insert(0.0) += entry.hours;
        }
    }

    summary.months.clear();
    for (month, total) in monthly_totals {
        summary.months.insert(
            month,
            MonthEntry {
                total_hours: round2(total),
                formatted: format_hm(total),
            },
        );
    }

    if updated_count > 0 {
        if let Err(e) = save_summary(&summary) {
            eprintln!("[ERROR] Failed to save daily_summary.json: {}", e);
        } else if debug {
            eprintln!(
                "[DEBUG] Saved {} updated days to daily_summary.json",
                updated_count
            );
        }
    }
}
