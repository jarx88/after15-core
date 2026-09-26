use chrono::{Local, NaiveDate, NaiveDateTime};
use serde::Deserialize;
use std::borrow::Borrow;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::SystemTime;
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
    pub has_claude: bool,
    pub has_codex: bool,
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
    // Lekki typ zamiast serde_json::Value: serde przeskakuje resztę payloadu
    // bez budowania drzewa. Przy 5 GB JSONL to różnica rzędu wielkości.
    payload: Option<CodexPayload>,
}

#[derive(Deserialize)]
struct CodexPayload {
    cwd: Option<String>,
}

#[derive(Deserialize)]
struct ToolInput {
    #[serde(rename = "filePath")]
    file_path: Option<String>,
    path: Option<String>,
    workdir: Option<String>,
}

/// Treść wiadomości — parsowana osobno, raz na plik (patrz `is_t3_metadata_prompt`),
/// bo `content` bywa wielomegabajtową tablicą bloków.
#[derive(Deserialize)]
struct JsonlMessageEntry {
    message: Option<JsonlMessage>,
}

#[derive(Deserialize)]
struct JsonlMessage {
    content: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct DailySummary {
    days: HashMap<String, DayData>,
}

#[derive(Deserialize)]
struct DayData {
    hours: f64,
    #[serde(default)]
    manual_override: bool,
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

/// Wybiera autorytatywną liczbę godzin dnia: przy manual_override bierze stored
/// `hours` (ręczna korekta z TUI), inaczej sumę przeliczoną z projektów.
fn resolve_day_hours(manual_override: bool, stored_hours: f64, recalculated_hours: f64) -> f64 {
    if manual_override {
        stored_hours
    } else {
        recalculated_hours
    }
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
                // Suma wszystkich projektów dnia, przed filtrami — mianownik
                // skalowania musi być identyczny jak w summary_projects() w web.rs.
                let all_hours: f64 = projects
                    .values()
                    .map(|h| h.weekday_hours + h.weekend_hours)
                    .sum();
                for (proj_name, proj_hours) in projects {
                    if config.is_source_excluded(&proj_name) {
                        continue;
                    }
                    let normalized =
                        normalize_project_name(&proj_name, &config.projects.tracked_path);
                    if config.projects.excluded_projects.contains(&normalized) {
                        continue;
                    }
                    recalculated_hours += proj_hours.weekday_hours + proj_hours.weekend_hours;
                    let canonical = canonical_project_key(&proj_name);
                    let entry = day_projects.entry(canonical).or_default();
                    entry.weekday_hours += proj_hours.weekday_hours;
                    entry.weekend_hours += proj_hours.weekend_hours;
                    entry.regular_hours += proj_hours.regular_hours;
                }
                let day_hours =
                    resolve_day_hours(day_data.manual_override, day_data.hours, recalculated_hours);
                // Manual day total overrides the computed one — scale project hours
                // proportionally so per-project sums match the correction (same
                // scaling as summary_projects() in web.rs).
                let scale = if day_data.manual_override && all_hours > 0.0 {
                    day_hours / all_hours
                } else {
                    1.0
                };
                if scale != 1.0 {
                    for entry in day_projects.values_mut() {
                        entry.weekday_hours *= scale;
                        entry.weekend_hours *= scale;
                    }
                }
                if day_hours > 0.0 {
                    result.hours.insert(date, day_hours);
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
    find_jsonl_files(Some(cutoff), None, debug)
}

pub fn find_all_jsonl_files(debug: bool) -> Vec<PathBuf> {
    find_jsonl_files(None, None, debug)
}

/// Pliki, które mogą zawierać rekordy z `date`: zmodyfikowane najwcześniej dzień
/// wcześniej i utworzone najpóźniej 2 dni później — plik powstały po tej dacie
/// nie mógł zapisać jej rekordów. Bez tego filtru dzień sprzed miesiąca ciągnie
/// wszystkie sesje z ostatniego miesiąca (~1 GB) zamiast kilkudziesięciu MB.
fn files_touching_date(date: NaiveDate, debug: bool) -> Vec<PathBuf> {
    find_jsonl_files(
        Some(date - chrono::Duration::days(1)),
        Some(date + chrono::Duration::days(2)),
        debug,
    )
}

/// Granice filtra `files_touching_date`: (najwcześniejsza modyfikacja, granica utworzenia).
pub(crate) fn touching_date_bounds(date: NaiveDate) -> (SystemTime, SystemTime) {
    (
        local_midnight(date - chrono::Duration::days(1)),
        local_midnight(date + chrono::Duration::days(3)),
    )
}

/// Filtr dat na metadanych pliku. Brak czasu (nieobsługiwany przez FS) = brak filtra.
pub(crate) fn passes_date_filter(
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    min_modified: Option<SystemTime>,
    max_created: Option<SystemTime>,
) -> bool {
    if let (Some(cutoff), Some(modified)) = (min_modified, modified)
        && modified < cutoff
    {
        return false;
    }
    // created() nieobsługiwane na starszych FS → brak filtru (jak dotąd)
    if let (Some(limit), Some(created)) = (max_created, created)
        && created >= limit
    {
        return false;
    }
    true
}

/// Lokalna północ danego dnia jako `SystemTime` — granice liczymy raz, żeby nie
/// konwertować stref czasowych dla każdego z ~14 tys. plików w drzewie.
fn local_midnight(date: NaiveDate) -> std::time::SystemTime {
    let seconds = date
        .and_hms_opt(0, 0, 0)
        .and_then(|dt| dt.and_local_timezone(Local).earliest())
        .map(|dt| dt.timestamp())
        .unwrap_or(0);
    let offset = std::time::Duration::from_secs(seconds.unsigned_abs());
    if seconds >= 0 {
        std::time::UNIX_EPOCH + offset
    } else {
        std::time::UNIX_EPOCH - offset
    }
}

const SEARCH_DIRS: [&str; 4] = [
    ".claude/projects",
    ".claude/transcripts",
    ".codex/sessions",
    ".codex/archived_sessions",
];

/// Przechodzi drzewa sesji i woła `visit` dla każdego liczonego pliku JSONL.
fn walk_jsonl_files(mut visit: impl FnMut(walkdir::DirEntry)) {
    let Some(home) = dirs::home_dir() else {
        return;
    };

    for search_dir in SEARCH_DIRS.iter().map(|dir| home.join(dir)) {
        if !search_dir.exists() {
            continue;
        }

        for entry in WalkDir::new(&search_dir)
            .follow_links(false)
            .into_iter()
            // Pliki subagentów i tak odpadają niżej — nie schodzimy do ich
            // katalogów (to ~40% wszystkich plików w drzewie).
            .filter_entry(|e| !(e.file_type().is_dir() && e.file_name() == "subagents"))
            .filter_map(|e| e.ok())
        {
            // file_type() pochodzi z readdir — bez dodatkowego stat na wpis
            if !entry.file_type().is_file() || !is_counted_jsonl(entry.path()) {
                continue;
            }

            visit(entry);
        }
    }
}

fn is_counted_jsonl(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "jsonl")
        && !path.to_string_lossy().contains("/subagents/")
}

/// Równoległy `walk_jsonl_files` z metadanymi (lstat, jak `DirEntry::metadata`
/// z walkdir). Zwraca te same pliki w tej samej kolejności: każdy wpis dostaje
/// klucz z pozycji w readdir kolejnych katalogów, a sortowanie po kluczu
/// odtwarza preorder WalkDir. Pliki bez metadanych pomija.
pub(crate) fn scan_jsonl_files() -> Vec<(PathBuf, fs::Metadata)> {
    struct Queue {
        dirs: Vec<(PathBuf, Vec<u32>)>,
        active: usize,
    }

    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let queue = Mutex::new(Queue {
        dirs: (0u32..)
            .zip(SEARCH_DIRS)
            .map(|(i, dir)| (home.join(dir), vec![i]))
            .collect(),
        active: 0,
    });
    let wake = Condvar::new();
    let found = Mutex::new(Vec::new());
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let (dir, key) = {
                        let mut queue = queue.lock().unwrap();
                        loop {
                            if let Some(task) = queue.dirs.pop() {
                                queue.active += 1;
                                break task;
                            }
                            if queue.active == 0 {
                                wake.notify_all();
                                return;
                            }
                            queue = wake.wait(queue).unwrap();
                        }
                    };

                    let mut subdirs = Vec::new();
                    let mut files = Vec::new();
                    if let Ok(entries) = fs::read_dir(&dir) {
                        for (i, entry) in (0u32..).zip(entries) {
                            let Ok(entry) = entry else { continue };
                            let Ok(file_type) = entry.file_type() else {
                                continue;
                            };
                            let mut entry_key = key.clone();
                            entry_key.push(i);
                            if file_type.is_dir() {
                                if entry.file_name() != "subagents" {
                                    subdirs.push((entry.path(), entry_key));
                                }
                            } else if file_type.is_file() {
                                let path = entry.path();
                                if is_counted_jsonl(&path)
                                    && let Ok(metadata) = entry.metadata()
                                {
                                    files.push((entry_key, path, metadata));
                                }
                            }
                        }
                    }

                    found.lock().unwrap().extend(files);
                    let mut queue = queue.lock().unwrap();
                    queue.dirs.extend(subdirs);
                    queue.active -= 1;
                    drop(queue);
                    wake.notify_all();
                }
            });
        }
    });

    let mut found = found.into_inner().unwrap();
    found.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    found
        .into_iter()
        .map(|(_, path, metadata)| (path, metadata))
        .collect()
}

