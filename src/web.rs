use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post},
};
use chrono::{Datelike, Duration, NaiveDate, TimeZone, Utc};
use chrono_tz::Europe::Warsaw;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs, net::SocketAddr, sync::Arc};
use tokio::sync::Mutex;

use crate::{archive, config, jsonl, overtime, pdf, report, schedule};

const INDEX_HTML: &str = include_str!("web/index.html");
type ApiError = (StatusCode, String);

#[derive(Clone)]
struct AppState {
    config: Arc<std::sync::RwLock<config::Config>>,
    mutation: Arc<Mutex<()>>,
}

impl AppState {
    fn config(&self) -> config::Config {
        self.config.read().unwrap().clone()
    }
}

pub fn router() -> Router {
    Router::new()
        .route("/", get(|| async { Html(INDEX_HTML) }))
        .route("/api/month/{month}", get(get_month))
        .route("/api/day/{date}", get(get_day).put(put_day))
        .route("/api/day/{date}/override", delete(delete_override))
        .route("/api/day/{date}/lock", post(lock_day))
        .route("/api/day/{date}/note", axum::routing::put(put_note))
        .route("/api/day/{date}/git", get(get_day_git))
        .route(
            "/api/day/{date}/git-summary",
            get(get_git_summary).post(post_git_summary),
        )
        .route("/api/rebuild", post(rebuild))
        .route("/api/shift", axum::routing::put(put_shift))
        .route("/api/projects", get(get_projects))
        .route("/api/months", get(get_months))
        .route("/api/report/{file}", get(get_pdf))
        .route("/api/invoice/{month}", get(get_invoice))
        .route(
            "/api/invoice/{month}/summaries",
            get(get_invoice_summaries)
                .post(post_invoice_summaries)
                .put(put_invoice_summary),
        )
        .with_state(AppState {
            config: Arc::new(std::sync::RwLock::new(config::load_config())),
            mutation: Arc::new(Mutex::new(())),
        })
}

pub fn serve(bind: &str) {
    let address: SocketAddr = bind.parse().unwrap_or_else(|_| {
        eprintln!("[BŁĄD] Nieprawidłowy adres --bind: {bind}");
        std::process::exit(1);
    });
    let runtime = tokio::runtime::Runtime::new().expect("Nie można uruchomić Tokio");
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(address)
            .await
            .unwrap_or_else(|e| {
                eprintln!("[BŁĄD] Nie można uruchomić serwera na {address}: {e}");
                std::process::exit(1);
            });
        println!("After15 web: http://{address}");
        tokio::spawn(auto_summary_loop());
        axum::serve(listener, router()).await.unwrap_or_else(|e| {
            eprintln!("[BŁĄD] Serwer zakończył pracę: {e}");
        });
    });
}

fn bad_request(kind: &str, value: &str) -> ApiError {
    (
        StatusCode::BAD_REQUEST,
        format!("Nieprawidłowy {kind}: {value}"),
    )
}

fn parse_date(value: &str) -> Result<NaiveDate, ApiError> {
    if value.len() != 10 {
        return Err(bad_request("format daty (użyj YYYY-MM-DD)", value));
    }
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| bad_request("format daty (użyj YYYY-MM-DD)", value))
}

fn parse_month(value: &str) -> Result<NaiveDate, ApiError> {
    if value.len() != 7 {
        return Err(bad_request("format miesiąca (użyj YYYY-MM)", value));
    }
    NaiveDate::parse_from_str(&format!("{value}-01"), "%Y-%m-%d")
        .map_err(|_| bad_request("format miesiąca (użyj YYYY-MM)", value))
}

fn today() -> NaiveDate {
    Utc::now().with_timezone(&Warsaw).date_naive()
}

fn project_hours(entry: &archive::ProjectHoursEntry) -> jsonl::ProjectHours {
    jsonl::ProjectHours {
        weekday_hours: entry.weekday_hours,
        weekend_hours: entry.weekend_hours,
        regular_hours: entry.regular_hours,
    }
}

fn summary_projects(
    summary: &archive::DailySummaryFile,
) -> HashMap<NaiveDate, HashMap<String, jsonl::ProjectHours>> {
    summary
        .days
        .iter()
        .filter_map(|(date, day)| {
            let projects = day.projects.as_ref()?;
            // Manual day total overrides the computed one — scale project hours
            // proportionally so per-project sums (and PLN) match the correction.
            let computed: f64 = projects
                .values()
                .map(|hours| hours.weekday_hours + hours.weekend_hours)
                .sum();
            let factor = if day.manual_override && computed > 0.0 {
                day.hours / computed
            } else {
                1.0
            };
            Some((
                NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?,
                projects
                    .iter()
                    .map(|(name, hours)| {
                        let mut hours = project_hours(hours);
                        hours.weekday_hours *= factor;
                        hours.weekend_hours *= factor;
                        (name.clone(), hours)
                    })
                    .collect(),
            ))
        })
        .collect()
}

#[derive(Serialize)]
struct MonthProject {
    name: String,
    weekday_hours: f64,
    weekend_hours: f64,
    regular_hours: f64,
    /// Kwota netto za dodatkowe godziny tego projektu w tym dniu (stawka dnia).
    pln: f64,
}

#[derive(Serialize)]
struct MonthDay {
    date: String,
    hours: f64,
    formatted: String,
    shift: String,
    shift_overridden: bool,
    manual_override: bool,
    has_note: bool,
    has_summary: bool,
    source: String,
    /// Kwota netto za dodatkowe godziny dnia: godziny x `config.day_rate(date)`.
    pln: f64,
    projects: Vec<MonthProject>,
}

#[derive(Serialize)]
struct Rates {
    weekday_pln: f64,
    weekend_pln: f64,
    b2b_pln: f64,
}

#[derive(Serialize)]
struct MonthResponse {
    days: Vec<MonthDay>,
    total_hours: f64,
    total_formatted: String,
    days_count: usize,
    average_hours: f64,
    average_formatted: String,
    average_months: usize,
    rates: Rates,
    total_pln: f64,
    /// Data przejscia na B2B (ISO) albo null.
    b2b_from: Option<String>,
}

/// Nazwa kubelka na godziny dnia, ktorych nie da sie przypisac do projektu:
/// reczne korekty i dni sprzed sledzenia projektow.
pub const UNASSIGNED: &str = "bez przypisania";

/// Godziny dnia poza projektami — dzieki temu suma po projektach = suma dnia.
fn unassigned_hours(hours: f64, projects: &HashMap<String, jsonl::ProjectHours>) -> f64 {
    let attributed: f64 = projects
        .values()
        .map(|entry| entry.weekday_hours + entry.weekend_hours)
        .sum();
    (hours - attributed).max(0.0)
}

