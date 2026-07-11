use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
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
        .route("/api/rebuild", post(rebuild))
        .route("/api/shift", axum::routing::put(put_shift))
        .route("/api/projects", get(get_projects))
        .route("/api/report/{file}", get(get_pdf))
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
        let listener = tokio::net::TcpListener::bind(address).await.unwrap_or_else(|e| {
            eprintln!("[BŁĄD] Nie można uruchomić serwera na {address}: {e}");
            std::process::exit(1);
        });
        println!("After15 web: http://{address}");
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
}

#[derive(Serialize)]
struct MonthDay {
    date: String,
    hours: f64,
    formatted: String,
    shift: String,
    manual_override: bool,
    source: String,
    projects: Vec<MonthProject>,
}

#[derive(Serialize)]
struct Rates {
    weekday_pln: f64,
    weekend_pln: f64,
}

#[derive(Serialize)]
struct MonthResponse {
    days: Vec<MonthDay>,
    total_hours: f64,
    total_formatted: String,
    days_count: usize,
    rates: Rates,
}

fn month_project_rows(
    projects: Option<&HashMap<String, jsonl::ProjectHours>>,
) -> Vec<MonthProject> {
    let mut rows: Vec<_> = projects
        .into_iter()
        .flat_map(|projects| projects.iter())
        .map(|(name, hours)| MonthProject {
            name: name.clone(),
            weekday_hours: hours.weekday_hours,
            weekend_hours: hours.weekend_hours,
            regular_hours: hours.regular_hours,
        })
        .collect();
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
        let archived_projects = summary_projects(&summary);
        let live = if first.year() == today().year() && first.month() == today().month() {
            Some(cached_compute_day(today(), &config))
        } else {
            None
        };
        let mut days = Vec::new();
        let mut date = first;
        while date.month() == first.month() {
            let key = date.to_string();
            let stored = summary.days.get(&key);
            let use_live = date == today() && !stored.is_some_and(|day| day.manual_override);
            let (hours, projects) = if use_live {
                let live = live.as_ref().unwrap();
                (live.hours, Some(&live.projects))
            } else {
                (
                    stored.map(|day| day.hours).unwrap_or(0.0),
                    archived_projects.get(&date),
                )
            };
            days.push(MonthDay {
                date: key,
                hours,
                formatted: archive::format_hm(hours),
                shift: schedule::shift_str(config.effective_shift(date)).to_string(),
                manual_override: stored.is_some_and(|day| day.manual_override),
                source: if stored.is_some_and(|day| day.manual_override) {
                    "ręczne"
                } else if use_live {
                    "jsonl"
                } else {
                    "archiwum"
                }
                .to_string(),
                projects: month_project_rows(projects),
            });
            date += Duration::days(1);
        }
        let total_hours = days.iter().map(|day| day.hours).sum();
        Ok(Json(MonthResponse {
            days_count: days.iter().filter(|day| day.hours > 0.0).count(),
            total_hours,
            total_formatted: archive::format_hm(total_hours),
            rates: Rates {
                weekday_pln: config.overtime_rate_weekday(),
                weekend_pln: config.overtime_rate_weekend(),
            },
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
        shift: schedule::shift_str(config.effective_shift(date)).to_string(),
        shift_overridden: config.shift_override(date).is_some(),
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
    mutate_day(state, date, move |summary, _| {
        let key = date.to_string();
        let entry = summary.days.entry(key).or_insert_with(|| {
            archive::day_entry(date, 0.0, None, false)
        });
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
            archive::day_entry(date, computed.hours, Some(&computed.projects), false),
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
                archive::day_entry(date, computed.hours, Some(&computed.projects), false),
            );
        }
        summary.days.get_mut(&key).unwrap().manual_override = true;
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

async fn rebuild(State(state): State<AppState>) -> Result<Json<crate::RebuildStats>, ApiError> {
    let mutation = state.mutation.clone();
    let _guard = mutation.lock().await;
    tokio::task::spawn_blocking(move || {
        let _archive_lock = archive::try_lock_archive().ok_or_else(lock_unavailable)?;
        crate::rebuild_archive(&state.config(), false)
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
}

#[derive(Serialize)]
struct ProjectsResponse {
    mode: String,
    projects: Vec<ProjectResponseRow>,
    total_hours: f64,
    total_formatted: String,
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
        let totals = crate::calculate_project_totals(&summary_projects(&summary), &state.config(), full);
        let values: Vec<_> = totals
            .into_iter()
            .map(|project| {
                let hours = project.hours.weekday_hours
                    + project.hours.weekend_hours
                    + if full { project.hours.regular_hours } else { 0.0 };
                (project.name, hours)
            })
            .collect();
        let total_hours: f64 = values.iter().map(|(_, hours)| hours).sum();
        Ok(Json(ProjectsResponse {
            mode,
            projects: values
                .into_iter()
                .map(|(name, hours)| ProjectResponseRow {
                    name,
                    hours,
                    formatted: archive::format_hm(hours),
                    share_pct: if total_hours == 0.0 {
                        0.0
                    } else {
                        hours / total_hours * 100.0
                    },
                })
                .collect(),
            total_hours,
            total_formatted: archive::format_hm(total_hours),
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