fn find_jsonl_files(
    min_date: Option<NaiveDate>,
    max_birth: Option<NaiveDate>,
    debug: bool,
) -> Vec<PathBuf> {
    let mut files = Vec::new();

    let min_modified = min_date.map(local_midnight);
    let max_created = max_birth.map(|date| local_midnight(date + chrono::Duration::days(1)));

    walk_jsonl_files(|entry| {
        if min_modified.is_some() || max_created.is_some() {
            let Ok(metadata) = entry.metadata() else {
                return;
            };
            if !passes_date_filter(
                metadata.modified().ok(),
                metadata.created().ok(),
                min_modified,
                max_created,
            ) {
                return;
            }
        }

        if debug {
            eprintln!("[DEBUG] Found JSONL: {:?}", entry.path());
        }
        files.push(entry.into_path());
    });

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
    let files = find_jsonl_files(Some(today), None, debug);
    load_overtime_from_files(files, Some(today), config, debug)
}

pub fn load_all_overtime(config: &Config, debug: bool) -> TodayData {
    load_overtime_from_files(find_all_jsonl_files(debug), None, config, debug)
}

pub fn load_sessions_for_date(date: NaiveDate, config: &Config, debug: bool) -> Vec<Session> {
    let files = files_touching_date(date, debug);

    let mut all_records = collect_timestamps_parallel(&files);
    all_records.retain(|r| is_counted_source(config, &r.project));
    sessions_touching_date(date, all_records)
}