/// Godziny i projekty kazdego dnia miesiaca tak, jak widzi je widok miesiaca:
/// archiwum, a dla dzisiaj (bez recznej korekty) swieze wyliczenie z jsonl.
fn month_day_data(
    summary: &archive::DailySummaryFile,
    first: NaiveDate,
    config: &config::Config,
) -> Vec<(NaiveDate, f64, HashMap<String, jsonl::ProjectHours>)> {
    let archived = summary_projects(summary);
    let live = (first.year() == today().year() && first.month() == today().month())
        .then(|| cached_compute_day(today(), config));
    let mut days = Vec::new();
    let mut date = first;
    while date.month() == first.month() {
        let stored = summary.days.get(&date.to_string());
        let use_live = date == today() && !stored.is_some_and(|day| day.manual_override);
        let (hours, projects) = match (use_live, live.as_ref()) {
            (true, Some(live)) => (live.hours, live.projects.clone()),
            _ => (
                stored.map(|day| day.hours).unwrap_or(0.0),
                archived.get(&date).cloned().unwrap_or_default(),
            ),
        };
        days.push((date, hours, projects));
        date += Duration::days(1);
    }
    days
}

fn month_project_rows(
    projects: &HashMap<String, jsonl::ProjectHours>,
    hours: f64,
    rate: f64,
    weekend: bool,
    b2b: bool,
    tracked_path: &str,
) -> Vec<MonthProject> {
    // Nazwy po normalizacji (worktree → repo główne), więc kilka surowych kluczy
    // może zlać się w jeden wiersz.
    let mut merged: HashMap<String, MonthProject> = HashMap::new();
    for (name, hours) in projects {
        let name = report::normalize_project_name(name, tracked_path);
        let row = merged.entry(name.clone()).or_insert(MonthProject {
            name,
            weekday_hours: 0.0,
            weekend_hours: 0.0,
            regular_hours: 0.0,
            pln: 0.0,
        });
        row.weekday_hours += hours.weekday_hours;
        row.weekend_hours += hours.weekend_hours;
        row.regular_hours += hours.regular_hours;
        row.pln += (hours.weekday_hours + hours.weekend_hours) * rate;
    }
    let mut rows: Vec<_> = merged.into_values().collect();
    // Tylko dni B2B — na starszych miesiacach kubelek zmienilby widok i kwoty,
    // a faktura i tak liczy wylacznie dni B2B.
    let rest = unassigned_hours(hours, projects);
    if b2b && rest > 0.0001 {
        rows.push(MonthProject {
            name: UNASSIGNED.to_string(),
            weekday_hours: if weekend { 0.0 } else { rest },
            weekend_hours: if weekend { rest } else { 0.0 },
            regular_hours: 0.0,
            pln: rest * rate,
        });
    }
    rows.sort_by(|a, b| {
        (b.weekday_hours + b.weekend_hours + b.regular_hours)
            .total_cmp(&(a.weekday_hours + a.weekend_hours + a.regular_hours))
    });
    rows
}

async fn get_month(
    State(state): State<AppState>,
    Path(month): Path<String>,
) -> Result<Json<MonthResponse>, ApiError> {
    let first = parse_month(&month)?;
    tokio::task::spawn_blocking(move || {
        let config = state.config();
        let summary = archive::load_summary_checked().map_err(internal)?;
        // presence only — fingerprint check would need a git scan per day
        let git_summaries = load_git_summaries();
        let mut days = Vec::new();
        for (date, hours, projects) in month_day_data(&summary, first, &config) {
            let key = date.to_string();
            let stored = summary.days.get(&key);
            let use_live = date == today() && !stored.is_some_and(|day| day.manual_override);
            days.push(MonthDay {
                date: key,
                hours,
                formatted: archive::format_hm(hours),
                shift: config.shift_label(date),
                shift_overridden: config.shift_override(date).is_some() && !config.is_b2b(date),
                manual_override: stored.is_some_and(|day| day.manual_override),
                has_note: stored
                    .is_some_and(|day| day.note.as_deref().is_some_and(|n| !n.is_empty())),
                has_summary: git_summaries.contains_key(&date.to_string()),
                source: if stored.is_some_and(|day| day.manual_override) {
                    "ręczne"
                } else if use_live {
                    "jsonl"
                } else {
                    "archiwum"
                }
                .to_string(),
                pln: hours * config.day_rate(date),
                projects: month_project_rows(
                    &projects,
                    hours,
                    config.day_rate(date),
                    schedule::is_weekend(date),
                    config.is_b2b(date),
                    &config.projects.tracked_path,
                ),
            });
        }
        let total_hours = days.iter().map(|day| day.hours).sum();
        let (average_hours, average_months) = month_average(&summary);
        Ok(Json(MonthResponse {
            days_count: days.iter().filter(|day| day.hours > 0.0).count(),
            total_hours,
            total_formatted: archive::format_hm(total_hours),
            average_hours,
            average_formatted: archive::format_hm(average_hours),
            average_months,
            rates: Rates {
                weekday_pln: config.overtime_rate_weekday(),
                weekend_pln: config.overtime_rate_weekend(),
                b2b_pln: config.billing.hourly_net,
            },
            total_pln: days.iter().map(|day| day.pln).sum(),
            b2b_from: config.billing.b2b_from.map(|d| d.to_string()),
            days,
        }))
    })
    .await
    .map_err(join_error)?
}

#[derive(Serialize, Clone)]
struct SessionProject {
    name: String,
    share: f64,
}

#[derive(Serialize, Clone)]
struct DaySession {
    start: String,
    end: String,
    duration_s: i64,
    overtime_h: f64,
    source: &'static str,
    projects: Vec<SessionProject>,
}

#[derive(Serialize)]
struct WorkWindowResponse {
    start: String,
    end: String,
}

#[derive(Serialize)]
struct DayResponse {
    date: String,
    shift: String,
    shift_overridden: bool,
    work_window: Option<WorkWindowResponse>,
    sessions: Vec<DaySession>,
    computed_hours: f64,
    manual_override: bool,
    stored_hours: Option<f64>,
    excluded_sessions: Vec<String>,
    note: Option<String>,
}

#[derive(Default, Clone)]
struct ComputedDay {
    hours: f64,
    projects: HashMap<String, jsonl::ProjectHours>,
    sessions: Vec<DaySession>,
}

pub fn clip_session_to_date(session: &jsonl::Session, date: NaiveDate) -> Option<jsonl::Session> {
    let start = session.start_time.and_utc().with_timezone(&Warsaw);
    let end = session.end_time.and_utc().with_timezone(&Warsaw);
    let day_start = Warsaw
        .from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
        .single()?;
    let next_start = Warsaw
        .from_local_datetime(&(date + Duration::days(1)).and_hms_opt(0, 0, 0).unwrap())
        .single()?;
    let clipped_start = start.max(day_start);
    let clipped_end = end.min(next_start);
    if clipped_end <= clipped_start {
        return None;
    }
    Some(jsonl::Session {
        id: session.id.clone(),
        project: session.project.clone(),
        project_counts: session.project_counts.clone(),
        start_time: clipped_start.naive_utc(),
        end_time: clipped_end.naive_utc(),
        duration_seconds: (clipped_end - clipped_start).num_seconds(),
        has_claude: session.has_claude,
        has_codex: session.has_codex,
    })
}

