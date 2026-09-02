use after15::{archive, config, jsonl, pdf, web};
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
            "2026-07-02": {"hours": 1.0, "formatted": "1:00", "shift": "regular", "processed": true},
            "2025-12-15": {"hours": 5.0, "formatted": "5:00", "shift": "regular", "processed": true,
                "projects": {"Programowanie/zeszloroczny": {"weekday_hours": 5.0, "weekend_hours": 0.0}}}
        },
        "months": {"2025-12": {"total_hours": 5.0, "formatted": "5:00"}, "2026-06": {"total_hours": 5.0, "formatted": "5:00"}, "2026-07": {"total_hours": 3.0, "formatted": "3:00"}}
    })).unwrap()).unwrap();

    // config.json musi istniec zanim router() zawola load_config()
    let config_dir = root.join("config/after15");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(config_dir.join("config.json"), serde_json::to_vec_pretty(&json!({
        "billing": {"b2b_from": "2026-09-02", "git_author_email": "git@jarx.pl"}
    })).unwrap()).unwrap();

    let app = web::router();
    let (status, month) = request(&app, "GET", "/api/month/2026-07", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(month["average_months"], 2);
    assert_eq!(month["average_formatted"], "4:00");

    let (status, trend) = request(&app, "GET", "/api/months", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(trend["months"].as_array().unwrap().len(), 2);
    assert_eq!(trend["months"][0]["month"], "2026-06");
    assert_eq!(trend["months"][1]["formatted"], "3:00");
    assert_eq!(trend["average_formatted"], "4:00");
    assert_eq!(trend["total_formatted"], "8:00");

    let (status, projects) = request(&app, "GET", "/api/projects?mode=overtime", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(projects["total_formatted"], "2:00");
    assert!(!projects.to_string().contains("zeszloroczny"));
    // Rozbicie etat/B2B: bez b2b_from wszystko jest etatowymi nadgodzinami.
    assert_eq!(projects["overtime_hours"], 2.0);
    assert_eq!(projects["b2b_hours"], 0.0);
    assert_eq!(projects["overtime_pln"], projects["earned_pln"]);
    assert_eq!(projects["projects"][0]["b2b_pln"], 0.0);

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
        has_claude: true,
        has_codex: false,
    };
    let clipped = web::clip_session_to_date(&session, NaiveDate::from_ymd_opt(2026, 7, 2).unwrap()).unwrap();
    assert_eq!(clipped.duration_seconds, 5400);

    // --- B2B: etykieta zmiany i stawka dnia w odpowiedzi miesiaca ---
    let cfg = config::Config::default();
    let (status, july) = request(&app, "GET", "/api/month/2026-07", None).await;
    assert_eq!(status, StatusCode::OK);
    let july_first = july["days"].as_array().unwrap()[0].clone();
    assert_eq!(july_first["date"], "2026-07-01");
    // dni sprzed przejscia licza sie po staremu: 2 h w dzien roboczy x stawka nadgodzin
    assert!((july_first["pln"].as_f64().unwrap() - 2.0 * cfg.overtime_rate_weekday()).abs() < 1e-6);
    assert_ne!(july_first["shift"], "b2b");

    let mut summary = archive::load_summary_checked().unwrap();
    for (date, hours) in [("2026-09-01", 2.0), ("2026-09-02", 3.0)] {
        let day = archive::day_entry(
            NaiveDate::parse_from_str(date, "%F").unwrap(), hours, None, false, &cfg);
        summary.days.insert(date.to_string(), archive::DayEntry { manual_override: true, ..day });
    }
    archive::save_summary(&summary).unwrap();

    let (status, sept) = request(&app, "GET", "/api/month/2026-09", None).await;
    assert_eq!(status, StatusCode::OK);
    let days = sept["days"].as_array().unwrap();
    let day_of = |d: &str| days.iter().find(|x| x["date"] == d).unwrap().clone();
    assert_ne!(day_of("2026-09-01")["shift"], "b2b");
    assert_eq!(day_of("2026-09-02")["shift"], "b2b");
    assert_eq!(day_of("2026-09-05")["shift"], "b2b"); // sobota tez jest B2B
    assert_eq!(day_of("2026-09-02")["pln"].as_f64().unwrap(), 3.0 * 140.0);
    assert_eq!(sept["rates"]["b2b_pln"], 140.0);
    // kubelek "bez przypisania" tylko na dniach B2B — stare miesiace wygladaja jak dotad
    assert!(day_of("2026-09-02")["projects"].as_array().unwrap()
        .iter().any(|p| p["name"] == "bez przypisania"));
    assert!(july["days"].as_array().unwrap().iter()
        .find(|d| d["date"] == "2026-07-02").unwrap()["projects"].as_array().unwrap().is_empty());
    assert_eq!(sept["b2b_from"], "2026-09-02");

    // --- R1: dzien B2B z reczna korekta i bez mapy projektow trafia na fakture ---
    let alpha = HashMap::from([(
        "-home-jarek-Programowanie-alpha".to_string(),
        jsonl::ProjectHours { weekday_hours: 4.0, ..Default::default() },
    )]);
    let mut summary = archive::load_summary_checked().unwrap();
    let with_projects = archive::day_entry(
        NaiveDate::parse_from_str("2026-09-03", "%F").unwrap(), 4.0, Some(&alpha), false, &cfg);
    summary.days.insert("2026-09-03".to_string(),
        archive::DayEntry { manual_override: true, ..with_projects });
    archive::save_summary(&summary).unwrap();

    let (status, invoice) = request(&app, "GET", "/api/invoice/2026-09/summaries", None).await;
    assert_eq!(status, StatusCode::OK);
    let rows = invoice["projects"].as_array().unwrap();
    let row = |name: &str| rows.iter().find(|r| r["project"] == name).cloned();
    // 2026-09-02: 3 h recznie, zero projektow — bez kubelka faktura zgubilaby ten dzien
    assert_eq!(row("bez przypisania").unwrap()["hours"], 3.0);
    assert_eq!(row("alpha").unwrap()["hours"], 4.0);
    assert_eq!(invoice["total_amount"].as_f64().unwrap(), 7.0 * 140.0);

    // suma faktury = suma dni B2B z widoku miesiaca
    let (_, sept) = request(&app, "GET", "/api/month/2026-09", None).await;
    let b2b_pln: f64 = sept["days"].as_array().unwrap().iter()
        .filter(|d| d["shift"] == "b2b")
        .map(|d| d["pln"].as_f64().unwrap())
        .sum();
    assert!((b2b_pln - invoice["total_amount"].as_f64().unwrap()).abs() < 1e-6);

    fs::write(data.join("daily_summary.json"), "{broken").unwrap();
    assert!(archive::load_summary_checked().is_err());
    fs::remove_dir_all(root).ok();
}

/// Czysta agregacja zalacznika do faktury: bez gita, bez claude, bez HOME.
#[test]
fn invoice_rows_count_only_b2b_days_and_keep_unassigned_hours() {
    let config: config::Config = serde_json::from_str(
        r#"{"billing":{"b2b_from":"2026-09-02"},"projects":{"tracked_path":"Programowanie","excluded_projects":["gamma"]}}"#,
    ).unwrap();
    let hours = |weekday: f64, weekend: f64| jsonl::ProjectHours {
        weekday_hours: weekday, weekend_hours: weekend, ..Default::default()
    };
    let day = |d: &str| NaiveDate::parse_from_str(d, "%F").unwrap();
    let days = vec![
        // przed przejsciem — nie wchodzi do zalacznika
        (day("2026-09-01"), 2.0, HashMap::from([
            ("-home-jarek-Programowanie-alpha".to_string(), hours(2.0, 0.0)),
        ])),
        (day("2026-09-02"), 5.0, HashMap::from([
            ("-home-jarek-Programowanie-alpha".to_string(), hours(3.0, 0.0)),
            ("-home-jarek-Programowanie-beta".to_string(), hours(1.5, 0.0)),
        ])),
        // reczna korekta bez mapy projektow — godziny musza trafic na fakture
        (day("2026-09-03"), 1.0, HashMap::new()),
        // projekt z excluded_projects wypada z faktury razem ze swoimi godzinami
        (day("2026-09-04"), 9.0, HashMap::from([
            ("-home-jarek-Programowanie-gamma".to_string(), hours(9.0, 0.0)),
        ])),
        (day("2026-09-05"), 2.0, HashMap::from([
            ("-home-jarek-Programowanie-alpha".to_string(), hours(0.0, 2.0)),
        ])),
    ];

    let rows = web::invoice_rows(&days, &config);
    assert_eq!(rows, vec![
        pdf::InvoiceRow { project: "alpha".into(), hours: 5.0, amount: 700.0, summary: None },
        pdf::InvoiceRow { project: "beta".into(), hours: 1.5, amount: 210.0, summary: None },
        // 1.0 z dnia bez projektow + 0.5 nieprzypisane z 2026-09-02 (5.0 - 3.0 - 1.5)
        pdf::InvoiceRow { project: "bez przypisania".into(), hours: 1.5, amount: 210.0, summary: None },
    ]);
}
