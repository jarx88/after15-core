use after15::{archive, web};
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use serde_json::{Value, json};
use std::fs;
use tower::ServiceExt;

async fn call(app: &axum::Router, method: &str, path: &str, body: Value) -> Value {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(if body.is_null() {
            Body::empty()
        } else {
            Body::from(body.to_string())
        })
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(
        status.is_success(),
        "{status}: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn excluding_segment_keeps_counting_later_work() {
    let root = std::env::temp_dir().join(format!("after15-exclusions-{}", std::process::id()));
    let data = root.join("data");
    let config = root.join("config/after15");
    let source = root.join(".claude/projects/-home-test-Programowanie-demo");
    fs::create_dir_all(&config).unwrap();
    fs::create_dir_all(&source).unwrap();
    unsafe {
        std::env::set_var("HOME", &root);
        std::env::set_var("XDG_DATA_HOME", &data);
        std::env::set_var("XDG_CONFIG_HOME", root.join("config"));
    }
    let date = chrono::Utc::now()
        .with_timezone(&chrono_tz::Europe::Warsaw)
        .date_naive()
        .to_string();
    let day_url = format!("/api/day/{date}");
    fs::write(
        config.join("config.json"),
        json!({"work_window_overrides":[{"date":date,"start":"00:00","end":"00:01"}]}).to_string(),
    )
    .unwrap();
    let file = source.join("session.jsonl");
    let records = |times: &[&str]| {
        times
            .iter()
            .map(|time| {
                json!({
                    "type":"assistant", "timestamp":format!("{date}T{time}:00Z")
                })
                .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    fs::write(&file, records(&["14:00", "14:30", "15:00"])).unwrap();
    let app = web::router();
    let before = call(&app, "GET", &day_url, Value::Null).await;
    assert_eq!(before["computed_hours"], 1.0);
    let session = &before["sessions"][0];
    let key = session["key"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| {
            format!(
                "{}→{}",
                session["start"].as_str().unwrap(),
                session["end"].as_str().unwrap()
            )
        });
    call(
        &app,
        "PUT",
        &day_url,
        json!({"hours":"0:00","exclude_session":key}),
    )
    .await;
    // The excluded session grows; only the separate later session should count.
    fs::write(
        &file,
        records(&[
            "14:00", "14:30", "15:00", "15:15", "16:00", "16:30", "17:00",
        ]),
    )
    .unwrap();
    call(&app, "POST", "/api/rebuild", Value::Null).await;
    let stored = archive::load_summary_checked().unwrap();
    let day = &stored.days[&date];
    assert_eq!(
        day.hours, 1.0,
        "excluding a segment must not freeze subsequent work"
    );
    assert!(!day.manual_override);
    assert!(!day.excluded_sessions.is_empty());
    let after = call(&app, "GET", &day_url, Value::Null).await;
    assert_eq!(after["computed_hours"], 1.0);
    assert_eq!(
        after["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| s["excluded"] == true)
            .count(),
        1
    );
    let month = call(
        &app,
        "GET",
        &format!("/api/month/{}", &date[..7]),
        Value::Null,
    )
    .await;
    let live = month["days"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["date"] == date)
        .unwrap();
    assert_eq!(live["hours"], 1.0);
    assert_eq!(live["manual_override"], false);
    let project_hours: f64 = day
        .projects
        .as_ref()
        .unwrap()
        .values()
        .map(|p| p.weekday_hours + p.weekend_hours)
        .sum();
    assert_eq!(project_hours, 1.0);
    // Excluding every remaining segment must persist a zero, not leave a stale positive day.
    for session in after["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["excluded"] == false)
    {
        call(
            &app,
            "PUT",
            &day_url,
            json!({"exclude_session":session["key"]}),
        )
        .await;
    }
    call(&app, "POST", "/api/rebuild", Value::Null).await;
    assert_eq!(
        archive::load_summary_checked().unwrap().days[&date].hours,
        0.0
    );
    // A previous manually frozen day can resume without losing its exclusions or note.
    call(
        &app,
        "PUT",
        &format!("{day_url}/note"),
        json!({"note":"keep me"}),
    )
    .await;
    call(&app, "POST", &format!("{day_url}/lock"), Value::Null).await;
    fs::write(
        &file,
        records(&[
            "14:00", "14:30", "15:00", "15:15", "16:00", "16:30", "17:00", "17:15", "18:00",
            "18:15",
        ]),
    )
    .unwrap();
    call(&app, "DELETE", &format!("{day_url}/override"), Value::Null).await;
    call(&app, "POST", "/api/rebuild", Value::Null).await;
    let resumed = archive::load_summary_checked().unwrap();
    assert_eq!(resumed.days[&date].hours, 0.25);
    assert_eq!(resumed.days[&date].excluded_sessions.len(), 2);
    assert_eq!(resumed.days[&date].note.as_deref(), Some("keep me"));
    // Next-day automatic archiving must also update a previously processed excluded day.
    let yesterday = chrono::NaiveDate::parse_from_str(&date, "%F")
        .unwrap()
        .pred_opt()
        .unwrap();
    let mut summary = resumed;
    let mut prior = summary.days[&date].clone();
    prior.excluded_sessions = vec!["16:00→17:00".into()];
    summary.days.insert(yesterday.to_string(), prior);
    archive::save_summary(&summary).unwrap();
    archive::archive_overtime(
        &std::collections::HashMap::from([(yesterday, 1.25)]),
        &std::collections::HashMap::new(),
        &after15::config::load_config(),
        false,
    );
    let archived = archive::load_summary_checked().unwrap();
    assert_eq!(archived.days[&yesterday.to_string()].hours, 1.25);
    assert_eq!(
        archived.days[&yesterday.to_string()].excluded_sessions,
        ["16:00→17:00"]
    );
    fs::remove_dir_all(root).unwrap();
}