fn session_projects(session: &jsonl::Session, tracked_path: &str) -> Vec<SessionProject> {
    let total: usize = session
        .project_counts
        .iter()
        .filter(|(name, _)| name.as_str() != "transcripts")
        .map(|(_, count)| count)
        .sum();
    if total == 0 {
        return vec![SessionProject {
            name: "unknown".to_string(),
            share: 1.0,
        }];
    }
    let mut projects: Vec<_> = session
        .project_counts
        .iter()
        .filter(|(name, _)| name.as_str() != "transcripts")
        .map(|(name, count)| SessionProject {
            name: report::normalize_project_name(name, tracked_path),
            share: *count as f64 / total as f64,
        })
        .collect();
    projects.sort_by(|a, b| b.share.total_cmp(&a.share));
    projects
}

// ponytail: unbounded in-process map — a handful of clicked dates, restart clears it
static DAY_CACHE: std::sync::LazyLock<std::sync::Mutex<HashMap<NaiveDate, (u64, ComputedDay)>>> =
    std::sync::LazyLock::new(Default::default);

fn cached_compute_day(date: NaiveDate, config: &config::Config) -> ComputedDay {
    let fingerprint = jsonl::files_fingerprint_for_date(date);
    if let Some((cached_fp, cached)) = DAY_CACHE.lock().unwrap().get(&date) {
        if *cached_fp == fingerprint {
            return cached.clone();
        }
    }
    let computed = compute_day(date, config);
    DAY_CACHE
        .lock()
        .unwrap()
        .insert(date, (fingerprint, computed.clone()));
    computed
}

fn compute_day(date: NaiveDate, config: &config::Config) -> ComputedDay {
    let mut result = ComputedDay::default();
    for session in jsonl::load_sessions_for_date(date, config, false) {
        let Some(clipped) = clip_session_to_date(&session, date) else {
            continue;
        };
        let overtime_h = overtime::calculate_session_overtime(&clipped, date, config, false)
            .get(&date)
            .copied()
            .unwrap_or(0.0);
        let regular_h = overtime::calculate_session_regular(&clipped, config)
            .get(&date)
            .copied()
            .unwrap_or(0.0);
        let projects = session_projects(&clipped, &config.projects.tracked_path);
        for project in &projects {
            let entry = result.projects.entry(project.name.clone()).or_default();
            if schedule::is_weekend(date) {
                entry.weekend_hours += overtime_h * project.share;
            } else {
                entry.weekday_hours += overtime_h * project.share;
            }
            entry.regular_hours += regular_h * project.share;
        }
        result.hours += overtime_h;
        let start = clipped.start_time.and_utc().with_timezone(&Warsaw);
        let end = clipped.end_time.and_utc().with_timezone(&Warsaw);
        result.sessions.push(DaySession {
            start: start.format("%H:%M").to_string(),
            end: if end.date_naive() > date {
                "24:00".to_string()
            } else {
                end.format("%H:%M").to_string()
            },
            duration_s: clipped.duration_seconds,
            overtime_h,
            source: match (clipped.has_claude, clipped.has_codex) {
                (true, true) => "mixed",
                (false, true) => "codex",
                _ => "claude",
            },
            projects,
        });
    }
    result
}

async fn get_day(
    State(state): State<AppState>,
    Path(date): Path<String>,
) -> Result<Json<DayResponse>, ApiError> {
    let date = parse_date(&date)?;
    tokio::task::spawn_blocking(move || day_response(date, &state.config()))
        .await
        .map_err(join_error)?
}

fn day_response(date: NaiveDate, config: &config::Config) -> Result<Json<DayResponse>, ApiError> {
    let summary = archive::load_summary_checked().map_err(internal)?;
    let stored = summary.days.get(&date.to_string());
    let computed = cached_compute_day(date, config);
    let window = config.effective_work_window(date);
    Ok(Json(DayResponse {
        date: date.to_string(),
        shift: config.shift_label(date),
        shift_overridden: config.shift_override(date).is_some() && !config.is_b2b(date),
        work_window: window.map(|window| WorkWindowResponse {
            start: window.start.format("%H:%M").to_string(),
            end: window.end.format("%H:%M").to_string(),
        }),
        sessions: computed.sessions,
        computed_hours: computed.hours,
        manual_override: stored.is_some_and(|day| day.manual_override),
        stored_hours: stored.map(|day| day.hours),
        excluded_sessions: stored
            .map(|day| day.excluded_sessions.clone())
            .unwrap_or_default(),
        note: stored.and_then(|day| day.note.clone()),
    }))
}

#[derive(Deserialize)]
struct HoursInput {
    hours: String,
    #[serde(default)]
    exclude_session: Option<String>,
}

async fn put_day(
    State(state): State<AppState>,
    Path(date): Path<String>,
    Json(input): Json<HoursInput>,
) -> Result<Json<DayResponse>, ApiError> {
    let date = parse_date(&date)?;
    let hours = crate::tui::state::parse_hours(&input.hours)
        .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e))?;
    mutate_day(state, date, move |summary, config| {
        let key = date.to_string();
        let entry = summary
            .days
            .entry(key)
            .or_insert_with(|| archive::day_entry(date, 0.0, None, false, config));
        entry.hours = hours;
        entry.formatted = archive::format_hm(hours);
        entry.processed = true;
        entry.manual_override = true;
        if let Some(key) = input.exclude_session {
            if !entry.excluded_sessions.contains(&key) {
                entry.excluded_sessions.push(key);
            }
        }
        Ok(())
    })
    .await
}

async fn delete_override(
    State(state): State<AppState>,
    Path(date): Path<String>,
) -> Result<Json<DayResponse>, ApiError> {
    let date = parse_date(&date)?;
    mutate_day(state, date, move |summary, config| {
        let computed = cached_compute_day(date, config);
        summary.days.insert(
            date.to_string(),
            archive::day_entry(
                date,
                computed.hours,
                Some(&computed.projects),
                false,
                config,
            ),
        );
        Ok(())
    })
    .await
}

async fn lock_day(
    State(state): State<AppState>,
    Path(date): Path<String>,
) -> Result<Json<DayResponse>, ApiError> {
    let date = parse_date(&date)?;
    mutate_day(state, date, move |summary, config| {
        let key = date.to_string();
        if !summary.days.contains_key(&key) {
            let computed = cached_compute_day(date, config);
            summary.days.insert(
                key.clone(),
                archive::day_entry(
                    date,
                    computed.hours,
                    Some(&computed.projects),
                    false,
                    config,
                ),
            );
        }
        summary.days.get_mut(&key).unwrap().manual_override = true;
        Ok(())
    })
    .await
}

