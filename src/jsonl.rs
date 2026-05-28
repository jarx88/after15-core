use chrono::{Local, NaiveDate, NaiveDateTime};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::config::Config;
use crate::overtime::{calculate_session_overtime, calculate_session_regular};
use crate::report::normalize_project_name;
use crate::schedule::is_weekend;

#[derive(Debug, Clone)]
pub struct Session {
    #[allow(dead_code)]
    pub id: String,
    #[allow(dead_code)]
    pub project: String,
    pub project_counts: HashMap<String, usize>,
    pub start_time: NaiveDateTime,
    pub end_time: NaiveDateTime,
    pub duration_seconds: i64,
}

#[derive(Deserialize)]
struct JsonlEntry {
    timestamp: Option<String>,
    #[serde(rename = "sessionId")]
    #[allow(dead_code)]
    session_id: Option<String>,
    #[serde(rename = "type")]
    entry_type: Option<String>,
    tool_input: Option<ToolInput>,
}

#[derive(Deserialize)]
struct ToolInput {
    #[serde(rename = "filePath")]
    file_path: Option<String>,
    path: Option<String>,
    workdir: Option<String>,
}

#[derive(Deserialize)]
struct DailySummary {
    days: HashMap<String, DayData>,
}

#[derive(Deserialize)]
struct DayData {
    hours: f64,
    #[serde(default)]
    projects: Option<HashMap<String, ProjectHoursJson>>,
}

#[derive(Deserialize)]
struct ProjectHoursJson {
    #[serde(default)]
    weekday_hours: f64,
    #[serde(default)]
    weekend_hours: f64,
    #[serde(default)]
    regular_hours: f64,
}

#[derive(Clone, Default)]
pub struct ProjectHours {
    pub weekday_hours: f64,
    pub weekend_hours: f64,
    pub regular_hours: f64,
}

pub struct DailySummaryData {
    pub hours: HashMap<NaiveDate, f64>,
    pub projects: HashMap<NaiveDate, HashMap<String, ProjectHours>>,
}

pub fn load_daily_summary_full(config: &Config, debug: bool) -> DailySummaryData {
    let mut result = DailySummaryData {
        hours: HashMap::new(),
        projects: HashMap::new(),
    };

    let summary_path = dirs::data_dir()
        .or_else(|| dirs::home_dir().map(|p| p.join(".local/share")))
        .map(|p| p.join("claude-overtime/daily_summary.json"));

    let Some(path) = summary_path else {
        if debug {
            eprintln!("[DEBUG] Cannot find data dir");
        }
        return result;
    };

    if !path.exists() {
        if debug {
            eprintln!("[DEBUG] daily_summary.json not found: {:?}", path);
        }
        return result;
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
        if debug {
            eprintln!("[DEBUG] daily_summary.json is empty");
        }
        return result;
    }

    let summary: DailySummary = serde_json::from_str(&content).unwrap_or_else(|e| {
        eprintln!(
            "[BŁĄD] Nie można sparsować daily_summary.json ({}): {}",
            path.display(),
            e
        );
        std::process::exit(1);
    });

    for (date_str, day_data) in summary.days {
        if let Ok(date) = NaiveDate::parse_from_str(&date_str, "%Y-%m-%d") {
            if let Some(projects) = day_data.projects {
                let mut day_projects: HashMap<String, ProjectHours> = HashMap::new();
                let mut recalculated_hours = 0.0;
                for (proj_name, proj_hours) in projects {
                    if config.is_source_excluded(&proj_name) {
                        continue;
                    }
                    let normalized = normalize_project_name(
                        &proj_name,
                        &config.projects.tracked_path,
                    );
                    if config.projects.excluded_projects.contains(&normalized) {
                        continue;
                    }
                    let ph = ProjectHours {
                        weekday_hours: proj_hours.weekday_hours,
                        weekend_hours: proj_hours.weekend_hours,
                        regular_hours: proj_hours.regular_hours,
                    };
                    recalculated_hours += ph.weekday_hours + ph.weekend_hours;
                    day_projects.insert(proj_name, ph);
                }
                if recalculated_hours > 0.0 {
                    result.hours.insert(date, recalculated_hours);
                }
                if !day_projects.is_empty() {
                    result.projects.insert(date, day_projects);
                }
            } else if day_data.hours > 0.0 {
                result.hours.insert(date, day_data.hours);
            }
        }
    }

    if debug {
        eprintln!(
            "[DEBUG] Loaded {} days from daily_summary ({} with projects)",
            result.hours.len(),
            result.projects.len()
        );
    }

    result
}