/// Rekord z projektu, który wchodzi do nadgodzin (śledzona ścieżka, nie wykluczony).
pub(crate) fn is_counted_source(config: &Config, project: &str) -> bool {
    config.is_tracked_source(project) && !config.is_source_excluded(project)
}

/// Sesje dotykające `date` z już odfiltrowanych rekordów plików `files_touching_date(date)`.
pub(crate) fn sessions_touching_date<R: Borrow<TimestampRecord>>(
    date: NaiveDate,
    mut records: Vec<R>,
) -> Vec<Session> {
    use chrono_tz::Europe::Warsaw;

    if records.is_empty() {
        return Vec::new();
    }

    records.sort_by_key(|r| r.borrow().timestamp);

    let sessions = build_sessions_from_records(&records, false);

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
pub(crate) struct TimestampRecord {
    pub(crate) timestamp: NaiveDateTime,
    // Arc: rekordy jednego pliku dzielą nazwę projektu, a cache panelu web
    // trzyma je w pamięci między żądaniami.
    pub(crate) project: Arc<str>,
    pub(crate) is_codex: bool,
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

    let mut all_records = collect_timestamps_parallel(&files);

    if all_records.is_empty() {
        return result;
    }

    all_records.retain(|r| is_counted_source(config, &r.project));
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

    let exclusions = crate::archive::session_exclusions();
    for session in &sessions {
        // Keep source days present even when all their sessions were excluded.
        let mut date = session
            .start_time
            .and_utc()
            .with_timezone(&chrono_tz::Europe::Warsaw)
            .date_naive();
        let end_date = session
            .end_time
            .and_utc()
            .with_timezone(&chrono_tz::Europe::Warsaw)
            .date_naive();
        while date <= end_date {
            if date_filter.is_none_or(|filter| filter == date)
                && exclusions
                    .iter()
                    .any(|(excluded_date, _)| *excluded_date == date)
            {
                result.hours.entry(date).or_insert(0.0);
                result.projects.entry(date).or_default();
            }
            date += chrono::Duration::days(1);
        }
    }
    for session in sessions
        .iter()
        .flat_map(|session| crate::overtime::exclude_session_ranges(session, &exclusions))
    {
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

/// Parsuje pliki równolegle; wynik w kolejności `files`. Wątki biorą kolejne
/// pliki z kolejki, więc jeden wielki plik nie blokuje reszty porcji.
fn collect_timestamps_parallel(files: &[PathBuf]) -> Vec<TimestampRecord> {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(files.len().max(1));
    let next = AtomicUsize::new(0);
    let mut parsed: Vec<(usize, Vec<TimestampRecord>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(path) = files.get(index) else {
                            break;
                        };
                        done.push((index, collect_timestamps_from_file(path)));
                    }
                    done
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap_or_default())
            .collect()
    });
    parsed.sort_unstable_by_key(|(index, _)| *index);
    parsed.into_iter().flat_map(|(_, records)| records).collect()
}