#[derive(Serialize)]
struct GitCommit {
    time: String,
    subject: String,
}

#[derive(Serialize)]
struct GitProject {
    project: String,
    commits: Vec<GitCommit>,
}

/// `git log` w jednym repo za `from..=to`. `author` (dni B2B) odsiewa commity
/// z konta pracowniczego i cudze, sciagniete przez pulla.
fn repo_commits(
    path: &std::path::Path,
    from: NaiveDate,
    to: NaiveDate,
    author: Option<&str>,
    time_format: &str,
) -> Option<GitProject> {
    let mut args: Vec<String> = vec![
        "log".into(),
        // t3 tworzy refs/t3/checkpoints/* przy kazdej turze agenta. To nie sa
        // commity uzytkownika, zasmiecalyby podsumowanie AI.
        "--exclude=refs/t3/*".into(),
        "--all".into(),
        "--no-merges".into(),
        "--since".into(),
        format!("{from} 00:00:00"),
        "--until".into(),
        format!("{to} 23:59:59"),
        format!("--date=format-local:{time_format}"),
        "--pretty=format:%ad\t%s".into(),
    ];
    if let Some(author) = author {
        args.push(format!("--author={author}"));
    }
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(&args)
        .output()
        .ok()?;
    let mut commits: Vec<GitCommit> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (time, subject) = line.split_once('\t')?;
            Some(GitCommit {
                time: time.to_string(),
                subject: subject.chars().take(120).collect(),
            })
        })
        .collect();
    if commits.is_empty() {
        return None;
    }
    commits.sort_by(|a, b| a.time.cmp(&b.time));
    Some(GitProject {
        project: path.file_name()?.to_string_lossy().to_string(),
        commits,
    })
}

fn collect_day_git(date: NaiveDate, config: &config::Config) -> Result<Vec<GitProject>, ApiError> {
    collect_git_range(date, date, config.git_author_for(date), "%H:%M", config)
}

/// `git log` za `from..=to` we wszystkich repo z `~/<tracked_path>` i `extra_git_paths`.
fn collect_git_range(
    from: NaiveDate,
    to: NaiveDate,
    author: Option<&str>,
    time_format: &str,
    config: &config::Config,
) -> Result<Vec<GitProject>, ApiError> {
    let home = dirs::home_dir().ok_or_else(|| internal("Brak katalogu domowego".into()))?;
    // Brak katalogu z repo (swieza maszyna, test) = brak commitow, nie blad.
    let mut repos: Vec<std::path::PathBuf> = fs::read_dir(home.join(&config.projects.tracked_path))
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default();
    // Repozytoria spoza tracked_path, np. ~/after15-core.
    repos.extend(config.projects.extra_git_paths.iter().map(|p| home.join(p)));
    let mut projects: Vec<GitProject> = repos
        .iter()
        // Linked worktrees have a `.git` FILE — the main repo already lists
        // their commits via --all, so only real `.git` dirs count.
        .filter(|path| path.join(".git").is_dir())
        .filter_map(|path| repo_commits(path, from, to, author, time_format))
        .collect();
    projects.sort_by(|a, b| a.project.cmp(&b.project));
    projects.dedup_by(|a, b| a.project == b.project);
    Ok(projects)
}

async fn get_day_git(
    State(state): State<AppState>,
    Path(date): Path<String>,
) -> Result<Json<Vec<GitProject>>, ApiError> {
    let date = parse_date(&date)?;
    tokio::task::spawn_blocking(move || collect_day_git(date, &state.config()).map(Json))
        .await
        .map_err(join_error)?
}

// AI day summaries are billed per call — persist by commit-set hash so repeat
// clicks and page/service restarts don't regenerate (and don't re-bill).
fn git_summaries_path() -> Option<std::path::PathBuf> {
    dirs::data_dir()
        .or_else(|| dirs::home_dir().map(|p| p.join(".local/share")))
        .map(|p| p.join("claude-overtime/git_summaries.json"))
}

fn load_git_summaries() -> HashMap<String, (u64, String)> {
    git_summaries_path()
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default()
}

// Jeden zamek na plik — read-modify-write z dwoch requestow nie moze sie nadpisac.
static SUMMARIES_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(Default::default);

/// Dopisuje jedno podsumowanie. Zapis po kazdym udanym wywolaniu AI, zeby blad
/// kolejnego repo nie kasowal tego, za co user juz zaplacil.
fn store_git_summary(key: String, fingerprint: u64, summary: &str) {
    let _guard = SUMMARIES_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut stored = load_git_summaries();
    stored.insert(key, (fingerprint, summary.to_string()));
    save_git_summaries(&stored);
}

fn save_git_summaries(map: &HashMap<String, (u64, String)>) {
    if let Some(path) = git_summaries_path() {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(path, serde_json::to_string(map).unwrap_or_default());
    }
}

/// Zapas: claude CLI (subscription auth) z małym Haiku, gdy Codex zawiedzie.
/// --strict-mcp-config i --setting-sources '' są nośne: bez nich każde wywołanie startuje
/// wszystkie serwery MCP, hooki i pluginy z ~/.claude (~5,7 s CPU i ~324 MB zamiast ~0,9 s),
/// a lista commitów staje się osiągalna dla serwerów pamięci w rodzaju Hindsight.
/// Nie dodawać --tools '': zmierzone 140 s wobec 29 s bez tej flagi.
fn run_claude(prompt: &str) -> Result<String, ApiError> {
    use std::io::Write;
    let claude_bin = dirs::home_dir()
        .map(|h| h.join(".local/bin/claude"))
        .filter(|p| p.exists())
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "claude".to_string());
    let mut child = std::process::Command::new("timeout")
        .args([
            "120",
            &claude_bin,
            "-p",
            "--model",
            "claude-haiku-4-5",
            "--strict-mcp-config",
            "--setting-sources",
            "",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| internal(format!("Nie można uruchomić claude: {e}")))?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(prompt.as_bytes())
        .map_err(|e| internal(e.to_string()))?;
    let output = child
        .wait_with_output()
        .map_err(|e| internal(e.to_string()))?;
    let summary = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || summary.is_empty() {
        return Err(internal(git_summary_error(&String::from_utf8_lossy(
            &output.stderr,
        ))));
    }
    Ok(summary)
}

/// Model Codexa do podsumowań (wybór Jarka, 2026-09-07).
const CODEX_MODEL: &str = "gpt-5.6-luna";