pub fn find_recent_jsonl_files(days: i64, debug: bool) -> Vec<PathBuf> {
    let cutoff = Local::now().date_naive() - chrono::Duration::days(days);
    find_jsonl_files(None, Some(cutoff), debug)
}

pub fn find_all_jsonl_files(debug: bool) -> Vec<PathBuf> {
    find_jsonl_files(None, None, debug)
}

fn find_jsonl_files(
    date_filter: Option<NaiveDate>,
    min_date: Option<NaiveDate>,
    debug: bool,
) -> Vec<PathBuf> {
    let mut files = Vec::new();

    let claude_dir = dirs::home_dir().map(|p| p.join(".claude"));

    let Some(claude_path) = claude_dir else {
        return files;
    };

    let search_dirs = [
        claude_path.join("projects"),
        claude_path.join("transcripts"),
    ];

    for search_dir in &search_dirs {
        if !search_dir.exists() {
            continue;
        }

        for entry in WalkDir::new(search_dir)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();

            if !path.is_file() {
                continue;
            }

            if path.extension().map(|e| e != "jsonl").unwrap_or(true) {
                continue;
            }

            let path_str = path.to_string_lossy();
            if path_str.contains("/subagents/") {
                continue;
            }

            if let Ok(metadata) = fs::metadata(path) {
                if let Ok(modified) = metadata.modified() {
                    let modified_date = chrono::DateTime::<Local>::from(modified).date_naive();

                    if let Some(filter_date) = date_filter {
                        if modified_date != filter_date {
                            continue;
                        }
                    }

                    if let Some(cutoff) = min_date {
                        if modified_date < cutoff {
                            continue;
                        }
                    }
                }
            }

            files.push(path.to_path_buf());
            if debug {
                eprintln!("[DEBUG] Found JSONL: {:?}", path);
            }
        }
    }

    files
}

pub struct TodayData {
    pub hours: HashMap<NaiveDate, f64>,
    pub projects: HashMap<NaiveDate, HashMap<String, ProjectHours>>,
}

pub fn load_recent_overtime(days: i64, config: &Config, debug: bool) -> TodayData {
    load_overtime_from_files(find_recent_jsonl_files(days, debug), None, config, debug)
}

pub fn load_today_overtime(config: &Config, debug: bool) -> TodayData {
    let today = Local::now().date_naive();
    let files = find_jsonl_files(None, Some(today), debug);
    load_overtime_from_files(files, Some(today), config, debug)
}

pub fn load_all_overtime(config: &Config, debug: bool) -> TodayData {
    load_overtime_from_files(find_all_jsonl_files(debug), None, config, debug)
}

pub fn load_sessions_for_date(date: NaiveDate, config: &Config, debug: bool) -> Vec<Session> {
    use chrono_tz::Europe::Warsaw;

    let files = find_all_jsonl_files(debug);

    let mut all_records: Vec<TimestampRecord> = Vec::new();

    for file_path in &files {
        let records = collect_timestamps_from_file(file_path);
        all_records.extend(records);
    }

    if all_records.is_empty() {
        return Vec::new();
    }

    all_records.retain(|r| config.is_tracked_source(&r.project) && !config.is_source_excluded(&r.project));
    all_records.sort_by_key(|r| r.timestamp);

    let sessions = build_sessions_from_records(&all_records, false);

    sessions
        .into_iter()
        .filter(|s| {
            let start_local = s.start_time.and_utc().with_timezone(&Warsaw).naive_local();
            let end_local = s.end_time.and_utc().with_timezone(&Warsaw).naive_local();
            let start_date = start_local.date();
            let end_date = end_local.date();
            date >= start_date && date <= end_date
        })
        .collect()
}

