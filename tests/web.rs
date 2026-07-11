use after15::{archive, jsonl, web};
use axum::{body::{to_bytes, Body}, http::{Request, StatusCode}};
use chrono::{NaiveDate, NaiveDateTime};
use serde_json::{json, Value};
use std::{collections::HashMap, fs};
use tower::ServiceExt;

async fn request(app: &axum::Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    let body = if let Some(value) = body {
        builder = builder.header("content-type", "application/json");
        Body::from(value.to_string())
    } else {
        Body::empty()
    };
    let response = app.clone().oneshot(builder.body(body).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, value)
}

#[tokio::test]
async fn web_contract_and_mutations_are_isolated() {
    let root = std::env::temp_dir().join(format!("after15-web-test-{}", std::process::id()));
    let data = root.join("data/claude-overtime");
    fs::create_dir_all(&data).unwrap();
    unsafe {
        std::env::set_var("HOME", &root);
        std::env::set_var("XDG_DATA_HOME", root.join("data"));
        std::env::set_var("XDG_CONFIG_HOME", root.join("config"));
    }
    fs::write(data.join("daily_summary.json"), serde_json::to_vec_pretty(&json!({
        "version": 2,
        "days": {
            "2026-07-01": {"hours": 2.0, "formatted": "2:00", "shift": "regular", "processed": true, "manual_override": true,
                "projects": {"Programowanie/demo": {"weekday_hours": 2.0, "weekend_hours": 0.0}}},
            "2026-07-02": {"hours": 1.0, "formatted": "1:00", "shift": "regular", "processed": true}
        },
        "months": {"2026-07": {"total_hours": 3.0, "formatted": "3:00"}}
    })).unwrap()).unwrap();

    let app = web::router();
    for uri in ["/api/month/2026-13", "/api/day/2026-02-30", "/api/report/nope.pdf"] {
        assert_eq!(request(&app, "GET", uri, None).await.0, StatusCode::BAD_REQUEST);
    }
    assert_eq!(request(&app, "PUT", "/api/day/2026-07-03", Some(json!({"hours":"25:00"}))).await.0, StatusCode::UNPROCESSABLE_ENTITY);

    let mut writes = Vec::new();
    for day in 3..=8 {
        let app = app.clone();
        writes.push(tokio::spawn(async move {
            request(&app, "PUT", &format!("/api/day/2026-07-{day:02}"), Some(json!({"hours":"1:30"}))).await.0
        }));
    }
    for write in writes { assert_eq!(write.await.unwrap(), StatusCode::OK); }
    let summary = archive::load_summary_checked().unwrap();
    for day in 3..=8 { assert_eq!(summary.days[&format!("2026-07-{day:02}")].hours, 1.5); }

    assert_eq!(request(&app, "DELETE", "/api/day/2026-07-03/override", None).await.0, StatusCode::OK);
    let restored = archive::load_summary_checked().unwrap();
    let day = &restored.days["2026-07-03"];
    assert_eq!(day.hours, 0.0);
    assert!(!day.manual_override);
    assert!(day.projects.as_ref().is_some_and(|projects| projects.is_empty()));

    assert_eq!(request(&app, "POST", "/api/day/2026-07-09/lock", None).await.0, StatusCode::OK);
    assert!(archive::load_summary_checked().unwrap().days["2026-07-09"].manual_override);

    assert_eq!(request(&app, "POST", "/api/rebuild", None).await.0, StatusCode::OK);
    assert!(archive::load_summary_checked().unwrap().days["2026-07-01"].manual_override);
    assert!(archive::load_summary_checked().unwrap().days.contains_key("2026-07-02"));

    let session = jsonl::Session {
        id: "test".into(), project: "test".into(), project_counts: HashMap::new(),
        start_time: NaiveDateTime::parse_from_str("2026-07-01 21:30:00", "%F %T").unwrap(),
        end_time: NaiveDateTime::parse_from_str("2026-07-01 23:30:00", "%F %T").unwrap(),
        duration_seconds: 7200,
    };
    let clipped = web::clip_session_to_date(&session, NaiveDate::from_ymd_opt(2026, 7, 2).unwrap()).unwrap();
    assert_eq!(clipped.duration_seconds, 5400);

    fs::write(data.join("daily_summary.json"), "{broken").unwrap();
    assert!(archive::load_summary_checked().is_err());
    fs::remove_dir_all(root).ok();
}