/// Codex z modelem `CODEX_MODEL`. `--ephemeral` nie zostawia pliku sesji
/// w ~/.codex/sessions, więc podsumowanie nie wpada do liczenia nadgodzin.
/// `-o` daje samą odpowiedź, bez logu przebiegu.
fn run_codex(prompt: &str) -> Result<String, ApiError> {
    use std::io::Write;
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let out_path =
        std::env::temp_dir().join(format!("after15-codex-{}-{seq}.txt", std::process::id()));
    let mut child = std::process::Command::new("timeout")
        .args([
            "120",
            "codex",
            "exec",
            "--ephemeral",
            "--skip-git-repo-check",
            "-s",
            "read-only",
            "-C",
            "/tmp",
            "-m",
            CODEX_MODEL,
            "-o",
            &out_path.to_string_lossy(),
            "-",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| internal(format!("Nie można uruchomić codex: {e}")))?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(prompt.as_bytes())
        .map_err(|e| internal(e.to_string()))?;
    let output = child
        .wait_with_output()
        .map_err(|e| internal(e.to_string()))?;
    let summary = fs::read_to_string(&out_path)
        .unwrap_or_default()
        .trim()
        .to_string();
    let _ = fs::remove_file(&out_path);
    if !output.status.success() || summary.is_empty() {
        eprintln!(
            "[WARN] codex exec nieudany: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return Err(internal(
            "Nie udało się wygenerować podsumowania".to_string(),
        ));
    }
    Ok(summary)
}

/// Najpierw Codex (Luna), przy błędzie claude (Haiku) jako zapas.
fn run_ai(prompt: &str) -> Result<String, ApiError> {
    match run_codex(prompt) {
        Ok(summary) => Ok(summary),
        Err((_, message)) => {
            eprintln!("[WARN] codex nieudany ({message}), próbuję claude");
            run_claude(prompt)
        }
    }
}

fn git_fingerprint(projects: &[GitProject], author: Option<&str>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "format-v2".hash(&mut hasher); // bump to invalidate summaries after prompt changes
    // Only hashed when set, so hashes of pre-cutover days stay identical.
    if let Some(author) = author {
        author.hash(&mut hasher);
    }
    for project in projects {
        project.project.hash(&mut hasher);
        for commit in &project.commits {
            commit.time.hash(&mut hasher);
            commit.subject.hash(&mut hasher);
        }
    }
    hasher.finish()
}

fn git_summary_error(stderr: &str) -> String {
    if stderr.contains("OAuth session expired") {
        "Sesja Claude wygasła — zaloguj się ponownie w terminalu poleceniem `claude`.".to_string()
    } else {
        "Nie udało się wygenerować podsumowania".to_string()
    }
}

async fn get_git_summary(
    State(state): State<AppState>,
    Path(date): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let date = parse_date(&date)?;
    tokio::task::spawn_blocking(move || {
        let config = state.config();
        let projects = collect_day_git(date, &config)?;
        let stored = load_git_summaries();
        let summary = stored
            .get(&date.to_string())
            .filter(|(fp, _)| *fp == git_fingerprint(&projects, config.git_author_for(date)))
            .map(|(_, s)| s.clone());
        Ok(Json(serde_json::json!({"summary": summary})))
    })
    .await
    .map_err(join_error)?
}

// Zwraca zapisane podsumowanie albo generuje nowe. Wywolanie AI jest platne,
// wiec generujemy tylko gdy odcisk zestawu commitow sie zmienil.
fn ensure_git_summary(date: NaiveDate, config: &config::Config) -> Result<String, ApiError> {
    let projects = collect_day_git(date, config)?;
    if projects.is_empty() {
        return Ok("Brak commitów tego dnia.".to_string());
    }
    let fingerprint = git_fingerprint(&projects, config.git_author_for(date));
    let stored = load_git_summaries();
    if let Some((cached_fp, cached)) = stored.get(&date.to_string()) {
        if *cached_fp == fingerprint {
            return Ok(cached.clone());
        }
    }
    let mut prompt = format!(
        "Na podstawie poniższej listy commitów gita z dnia {date} napisz po polsku zwięzłe \
         podsumowanie tego, co zostało zrobione. Dla KAŻDEGO projektu osobny akapit w formacie \
         dokładnie: 'NazwaProjektu: podsumowanie 1-3 zdaniami', akapity rozdzielone pustą linią. \
         Pisz o efektach a nie o commitach. Bez wstępów, nagłówków i markdownu — zwykły tekst.\n\n"
    );
    for project in &projects {
        prompt.push_str(&format!("Projekt {}:\n", project.project));
        for commit in &project.commits {
            prompt.push_str(&format!("- {} {}\n", commit.time, commit.subject));
        }
        prompt.push('\n');
    }
    let summary = run_ai(&prompt)?;
    store_git_summary(date.to_string(), fingerprint, &summary);
    Ok(summary)
}

async fn post_git_summary(
    State(state): State<AppState>,
    Path(date): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let date = parse_date(&date)?;
    tokio::task::spawn_blocking(move || {
        ensure_git_summary(date, &state.config())
            .map(|summary| Json(serde_json::json!({ "summary": summary })))
    })
    .await
    .map_err(join_error)?
}

// Raz na godzine dopisuje brakujace podsumowanie wczorajszego dnia. Gdy jest juz
// zapisane i commity sie nie zmienily, ensure_git_summary nie wola AI.
/// Dobowa archiwizacja wczorajszych godzin z JSONL. Wcześniej robił to tylko
/// pasek Claude Code (`after15 --statusline`), więc bez Claude archiwum stało.
/// Znacznik i lock są wspólne z paskiem, więc dzień zapisuje się raz.
fn daily_archive_tick() {
    let Some(_daily_lock) = archive::lock_daily_automation() else {
        return;
    };
    if !archive::needs_daily_archive() {
        return;
    }
    let Some(_archive_lock) = archive::try_lock_archive() else {
        return;
    };
    match crate::rebuild_archive(&config::load_config(), false, Some(2)) {
        Ok(stats) => {
            eprintln!("[INFO] Dobowe archiwum: {} dni z JSONL", stats.updated);
            archive::mark_daily_archive_done();
        }
        Err(message) => eprintln!("[WARN] Dobowe archiwum nieudane: {message}"),
    }
}

async fn auto_summary_loop() {
    loop {
        let _ = tokio::task::spawn_blocking(daily_archive_tick).await;
        let _ = tokio::task::spawn_blocking(|| {
            crate::telegram::auto_daily_backup(&config::load_config())
        })
        .await;
        let date = today() - Duration::days(1);
        let task =
            tokio::task::spawn_blocking(move || ensure_git_summary(date, &config::load_config()));
        if let Ok(Err((_, message))) = task.await {
            eprintln!("[WARN] Auto-podsumowanie {date} nieudane: {message}");
        }
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
    }
}

#[derive(Deserialize)]
struct NoteInput {
    note: String,
}