const SESSION_GAP_SECONDS: i64 = 30 * 60;
const MIN_SESSION_SECONDS: i64 = 5 * 60;

#[derive(Debug, Clone)]
struct TimestampRecord {
    timestamp: NaiveDateTime,
    project: String,
}

fn load_overtime_from_files(
    files: Vec<PathBuf>,
    date_filter: Option<NaiveDate>,
    config: &Config,
    debug: bool,
) -> TodayData {
    let mut result = TodayData {
        hours: HashMap::new(),
        projects: HashMap::new(),
    };

    if debug {
        eprintln!(
            "[DEBUG] Processing {} JSONL files with GLOBAL gap detection",
            files.len()
        );
    }

    let mut all_records: Vec<TimestampRecord> = Vec::new();

    for file_path in &files {
        let records = collect_timestamps_from_file(file_path);
        all_records.extend(records);
    }

    if all_records.is_empty() {
        return result;
    }

    all_records.retain(|r| config.is_tracked_source(&r.project) && !config.is_source_excluded(&r.project));
    all_records.sort_by_key(|r| r.timestamp);

    if debug {
        eprintln!(
            "[DEBUG] Collected {} total records from all files (after source filtering)",
            all_records.len()
        );
    }

    let sessions = build_sessions_from_records(&all_records, debug);

    if debug {
        eprintln!(
            "[DEBUG] Created {} sessions from global gap detection",
            sessions.len()
        );
    }

    for session in sessions {
        let filter = date_filter.unwrap_or(session.start_time.date());
        let overtime = calculate_session_overtime(&session, filter, config, debug);
        let regular = calculate_session_regular(&session, config);

        let real_projects: HashMap<String, usize> = session
            .project_counts
            .iter()
            .filter(|(name, _)| *name != "transcripts")
            .map(|(k, v)| (k.clone(), *v))
            .collect();

        let total_records: usize = real_projects.values().sum();

        for (date, hours) in overtime {
            let dominated = date_filter.map(|f| date != f).unwrap_or(false);
            if dominated || hours <= 0.0 {
                continue;
            }

            *result.hours.entry(date).or_insert(0.0) += hours;

            let day_projects = result.projects.entry(date).or_default();

            if total_records == 0 {
                let proj_entry = day_projects.entry("unknown".to_string()).or_default();
                if is_weekend(date) {
                    proj_entry.weekend_hours += hours;
                } else {
                    proj_entry.weekday_hours += hours;
                }
            } else {
                for (proj_name, &count) in &real_projects {
                    let fraction = count as f64 / total_records as f64;
                    let proj_hours = hours * fraction;

                    let proj_entry = day_projects.entry(proj_name.clone()).or_default();

                    if is_weekend(date) {
                        proj_entry.weekend_hours += proj_hours;
                    } else {
                        proj_entry.weekday_hours += proj_hours;
                    }
                }
            }
        }

        for (date, reg_hours) in regular {
            let dominated = date_filter.map(|f| date != f).unwrap_or(false);
            if dominated || reg_hours <= 0.0 {
                continue;
            }

            let day_projects = result.projects.entry(date).or_default();

            if total_records == 0 {
                let proj_entry = day_projects.entry("unknown".to_string()).or_default();
                proj_entry.regular_hours += reg_hours;
            } else {
                for (proj_name, &count) in &real_projects {
                    let fraction = count as f64 / total_records as f64;
                    let proj_hours = reg_hours * fraction;
                    let proj_entry = day_projects.entry(proj_name.clone()).or_default();
                    proj_entry.regular_hours += proj_hours;
                }
            }
        }
    }

    result
}