pub(crate) fn collect_timestamps_from_file(path: &Path) -> Vec<TimestampRecord> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let mut parser = FileParser::new(path);
    let records: Vec<_> = BufReader::new(file)
        .lines()
        .flatten()
        .filter_map(|line| parser.parse_line(&line))
        .collect();

    if parser.is_t3_metadata_session {
        Vec::new()
    } else {
        records
    }
}

/// Stan parsowania jednego pliku JSONL między liniami. Pliki sesji są tylko
/// dopisywane, więc cache panelu web może doczytać nowe linie od zapamiętanego
/// stanu zamiast parsować cały plik od nowa.
#[derive(Clone)]
pub(crate) struct FileParser {
    pub(crate) default_project: Arc<str>,
    pub(crate) is_transcript: bool,
    pub(crate) is_codex: bool,
    pub(crate) codex_project: Option<Arc<str>>,
    pub(crate) first_user_seen: bool,
    /// Pierwszy prompt to generator tytułu/gałęzi t3 — cały plik się nie liczy.
    pub(crate) is_t3_metadata_session: bool,
}

impl FileParser {
    pub(crate) fn new(path: &Path) -> Self {
        let default_project: Arc<str> = extract_project_name(path).into();
        FileParser {
            is_transcript: &*default_project == "transcripts",
            is_codex: path.to_string_lossy().contains("/.codex/"),
            default_project,
            codex_project: None,
            first_user_seen: false,
            is_t3_metadata_session: false,
        }
    }

    pub(crate) fn parse_line(&mut self, line: &str) -> Option<TimestampRecord> {
        let entry = serde_json::from_str::<JsonlEntry>(line).ok()?;
        if self.is_codex
            && let Some(cwd) = entry
                .payload
                .as_ref()
                .and_then(|payload| payload.cwd.as_deref())
        {
            self.codex_project = extract_project_from_path(cwd).map(Arc::from);
        }
        let entry_type = entry.entry_type.as_deref().unwrap_or("");
        if !self.is_codex && entry_type == "user" && !self.first_user_seen {
            self.first_user_seen = true;
            self.is_t3_metadata_session = is_t3_metadata_prompt(line);
        }
        match entry_type {
            "system"
            | "file-history-snapshot"
            | "queue-operation"
            | "custom-title"
            | "agent-name"
            | "last-prompt"
            | "pr-link" => return None,
            _ => {}
        }

        let timestamp = parse_timestamp(entry.timestamp.as_deref()?)?;
        let project = if self.is_codex {
            self.codex_project.clone()?
        } else if self.is_transcript {
            extract_project_from_tool_input(&entry)
                .map(Arc::from)
                .unwrap_or_else(|| self.default_project.clone())
        } else {
            self.default_project.clone()
        };

        Some(TimestampRecord {
            timestamp,
            project,
            is_codex: self.is_codex,
        })
    }
}

fn is_t3_metadata_prompt(line: &str) -> bool {
    matches!(
        serde_json::from_str::<JsonlMessageEntry>(line)
            .ok()
            .and_then(|entry| entry.message)
            .and_then(|message| message.content)
            .as_ref()
            .and_then(|content| content.as_str())
            .and_then(|content| content.lines().next()),
        Some("You write concise thread titles for coding conversations.")
            | Some("You generate concise git branch names.")
    )
}