async fn put_note(
    State(state): State<AppState>,
    Path(date): Path<String>,
    Json(input): Json<NoteInput>,
) -> Result<Json<DayResponse>, ApiError> {
    let date = parse_date(&date)?;
    if input.note.chars().count() > 2000 {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            "Notatka może mieć maksymalnie 2000 znaków".to_string(),
        ));
    }
    let note = input.note.trim().to_string();
    mutate_day(state, date, move |summary, config| {
        let key = date.to_string();
        if !summary.days.contains_key(&key) {
            if note.is_empty() {
                return Ok(());
            }
            let computed = compute_day(date, config);
            summary.days.insert(
                key.clone(),
                archive::day_entry(
                    date,
                    computed.hours,
                    Some(&computed.projects),
                    false,
                    config,
                ),
            );
        }
        summary.days.get_mut(&key).unwrap().note = if note.is_empty() { None } else { Some(note) };
        Ok(())
    })
    .await
}

async fn mutate_day<F>(
    state: AppState,
    date: NaiveDate,
    operation: F,
) -> Result<Json<DayResponse>, ApiError>
where
    F: FnOnce(&mut archive::DailySummaryFile, &config::Config) -> Result<(), String>
        + Send
        + 'static,
{
    let _guard = state.mutation.lock().await;
    let config = state.config();
    tokio::task::spawn_blocking(move || {
        let _archive_lock = archive::try_lock_archive().ok_or_else(lock_unavailable)?;
        let mut summary = archive::load_summary_checked().map_err(internal)?;
        operation(&mut summary, &config).map_err(internal)?;
        archive::recalc_months(&mut summary);
        archive::save_summary(&summary).map_err(internal)?;
        day_response(date, &config)
    })
    .await
    .map_err(join_error)?
}

#[derive(Deserialize)]
struct RebuildQuery {
    days: Option<i64>,
}

async fn rebuild(
    State(state): State<AppState>,
    Query(query): Query<RebuildQuery>,
) -> Result<Json<crate::RebuildStats>, ApiError> {
    let recent_days = query.days.filter(|d| (1..=365).contains(d));
    let mutation = state.mutation.clone();
    let _guard = mutation.lock().await;
    tokio::task::spawn_blocking(move || {
        let _archive_lock = archive::try_lock_archive().ok_or_else(lock_unavailable)?;
        crate::rebuild_archive(&state.config(), false, recent_days)
            .map(Json)
            .map_err(internal)
    })
    .await
    .map_err(join_error)?
}

#[derive(Deserialize)]
struct ProjectsQuery {
    mode: Option<String>,
}

#[derive(Serialize)]
struct ProjectResponseRow {
    name: String,
    hours: f64,
    formatted: String,
    share_pct: f64,
    pln: f64,
    // Rozbicie: etatowe nadgodziny (do cutoveru) vs B2B (od cutoveru), godziny i PLN.
    overtime_hours: f64,
    overtime_pln: f64,
    b2b_hours: f64,
    b2b_pln: f64,
}

#[derive(Serialize)]
struct ProjectsResponse {
    mode: String,
    projects: Vec<ProjectResponseRow>,
    total_hours: f64,
    total_formatted: String,
    earned_pln: f64,
    overtime_hours: f64,
    overtime_pln: f64,
    b2b_hours: f64,
    b2b_pln: f64,
}

#[derive(Serialize)]
struct MonthsRow {
    month: String,
    hours: f64,
    formatted: String,
    current: bool,
}

#[derive(Serialize)]
struct MonthsResponse {
    months: Vec<MonthsRow>,
    average_hours: f64,
    average_formatted: String,
    average_months: usize,
    total_hours: f64,
    total_formatted: String,
}

fn current_month() -> String {
    let today = today();
    format!("{}-{:02}", today.year(), today.month())
}

/// Miesiące bieżącego roku, po kluczu "RRRR-MM".
fn this_year_months(
    summary: &archive::DailySummaryFile,
) -> impl Iterator<Item = (&String, &archive::MonthEntry)> {
    let prefix = format!("{}-", crate::current_year());
    summary
        .months
        .iter()
        .filter(move |(key, _)| key.starts_with(&prefix))
}

/// Średnia z zamkniętych miesięcy — bieżący jest niepełny i zaniżałby wynik.
fn month_average(summary: &archive::DailySummaryFile) -> (f64, usize) {
    let current = current_month();
    let past: Vec<f64> = this_year_months(summary)
        .filter(|(key, _)| key.as_str() != current)
        .map(|(_, month)| month.total_hours)
        .collect();
    if past.is_empty() {
        (0.0, 0)
    } else {
        (past.iter().sum::<f64>() / past.len() as f64, past.len())
    }
}

async fn get_months() -> Result<Json<MonthsResponse>, ApiError> {
    tokio::task::spawn_blocking(move || {
        let summary = archive::load_summary_checked().map_err(internal)?;
        let current = current_month();
        let months: Vec<MonthsRow> = this_year_months(&summary)
            .map(|(key, month)| MonthsRow {
                month: key.clone(),
                hours: month.total_hours,
                formatted: archive::format_hm(month.total_hours),
                current: key.as_str() == current,
            })
            .collect();
        let (average_hours, average_months) = month_average(&summary);
        let total_hours: f64 = months.iter().map(|month| month.hours).sum();
        Ok(Json(MonthsResponse {
            months,
            average_hours,
            average_formatted: archive::format_hm(average_hours),
            average_months,
            total_hours,
            total_formatted: archive::format_hm(total_hours),
        }))
    })
    .await
    .map_err(join_error)?
}

async fn get_projects(
    State(state): State<AppState>,
    Query(query): Query<ProjectsQuery>,
) -> Result<Json<ProjectsResponse>, ApiError> {
    let mode = query
        .mode
        .filter(|mode| mode == "overtime" || mode == "all")
        .ok_or_else(|| bad_request("tryb (użyj overtime lub all)", "mode"))?;
    tokio::task::spawn_blocking(move || {
        let summary = archive::load_summary_checked().map_err(internal)?;
        let full = mode == "all";
        let config = state.config();
        let totals = crate::calculate_project_totals(&summary_projects(&summary), &config, full);
        let values: Vec<_> = totals
            .into_iter()
            .map(|project| {
                let hours = project.hours.weekday_hours
                    + project.hours.weekend_hours
                    + if full {
                        project.hours.regular_hours
                    } else {
                        0.0
                    };
                // amount_pln jest liczone per dzien stawka tego dnia (B2B vs nadgodziny),
                // mnozenie sum miesiecznych przez jedna stawke zawyzaloby/zanizalo wrzesien 2026
                let extra = project.hours.weekday_hours + project.hours.weekend_hours;
                (
                    project.name,
                    hours,
                    project.amount_pln,
                    extra - project.b2b_hours,
                    project.amount_pln - project.b2b_pln,
                    project.b2b_hours,
                    project.b2b_pln,
                )
            })
            .collect();
        let total_hours: f64 = values.iter().map(|v| v.1).sum();
        let earned_pln: f64 = values.iter().map(|v| v.2).sum();
        let overtime_hours: f64 = values.iter().map(|v| v.3).sum();
        let overtime_pln: f64 = values.iter().map(|v| v.4).sum();
        let b2b_hours: f64 = values.iter().map(|v| v.5).sum();
        let b2b_pln: f64 = values.iter().map(|v| v.6).sum();
        Ok(Json(ProjectsResponse {
            mode,
            projects: values
                .into_iter()
                .map(
                    |(name, hours, pln, overtime_hours, overtime_pln, b2b_hours, b2b_pln)| {
                        ProjectResponseRow {
                            name,
                            hours,
                            formatted: archive::format_hm(hours),
                            share_pct: if total_hours == 0.0 {
                                0.0
                            } else {
                                hours / total_hours * 100.0
                            },
                            pln,
                            overtime_hours,
                            overtime_pln,
                            b2b_hours,
                            b2b_pln,
                        }
                    },
                )
                .collect(),
            total_hours,
            total_formatted: archive::format_hm(total_hours),
            earned_pln,
            overtime_hours,
            overtime_pln,
            b2b_hours,
            b2b_pln,
        }))
    })
    .await
    .map_err(join_error)?
}