fn collect_timestamps_from_file(path: &Path) -> Vec<TimestampRecord> {
    let mut records = Vec::new();

    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return records,
    };
    let reader = BufReader::new(file);
    let default_project = extract_project_name(path);
    let is_transcript = default_project == "transcripts";

    for line in reader.lines().flatten() {
        if let Ok(entry) = serde_json::from_str::<JsonlEntry>(&line) {
            let entry_type = entry.entry_type.as_deref().unwrap_or("");
            match entry_type {
                "system" | "file-history-snapshot" | "queue-operation"
                | "custom-title" | "agent-name" | "last-prompt" | "pr-link" => continue,
                _ => {}
            }

            if let Some(ref ts_str) = entry.timestamp {
                if let Some(ts) = parse_timestamp(ts_str) {
                    let project = if is_transcript {
                        extract_project_from_tool_input(&entry)
                            .unwrap_or_else(|| default_project.clone())
                    } else {
                        default_project.clone()
                    };

                    records.push(TimestampRecord {
                        timestamp: ts,
                        project,
                    });
                }
            }
        }
    }

    records
}

const PROGRAMOWANIE_DIR: &str = "Programowanie";
const WORKTREE_PATH_MARKER: &str = "/.t3/worktrees/";
const WORKTREE_DIR_PREFIX: &str = "--t3-worktrees-";
const WORKTREE_DIR_SUFFIX: &str = "-t3code-";

fn extract_worktree_project_name(encoded: &str) -> Option<&str> {
    let after = encoded.split_once(WORKTREE_DIR_PREFIX)?.1;
    after
        .split_once(WORKTREE_DIR_SUFFIX)
        .map(|(name, _)| name)
        .filter(|s| !s.is_empty())
}

fn extract_worktree_project_from_filepath(file_path: &str) -> Option<&str> {
    let after = file_path.split_once(WORKTREE_PATH_MARKER)?.1;
    after.split('/').next().filter(|s| !s.is_empty())
}

fn worktree_project_exists_in_programowanie(project: &str) -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    let prog_dir = home.join(PROGRAMOWANIE_DIR);
    if prog_dir.join(project).is_dir() {
        return true;
    }
    let with_underscore = project.replace('-', "_");
    prog_dir.join(with_underscore).is_dir()
}

fn extract_project_from_tool_input(entry: &JsonlEntry) -> Option<String> {
    let tool_input = entry.tool_input.as_ref()?;

    let file_path = tool_input
        .file_path
        .as_ref()
        .or(tool_input.path.as_ref())
        .or(tool_input.workdir.as_ref())?;

    let project_name = if let Some((_, after)) = file_path.split_once("/Programowanie/") {
        after.split('/').next().filter(|s| !s.is_empty())?
    } else if let Some(name) = extract_worktree_project_from_filepath(file_path) {
        if !worktree_project_exists_in_programowanie(name) {
            return None;
        }
        name
    } else {
        return None;
    };

    let normalized = project_name.replace('_', "-");
    Some(format!("-home-jarx-Programowanie-{}", normalized))
}

fn build_sessions_from_records(records: &[TimestampRecord], debug: bool) -> Vec<Session> {
    let mut sessions = Vec::new();

    if records.is_empty() {
        return sessions;
    }

    let mut session_start = records[0].timestamp;
    let mut session_end = records[0].timestamp;
    let mut session_projects: HashMap<String, usize> = HashMap::new();
    session_projects.insert(records[0].project.clone(), 1);
    let mut session_count = 0;

    for i in 1..records.len() {
        let gap = (records[i].timestamp - session_end).num_seconds();

        if gap > SESSION_GAP_SECONDS {
            let duration = (session_end - session_start).num_seconds();
            if duration >= MIN_SESSION_SECONDS {
                let dominant_project = session_projects
                    .iter()
                    .max_by_key(|(_, count)| *count)
                    .map(|(proj, _)| proj.clone())
                    .unwrap_or_else(|| "unknown".to_string());

                sessions.push(Session {
                    id: format!("global-{}", session_count),
                    project: dominant_project,
                    project_counts: session_projects.clone(),
                    start_time: session_start,
                    end_time: session_end,
                    duration_seconds: duration,
                });
                session_count += 1;
            }
            session_start = records[i].timestamp;
            session_projects.clear();
        }
        session_end = records[i].timestamp;
        *session_projects
            .entry(records[i].project.clone())
            .or_insert(0) += 1;
    }

    let duration = (session_end - session_start).num_seconds();
    if duration >= MIN_SESSION_SECONDS {
        let dominant_project = session_projects
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(proj, _)| proj.clone())
            .unwrap_or_else(|| "unknown".to_string());

        sessions.push(Session {
            id: format!("global-{}", session_count),
            project: dominant_project,
            project_counts: session_projects.clone(),
            start_time: session_start,
            end_time: session_end,
            duration_seconds: duration,
        });
    }

    if debug && !sessions.is_empty() {
        let total_duration: i64 = sessions.iter().map(|s| s.duration_seconds).sum();
        eprintln!(
            "[DEBUG] Total session time: {}s ({:.2}h)",
            total_duration,
            total_duration as f64 / 3600.0
        );
    }

    sessions
}