pub(crate) const PROGRAMOWANIE_DIR: &str = "Programowanie";
const WORKTREE_PATH_MARKER: &str = "/.t3/worktrees/";
const WORKTREE_DIR_PREFIX: &str = "--t3-worktrees-";
const WORKTREE_DIR_SUFFIX: &str = "-t3code-";

/// Canonical raw key for a project source so hours from variant names merge:
/// - Claude Code worktree dirs (`…-<projekt>--claude-worktrees-<id>`) → main project
/// - legacy `-home-jarx-` prefix → `-home-jarek-`
fn canonical_project_key(name: &str) -> String {
    let name = name
        .split_once("--claude-worktrees-")
        .map(|(project, _)| project)
        .unwrap_or(name);
    name.replace("-home-jarx-", "-home-jarek-")
}

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

    extract_project_from_path(file_path)
}

fn extract_project_from_path(file_path: &str) -> Option<String> {
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
    Some(format!("-home-jarek-Programowanie-{}", normalized))
}

fn build_sessions_from_records<R: Borrow<TimestampRecord>>(
    records: &[R],
    debug: bool,
) -> Vec<Session> {
    let mut sessions = Vec::new();

    if records.is_empty() {
        return sessions;
    }

    let first = records[0].borrow();
    let mut session_start = first.timestamp;
    let mut session_end = first.timestamp;
    // Klucze pożyczone z rekordów: String powstaje raz na sesję, nie na rekord.
    let mut session_projects: HashMap<&str, usize> = HashMap::new();
    session_projects.insert(&*first.project, 1);
    let mut session_has_claude = !first.is_codex;
    let mut session_has_codex = first.is_codex;
    let mut session_count = 0;

    for record in &records[1..] {
        let record = record.borrow();
        let gap = (record.timestamp - session_end).num_seconds();

        if gap > SESSION_GAP_SECONDS {
            let duration = (session_end - session_start).num_seconds();
            if duration >= MIN_SESSION_SECONDS {
                let dominant_project = session_projects
                    .iter()
                    .max_by_key(|(_, count)| *count)
                    .map(|(proj, _)| proj.to_string())
                    .unwrap_or_else(|| "unknown".to_string());

                sessions.push(Session {
                    id: format!("global-{}", session_count),
                    project: dominant_project,
                    project_counts: owned_counts(&session_projects),
                    start_time: session_start,
                    end_time: session_end,
                    duration_seconds: duration,
                    has_claude: session_has_claude,
                    has_codex: session_has_codex,
                });
                session_count += 1;
            }
            session_start = record.timestamp;
            session_projects.clear();
            session_has_claude = false;
            session_has_codex = false;
        }
        session_end = record.timestamp;
        session_has_claude |= !record.is_codex;
        session_has_codex |= record.is_codex;
        *session_projects.entry(&*record.project).or_insert(0) += 1;
    }

    let duration = (session_end - session_start).num_seconds();
    if duration >= MIN_SESSION_SECONDS {
        let dominant_project = session_projects
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(proj, _)| proj.to_string())
            .unwrap_or_else(|| "unknown".to_string());

        sessions.push(Session {
            id: format!("global-{}", session_count),
            project: dominant_project,
            project_counts: owned_counts(&session_projects),
            start_time: session_start,
            end_time: session_end,
            duration_seconds: duration,
            has_claude: session_has_claude,
            has_codex: session_has_codex,
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

fn owned_counts(counts: &HashMap<&str, usize>) -> HashMap<String, usize> {
    counts
        .iter()
        .map(|(name, count)| (name.to_string(), *count))
        .collect()
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
                    return format!("-home-jarek-Programowanie-{}", normalized);
                }
            }
            return canonical_project_key(&parent_name);
        }
    }

    "unknown".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(timestamp: &str, project: &str) -> TimestampRecord {
        TimestampRecord {
            timestamp: NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%d %H:%M:%S").unwrap(),
            project: project.into(),
            is_codex: false,
        }
    }

    fn codex_record(timestamp: &str, project: &str) -> TimestampRecord {
        TimestampRecord {
            is_codex: true,
            ..record(timestamp, project)
        }
    }

    #[test]
    fn test_parse_timestamp() {
        let ts = "2026-01-28T06:58:16.234Z";
        let parsed = parse_timestamp(ts).unwrap();
        assert_eq!(parsed.to_string(), "2026-01-28 06:58:16");
    }

    #[test]
    fn t3_metadata_helpers_are_not_work_sessions() {
        for prompt in [
            "You write concise thread titles for coding conversations.",
            "You generate concise git branch names.",
        ] {
            let line = serde_json::json!({
                "type": "user",
                "message": {"content": prompt}
            })
            .to_string();
            assert!(is_t3_metadata_prompt(&line));
        }
    }

    #[test]
    fn claude_only_session_values_stay_unchanged() {
        let project = "-home-jarek-Programowanie-farmaster2";
        let records = vec![
            record("2026-07-12 04:52:00", project),
            record("2026-07-12 05:21:00", project),
            record("2026-07-12 07:11:00", project),
            record("2026-07-12 07:35:00", project),
            record("2026-07-12 08:00:00", project),
            record("2026-07-12 08:20:00", project),
            record("2026-07-12 08:43:00", project),
        ];

        let sessions = build_sessions_from_records(&records, false);

        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].duration_seconds, 29 * 60);
        assert_eq!(sessions[1].duration_seconds, 92 * 60);
        assert!(
            sessions
                .iter()
                .all(|session| session.has_claude && !session.has_codex)
        );
    }

    #[test]
    fn overlapping_claude_and_codex_activity_is_counted_once() {
        let claude = "-home-jarek-Programowanie-claude";
        let codex = "-home-jarek-Programowanie-codex";
        let mut records = vec![
            record("2026-07-12 10:00:00", claude),
            record("2026-07-12 10:20:00", claude),
            record("2026-07-12 10:40:00", claude),
            record("2026-07-12 11:00:00", claude),
            codex_record("2026-07-12 10:05:00", codex),
            codex_record("2026-07-12 10:25:00", codex),
            codex_record("2026-07-12 10:45:00", codex),
            codex_record("2026-07-12 10:55:00", codex),
        ];
        records.sort_by_key(|record| record.timestamp);

        let sessions = build_sessions_from_records(&records, false);

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].duration_seconds, 60 * 60);
        assert!(sessions[0].has_claude);
        assert!(sessions[0].has_codex);

        let codex_only = build_sessions_from_records(
            &[
                codex_record("2026-07-12 12:00:00", codex),
                codex_record("2026-07-12 12:10:00", codex),
            ],
            false,
        );
        assert!(!codex_only[0].has_claude);
        assert!(codex_only[0].has_codex);
    }

    #[test]
    fn codex_session_meta_maps_cwd_to_project() {
        let entry: JsonlEntry = serde_json::from_str(
            r#"{"timestamp":"2026-07-13T09:59:16.433Z","type":"session_meta","payload":{"cwd":"/home/jarek/Programowanie/farmaster2"}}"#,
        )
        .unwrap();
        let cwd = entry
            .payload
            .as_ref()
            .and_then(|p| p.cwd.as_deref())
            .unwrap();

        assert_eq!(
            extract_project_from_path(cwd).as_deref(),
            Some("-home-jarek-Programowanie-farmaster2")
        );
    }

    #[test]
    fn manual_override_uses_stored_hours_not_projects() {
        // Dzień z ręczną korektą: stored 9.0, projekty sumują się do 2.5 → liczy się 9.0.
        assert_eq!(resolve_day_hours(true, 9.0, 2.5), 9.0);
    }

    #[test]
    fn without_manual_override_uses_recalculated_projects() {
        // Bez flagi: ignoruje stored, bierze sumę z projektów.
        assert_eq!(resolve_day_hours(false, 9.0, 2.5), 2.5);
    }

    #[test]
    fn test_extract_project_name() {
        let path = Path::new(
            "/home/jarx/.claude/projects/-home-jarx-Programowanie-farmaster2/session.jsonl",
        );
        let name = extract_project_name(path);
        // Legacy jarx prefix is canonicalized to jarek
        assert_eq!(name, "-home-jarek-Programowanie-farmaster2");
    }

    #[test]
    fn claude_worktree_dirs_map_to_main_project() {
        let path = Path::new(
            "/home/jarek/.claude/projects/-home-jarek-Programowanie-farmaster2--claude-worktrees-awesome-leakey-d15f52/session.jsonl",
        );
        assert_eq!(
            extract_project_name(path),
            "-home-jarek-Programowanie-farmaster2"
        );
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