async fn get_pdf(
    State(state): State<AppState>,
    Path(file): Path<String>,
) -> Result<Response, ApiError> {
    let month = file
        .strip_suffix(".pdf")
        .ok_or_else(|| bad_request("nazwę raportu (użyj YYYY-MM.pdf)", &file))?
        .to_string();
    parse_month(&month)?;
    let mutation = state.mutation.clone();
    let _guard = mutation.lock().await;
    tokio::task::spawn_blocking(move || {
        let _archive_lock = archive::try_lock_archive().ok_or_else(lock_unavailable)?;
        let summary = archive::load_summary_checked().map_err(internal)?;
        let path = pdf::generate_pdf(&summary_projects(&summary), &state.config(), Some(&month))
            .map_err(internal)?;
        let bytes = fs::read(path).map_err(|e| internal(e.to_string()))?;
        Ok((
            [(header::CONTENT_TYPE, "application/pdf")],
            Body::from(bytes),
        )
            .into_response())
    })
    .await
    .map_err(join_error)?
}

// ==== Zalacznik do faktury (B2B) ====

/// Pierwszy i ostatni dzien B2B w miesiacu `first`. None, gdy miesiac jest caly sprzed przejscia.
fn b2b_month_range(first: NaiveDate, config: &config::Config) -> Option<(NaiveDate, NaiveDate)> {
    let last = first
        .with_day(1)
        .unwrap()
        .checked_add_months(chrono::Months::new(1))?
        - Duration::days(1);
    let from = config.billing.b2b_from?.max(first);
    (from <= last).then_some((from, last))
}

/// Godziny i kwoty per repo za dni B2B miesiaca. Czysta agregacja — bez gita i bez AI.
/// `days` to te same dni co w widoku miesiaca (z `month_day_data`), wiec godziny bez
/// przypisania do projektu trafiaja do wiersza `UNASSIGNED` i suma faktury = suma miesiaca.
pub fn invoice_rows(
    days: &[(NaiveDate, f64, HashMap<String, jsonl::ProjectHours>)],
    config: &config::Config,
) -> Vec<pdf::InvoiceRow> {
    let mut hours: HashMap<String, f64> = HashMap::new();
    for (date, day_hours, projects) in days {
        if !config.is_b2b(*date) {
            continue;
        }
        for (raw_name, entry) in projects {
            let name = report::normalize_project_name(raw_name, &config.projects.tracked_path);
            if config.projects.excluded_projects.contains(&name) {
                continue;
            }
            let value = entry.weekday_hours + entry.weekend_hours;
            if value < 0.0001 {
                continue;
            }
            *hours.entry(name).or_default() += value;
        }
        let rest = unassigned_hours(*day_hours, projects);
        if rest > 0.0001 {
            *hours.entry(UNASSIGNED.to_string()).or_default() += rest;
        }
    }
    let mut rows: Vec<_> = hours
        .into_iter()
        .map(|(project, hours)| pdf::InvoiceRow {
            project,
            hours: archive::round2(hours),
            amount: archive::round2(hours * config.billing.hourly_net),
            summary: None,
        })
        .collect();
    rows.sort_by(|a, b| {
        b.hours
            .total_cmp(&a.hours)
            .then_with(|| a.project.cmp(&b.project))
    });
    rows
}

fn invoice_key(month: &str, project: &str) -> String {
    format!("invoice:{month}:{project}")
}

/// Wiersze faktury z opisami prac. Opis z cache'u tylko przy zgodnym fingerprincie commitow;
/// `generate = true` dowoluje brakujace przez claude CLI (platne), po jednym repo na raz.
/// Zwraca tez bledy poszczegolnych repo — jedno padniete nie przerywa reszty.
fn invoice_data(
    month: &str,
    first: NaiveDate,
    config: &config::Config,
    generate: bool,
) -> Result<(Vec<pdf::InvoiceRow>, Vec<String>), ApiError> {
    let summary = archive::load_summary_checked().map_err(internal)?;
    let mut rows = invoice_rows(&month_day_data(&summary, first, config), config);
    let mut errors = Vec::new();
    let Some((from, to)) = b2b_month_range(first, config) else {
        return Ok((rows, errors));
    };
    if rows.is_empty() {
        return Ok((rows, errors));
    }
    let author = config.git_author_for(from);
    let git = collect_git_range(from, to, author, "%Y-%m-%d %H:%M", config)?;
    let stored = load_git_summaries();
    for row in &mut rows {
        let Some(project) = git.iter().find(|p| p.project == row.project) else {
            continue;
        };
        let key = invoice_key(month, &row.project);
        let fingerprint = git_fingerprint(std::slice::from_ref(project), author);
        if let Some((cached_fp, cached)) = stored.get(&key) {
            if *cached_fp == fingerprint {
                row.summary = Some(cached.clone());
                continue;
            }
        }
        if !generate {
            continue;
        }
        let mut prompt = format!(
            "Na podstawie poniższej listy commitów gita z projektu {} za miesiąc {month} napisz \
             po polsku zwięzły opis prac wykonanych w tym miesiącu — 3-6 zdań, jako opis do \
             załącznika do faktury. Pisz o efektach dla projektu, nie o commitach. \
             Bez wstępów, nagłówków i markdownu — zwykły tekst.\n\n",
            row.project
        );
        for commit in &project.commits {
            prompt.push_str(&format!("- {} {}\n", commit.time, commit.subject));
        }
        match run_ai(&prompt) {
            Ok(summary) => {
                store_git_summary(key, fingerprint, &summary);
                row.summary = Some(summary);
            }
            Err((_, message)) => errors.push(format!("{}: {message}", row.project)),
        }
    }
    Ok((rows, errors))
}

#[derive(Serialize)]
struct InvoiceRowResponse {
    project: String,
    hours: f64,
    amount: f64,
    summary: Option<String>,
}