fn parse_timestamp(ts: &str) -> Option<NaiveDateTime> {
    let cleaned = ts.trim_end_matches('Z').replace('T', " ");
    let without_ms = cleaned.split('.').next()?;
    NaiveDateTime::parse_from_str(without_ms, "%Y-%m-%d %H:%M:%S").ok()
}

fn extract_project_name(path: &Path) -> String {
    let path_str = path.to_string_lossy();

    if path_str.contains("transcripts") {
        return "transcripts".to_string();
    }

    if let Some(parent) = path.parent() {
        let parent_name = parent
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        if !parent_name.is_empty() && parent_name != "projects" {
            if let Some(project) = extract_worktree_project_name(&parent_name) {
                if worktree_project_exists_in_programowanie(project) {
                    let normalized = project.replace('_', "-");
                    return format!("-home-jarx-Programowanie-{}", normalized);
                }
            }
            return parent_name;
        }
    }

    "unknown".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_timestamp() {
        let ts = "2026-01-28T06:58:16.234Z";
        let parsed = parse_timestamp(ts).unwrap();
        assert_eq!(parsed.to_string(), "2026-01-28 06:58:16");
    }

    #[test]
    fn test_extract_project_name() {
        let path = Path::new(
            "/home/jarx/.claude/projects/-home-jarx-Programowanie-farmaster2/session.jsonl",
        );
        let name = extract_project_name(path);
        assert_eq!(name, "-home-jarx-Programowanie-farmaster2");
    }

    #[test]
    fn test_extract_worktree_project_name_helper() {
        assert_eq!(
            extract_worktree_project_name("-home-jarek--t3-worktrees-farmaster2-t3code-0b257d0d"),
            Some("farmaster2")
        );
        assert_eq!(
            extract_worktree_project_name(
                "-home-jarek--t3-worktrees-Eksozycja-apteki-t3code-09aba15b"
            ),
            Some("Eksozycja-apteki")
        );
        assert_eq!(
            extract_worktree_project_name("-home-jarek-Programowanie-farmaster2"),
            None
        );
        assert_eq!(extract_worktree_project_name("random-string"), None);
        assert_eq!(
            extract_worktree_project_name("-home-jarek--t3-worktrees--t3code-XXXX"),
            None
        );
    }

    #[test]
    fn test_extract_worktree_project_from_filepath_helper() {
        assert_eq!(
            extract_worktree_project_from_filepath(
                "/home/jarek/.t3/worktrees/farmaster2/t3code-0b257d0d/backend/file.rs"
            ),
            Some("farmaster2")
        );
        assert_eq!(
            extract_worktree_project_from_filepath(
                "/home/jarek/.t3/worktrees/Eksozycja-apteki/t3code-09aba15b/src/main.rs"
            ),
            Some("Eksozycja-apteki")
        );
        assert_eq!(
            extract_worktree_project_from_filepath("/home/jarek/Programowanie/farmaster2/file.rs"),
            None
        );
        assert_eq!(
            extract_worktree_project_from_filepath("/home/jarek/.t3/worktrees//file.rs"),
            None
        );
    }
}