fn invoice_response(
    month: String,
    rows: Vec<pdf::InvoiceRow>,
    errors: Vec<String>,
    config: &config::Config,
) -> serde_json::Value {
    serde_json::json!({
        "month": month,
        "hourly_net": config.billing.hourly_net,
        "total_hours": archive::round2(rows.iter().map(|r| r.hours).sum()),
        "total_amount": archive::round2(rows.iter().map(|r| r.amount).sum()),
        "errors": errors,
        "projects": rows
            .into_iter()
            .map(|r| InvoiceRowResponse { project: r.project, hours: r.hours, amount: r.amount, summary: r.summary })
            .collect::<Vec<_>>(),
    })
}

async fn get_invoice_summaries(
    State(state): State<AppState>,
    Path(month): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let first = parse_month(&month)?;
    tokio::task::spawn_blocking(move || {
        let config = state.config();
        let (rows, errors) = invoice_data(&month, first, &config, false)?;
        Ok(Json(invoice_response(month, rows, errors, &config)))
    })
    .await
    .map_err(join_error)?
}

// Bez `state.mutation` — generowanie trwa minuty, a nie dotyka archiwum;
// plik z podsumowaniami chroni SUMMARIES_LOCK.
async fn post_invoice_summaries(
    State(state): State<AppState>,
    Path(month): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let first = parse_month(&month)?;
    tokio::task::spawn_blocking(move || {
        let config = state.config();
        let (rows, errors) = invoice_data(&month, first, &config, true)?;
        Ok(Json(invoice_response(month, rows, errors, &config)))
    })
    .await
    .map_err(join_error)?
}

#[derive(Deserialize)]
struct InvoiceSummaryInput {
    project: String,
    summary: String,
}

/// Ręczna korekta opisu do załącznika. Zapis pod fingerprintem bieżącego zestawu
/// commitów, więc PDF i GET widzą korektę tak samo jak tekst z AI; nowe commity
/// w tym repo unieważnią ją, jak każdy opis.
async fn put_invoice_summary(
    State(state): State<AppState>,
    Path(month): Path<String>,
    Json(input): Json<InvoiceSummaryInput>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let first = parse_month(&month)?;
    let summary = input.summary.trim().to_string();
    if summary.is_empty() || summary.len() > 4000 {
        return Err(bad_request("opis (1–4000 znaków)", &input.project));
    }
    tokio::task::spawn_blocking(move || {
        let config = state.config();
        let Some((from, to)) = b2b_month_range(first, &config) else {
            return Err(bad_request("miesiąc — brak dni B2B", &month));
        };
        let author = config.git_author_for(from);
        let git = collect_git_range(from, to, author, "%Y-%m-%d %H:%M", &config)?;
        let project = git
            .iter()
            .find(|p| p.project == input.project)
            .ok_or_else(|| bad_request("projekt — brak commitów w tym miesiącu", &input.project))?;
        let fingerprint = git_fingerprint(std::slice::from_ref(project), author);
        store_git_summary(invoice_key(&month, &input.project), fingerprint, &summary);
        Ok(Json(serde_json::json!({"project": input.project, "summary": summary})))
    })
    .await
    .map_err(join_error)?
}

/// PDF zalacznika. Czyta wylacznie zapisane podsumowania — generowanie idzie przez POST,
/// zeby otwarcie PDF-u nie odpalalo (platnego) claude CLI.
async fn get_invoice(
    State(state): State<AppState>,
    Path(month): Path<String>,
) -> Result<Response, ApiError> {
    let first = parse_month(&month)?;
    tokio::task::spawn_blocking(move || {
        let config = state.config();
        let (rows, _) = invoice_data(&month, first, &config, false)?;
        if rows.is_empty() {
            return Err(bad_request("miesiąc — brak godzin B2B", &month));
        }
        let path = pdf::generate_invoice_attachment(&month, &rows, &config).map_err(internal)?;
        let bytes = fs::read(path).map_err(|e| internal(e.to_string()))?;
        Ok((
            [(header::CONTENT_TYPE, "application/pdf")],
            Body::from(bytes),
        )
            .into_response())
    })
    .await
    .map_err(join_error)?
}

#[derive(Deserialize)]
struct ShiftInput {
    from: String,
    to: String,
    shift: Option<String>,
}

async fn put_shift(
    State(state): State<AppState>,
    Json(input): Json<ShiftInput>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let from = parse_date(&input.from)?;
    let to = parse_date(&input.to)?;
    if to < from || (to - from).num_days() > 62 {
        return Err(bad_request("zakres dat", &format!("{from} — {to}")));
    }
    if let Some(shift) = &input.shift {
        schedule::shift_from_str(shift).ok_or_else(|| bad_request("zmianę", shift))?;
    }
    let mutation = state.mutation.clone();
    let _guard = mutation.lock().await;
    tokio::task::spawn_blocking(move || {
        let path = config::config_file_path()
            .ok_or_else(|| internal("Nie znaleziono katalogu konfiguracji".into()))?;
        let mut root: serde_json::Value = if path.exists() {
            serde_json::from_str(&fs::read_to_string(&path).map_err(|e| internal(e.to_string()))?)
                .map_err(|e| internal(format!("config.json: {e}")))?
        } else {
            serde_json::json!({})
        };
        let mut list = root
            .get("shift_overrides")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        // ponytail: overlapping entries are dropped whole (UI operates day/week-wise)
        list.retain(|entry| {
            let get = |k: &str| {
                entry
                    .get(k)
                    .and_then(|v| v.as_str())
                    .and_then(|v| NaiveDate::parse_from_str(v, "%Y-%m-%d").ok())
            };
            match (get("from"), get("to")) {
                (Some(f), Some(t)) => t < from || f > to,
                _ => false,
            }
        });
        if let Some(shift) = input.shift {
            list.push(serde_json::json!({
                "from": from.to_string(),
                "to": to.to_string(),
                "shift": shift,
            }));
        }
        root["shift_overrides"] = serde_json::Value::Array(list);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| internal(e.to_string()))?;
        }
        fs::write(&path, serde_json::to_string_pretty(&root).unwrap())
            .map_err(|e| internal(e.to_string()))?;
        *state.config.write().unwrap() = config::load_config();
        DAY_CACHE.lock().unwrap().clear();
        Ok(Json(serde_json::json!({"ok": true})))
    })
    .await
    .map_err(join_error)?
}

fn internal(message: String) -> ApiError {
    (StatusCode::INTERNAL_SERVER_ERROR, message)
}

fn lock_unavailable() -> ApiError {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "Archiwum jest używane przez inny proces".to_string(),
    )
}

fn join_error(error: tokio::task::JoinError) -> ApiError {
    internal(format!("Błąd zadania serwera: {error}"))
}

#[cfg(test)]
mod tests {
    use super::git_summary_error;

    #[test]
    fn reports_expired_claude_session() {
        assert!(
            git_summary_error("Failed to authenticate: OAuth session expired")
                .starts_with("Sesja Claude wygasła")
        );
    }
}
