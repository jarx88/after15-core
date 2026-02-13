mod archive;
mod config;
mod jsonl;
mod overtime;
mod pdf;
mod report;
mod schedule;

use chrono::{Datelike, Local};
use clap::Parser;
use std::collections::HashMap;

use report::format_hm;

#[derive(Parser)]
#[command(name = "after15")]
#[command(about = "Overtime calculator for Claude Code sessions")]
struct Cli {
    #[arg(long, help = "Show compact statusline (today/month)")]
    statusline: bool,

    #[arg(long, help = "Filter by month (YYYY-MM)")]
    month: Option<String>,

    #[arg(long, help = "Explain specific date")]
    explain: Option<String>,

    #[arg(long, help = "Generate PDF report")]
    pdf: bool,

    #[arg(long, help = "Send PDF report via Telegram")]
    pdf_telegram: bool,

    #[arg(long, help = "Debug output")]
    debug: bool,

    #[arg(long, help = "Rebuild archive from JSONL files")]
    rebuild: bool,

    #[arg(long, help = "Send daily_summary.json backup via Telegram")]
    backup: bool,
}

fn main() {
    let cli = Cli::parse();
    let config = config::load_config();

    if cli.rebuild {
        rebuild_archive(cli.debug);
        return;
    }

    if cli.backup {
        send_telegram_backup(&config);
        return;
    }

    if cli.statusline {
        let summary = jsonl::load_daily_summary_full(false);
        let mut daily_hours = summary.hours;
        let mut daily_projects = summary.projects;
        let today = Local::now().date_naive();

        if needs_daily_archive() {
            let recent_data = jsonl::load_recent_overtime(1, false);
            for (date, hours) in recent_data.hours {
                if date != today && !daily_hours.contains_key(&date) {
                    daily_hours.insert(date, hours);
                }
            }
            for (date, projects) in recent_data.projects {
                if date != today && !daily_projects.contains_key(&date) {
                    daily_projects.insert(date, projects);
                }
            }
            archive::archive_overtime(&daily_hours, &daily_projects, false);
            auto_telegram_backup(&config);
            mark_daily_archive_done();
        }

        let today_data = jsonl::load_today_overtime(false);
        for (date, hours) in today_data.hours {
            if date == today {
                daily_hours.insert(date, hours);
            }
        }

        print_statusline(&daily_hours);
        return;
    }

    auto_telegram_backup(&config);

    if let Some(explain_date_str) = &cli.explain {
        match chrono::NaiveDate::parse_from_str(explain_date_str, "%Y-%m-%d") {
            Ok(explain_date) => {
                print_explain(explain_date, cli.debug);
                return;
            }
            Err(_) => {
                eprintln!(
                    "[BŁĄD] Nieprawidłowy format daty: {} (użyj YYYY-MM-DD)",
                    explain_date_str
                );
                std::process::exit(1);
            }
        }
    }

    let summary = jsonl::load_daily_summary_full(cli.debug);
    let mut daily_hours = summary.hours;
    let mut daily_projects = summary.projects;

    let today = Local::now().date_naive();
    let recent_data = jsonl::load_recent_overtime(1, cli.debug);

    for (date, hours) in recent_data.hours {
        if date == today || !daily_hours.contains_key(&date) {
            daily_hours.insert(date, hours);
        }
    }
    for (date, projects) in recent_data.projects {
        if date == today || !daily_projects.contains_key(&date) {
            daily_projects.insert(date, projects);
        }
    }

    archive::archive_overtime(&daily_hours, &daily_projects, cli.debug);

    if cli.pdf_telegram {
        match pdf::generate_pdf(&daily_projects, &config, cli.month.as_deref()) {
            Ok(path) => {
                let (month_label, table) =
                    build_telegram_month_table(&daily_projects, &config, cli.month.as_deref());
                let message = format!(
                    "📄 Raport PDF nadgodzin\n📅 {}\n<pre>{}</pre>",
                    month_label,
                    escape_html(&table)
                );
                send_telegram_message(&message, &config);
                let caption = format!("📄 Raport PDF nadgodzin\n📅 {}", month_label);
                send_telegram_file(&path, &caption, &config, true);
            }
            Err(e) => {
                eprintln!("[BLAD] {}", e);
                std::process::exit(1);
            }
        }
        return;
    }

    if cli.pdf {
        match pdf::generate_pdf(&daily_projects, &config, cli.month.as_deref()) {
            Ok(path) => println!("PDF wygenerowany: {}", path.display()),
            Err(e) => {
                eprintln!("[BLAD] {}", e);
                std::process::exit(1);
            }
        }
    } else {
        report::print_full_report(&daily_hours, &daily_projects, &config, cli.month.as_deref());
    }
}

fn rebuild_archive(debug: bool) {
    let _lock = archive::lock_archive();
    let fresh = jsonl::load_all_overtime(debug);
    let mut archive = archive::load_summary();

    let pre_rebuild_days = archive.days.len();
    if pre_rebuild_days > 0 {
        match archive::force_backup() {
            Ok(path) => eprintln!("[INFO] Backup przed rebuild: {}", path.display()),
            Err(e) => {
                eprintln!("[BŁĄD] Nie udało się utworzyć backupu: {}", e);
                std::process::exit(1);
            }
        }
    }

    archive.version = 2;

    let round2 = |v: f64| (v * 100.0).round() / 100.0;

    let mut overwritten_days = 0usize;
    for (date, hours) in &fresh.hours {
        let date_str = date.format("%Y-%m-%d").to_string();
        let shift_type = schedule::get_shift_type(*date);
        let projects_entry = fresh.projects.get(date).map(|projs| {
            projs
                .iter()
                .map(|(name, hours)| {
                    (
                        name.clone(),
                        archive::ProjectHoursEntry {
                            weekday_hours: round2(hours.weekday_hours),
                            weekend_hours: round2(hours.weekend_hours),
                        },
                    )
                })
                .collect()
        });

        let entry = archive::DayEntry {
            hours: round2(*hours),
            formatted: archive::format_hm(*hours),
            shift: match shift_type {
                schedule::ShiftType::Regular => "regular".to_string(),
                schedule::ShiftType::Afternoon => "afternoon".to_string(),
                schedule::ShiftType::Weekend => "weekend".to_string(),
                schedule::ShiftType::SaturdayAfternoon => "saturday_afternoon".to_string(),
            },
            processed: true,
            projects: projects_entry,
        };

        archive.days.insert(date_str, entry);
        overwritten_days += 1;
    }

    let mut monthly_totals: HashMap<String, f64> = HashMap::new();
    for (date_str, entry) in &archive.days {
        if let Ok(date) = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
            let month_key = format!("{}-{:02}", date.year(), date.month());
            *monthly_totals.entry(month_key).or_insert(0.0) += entry.hours;
        }
    }

    archive.months.clear();
    for (month, total) in monthly_totals {
        archive.months.insert(
            month,
            archive::MonthEntry {
                total_hours: total,
                formatted: archive::format_hm(total),
            },
        );
    }

    let preserved_days = archive.days.len().saturating_sub(overwritten_days);
    match archive::save_summary(&archive) {
        Ok(()) => println!(
            "Przebudowano archiwum: {} dni z JSONL, {} dni zachowanych z archiwum",
            overwritten_days, preserved_days
        ),
        Err(e) => {
            eprintln!("[BŁĄD] Nie udało się zapisać archiwum: {}", e);
            std::process::exit(1);
        }
    }
}

fn print_statusline(daily: &HashMap<chrono::NaiveDate, f64>) {
    let today = Local::now().date_naive();
    let today_hours = daily.get(&today).copied().unwrap_or(0.0);

    let month_hours: f64 = daily
        .iter()
        .filter(|(d, _)| d.year() == today.year() && d.month() == today.month())
        .map(|(_, h)| h)
        .sum();

    let icon = if schedule::is_overtime_hour(Local::now()) {
        "🌙"
    } else {
        "🏢"
    };

    println!(
        "{} {}/{}",
        icon,
        format_hm(today_hours),
        format_hm(month_hours)
    );
}

fn print_explain(date: chrono::NaiveDate, debug: bool) {
    use chrono_tz::Europe::Warsaw;
    use colored::*;

    let cfg = config::load_config();
    let tracked_path = &cfg.projects.tracked_path;

    let shift_type = schedule::get_shift_type(date);
    let shift_name = match shift_type {
        schedule::ShiftType::Regular => "REGULARNA",
        schedule::ShiftType::Afternoon => "POPOŁUDNIOWA",
        schedule::ShiftType::Weekend => "WEEKEND",
        schedule::ShiftType::SaturdayAfternoon => "SOBOTA (zmiana popołudniowa)",
    };

    let window = schedule::get_regular_work_window(date);
    let window_desc = match &window {
        Some(w) => format!(
            "{}:00-{}:00 = regularne, reszta = nadgodziny",
            w.start.format("%H"),
            w.end.format("%H")
        ),
        None => "cały dzień = nadgodziny".to_string(),
    };

    println!();
    println!("{}", format!("[WYJAŚNIENIE dla {}]", date).cyan().bold());
    println!("Typ zmiany: {}", shift_name.yellow());
    println!("Okno pracy: {}", window_desc);
    println!();

    let sessions = jsonl::load_sessions_for_date(date, debug);

    if sessions.is_empty() {
        println!("{}", "Brak sesji z nadgodzinami dla tego dnia.".red());
        return;
    }

    println!(
        "{}",
        format!("Znaleziono {} sesji:", sessions.len()).green()
    );
    println!();

    let mut total_overtime_secs: f64 = 0.0;

    for (i, session) in sessions.iter().enumerate() {
        let start_local = session
            .start_time
            .and_utc()
            .with_timezone(&Warsaw)
            .naive_local();
        let end_local = session
            .end_time
            .and_utc()
            .with_timezone(&Warsaw)
            .naive_local();

        let overtime_result = overtime::calculate_session_overtime(session, date, false);
        let overtime_hours = overtime_result.get(&date).copied().unwrap_or(0.0);
        let overtime_secs = overtime_hours * 3600.0;
        total_overtime_secs += overtime_secs;

        let duration_mins = session.duration_seconds / 60;
        let overtime_mins = (overtime_secs / 60.0).round() as i64;

        println!(
            "{}. {} → {}",
            i + 1,
            start_local.format("%H:%M:%S").to_string().white(),
            end_local.format("%H:%M:%S").to_string().white()
        );

        let real_projects: Vec<_> = session
            .project_counts
            .iter()
            .filter(|(name, _)| *name != "transcripts")
            .collect();

        let total_real_records: usize = real_projects.iter().map(|(_, c)| *c).sum();

        if real_projects.is_empty() {
            println!("   Projekty: {}", "(brak - tylko transcripts)".dimmed());
        } else {
            println!("   Projekty:");
            let mut sorted_projects: Vec<_> = real_projects.clone();
            sorted_projects.sort_by(|a, b| b.1.cmp(a.1));

            for (proj_name, count) in &sorted_projects {
                let count = **count;
                let display_name = report::normalize_project_name(proj_name, tracked_path);
                let pct = if total_real_records > 0 {
                    (count as f64 / total_real_records as f64 * 100.0).round() as i64
                } else {
                    0
                };
                let proj_overtime_mins = if total_real_records > 0 {
                    (overtime_mins as f64 * count as f64 / total_real_records as f64).round() as i64
                } else {
                    0
                };
                let h = proj_overtime_mins / 60;
                let m = proj_overtime_mins % 60;

                if overtime_mins > 0 {
                    println!(
                        "     • {} ({}%) → {}:{:02} nadgodzin",
                        display_name.cyan(),
                        pct,
                        h,
                        m
                    );
                } else {
                    println!("     • {} ({}%)", display_name.cyan(), pct);
                }
            }
        }

        println!("   Czas trwania: {} min", duration_mins);

        if overtime_mins > 0 {
            let h = overtime_mins / 60;
            let m = overtime_mins % 60;
            println!(
                "   {}",
                format!("Nadgodziny sesji: {}:{:02}", h, m).red().bold()
            );
        } else {
            println!("   Nadgodziny: 0:00 (w oknie regularnym)");
        }
        println!();
    }

    let total_h = (total_overtime_secs / 3600.0).floor() as i64;
    let total_m = ((total_overtime_secs % 3600.0) / 60.0).round() as i64;

    println!("{}", "─".repeat(40));
    println!(
        "{}",
        format!("SUMA NADGODZIN: {}:{:02}", total_h, total_m)
            .yellow()
            .bold()
    );
}

fn needs_daily_archive() -> bool {
    let marker_path = dirs::data_dir()
        .or_else(|| dirs::home_dir().map(|p| p.join(".local/share")))
        .map(|p| p.join("claude-overtime/.statusline_last_archive"));

    let Some(marker) = marker_path else {
        return true;
    };

    if marker.exists() {
        if let Ok(content) = std::fs::read_to_string(&marker) {
            let today = chrono::Local::now().format("%Y-%m-%d").to_string();
            if content.trim() == today {
                return false;
            }
        }
    }
    true
}

fn mark_daily_archive_done() {
    let marker_path = dirs::data_dir()
        .or_else(|| dirs::home_dir().map(|p| p.join(".local/share")))
        .map(|p| p.join("claude-overtime/.statusline_last_archive"));

    if let Some(marker) = marker_path {
        if let Some(parent) = marker.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let _ = std::fs::write(&marker, &today);
    }
}

fn auto_telegram_backup(config: &config::Config) {
    if !config.telegram.is_configured() {
        return;
    }

    let marker_path = dirs::data_dir()
        .or_else(|| dirs::home_dir().map(|p| p.join(".local/share")))
        .map(|p| p.join("claude-overtime/.telegram_last_backup"));

    let Some(marker) = marker_path else { return };

    let today = chrono::Local::now().format("%Y-%m-%d").to_string();

    if marker.exists() {
        if let Ok(content) = std::fs::read_to_string(&marker) {
            if content.trim() == today {
                return;
            }
        }
    }

    if send_telegram_backup_silent(config) {
        if let Some(parent) = marker.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&marker, &today);
    }
}

fn send_telegram_file(
    path: &std::path::Path,
    caption: &str,
    config: &config::Config,
    exit_on_error: bool,
) -> bool {
    use std::process::Command;

    if !config.telegram.is_configured() {
        if exit_on_error {
            eprintln!("[BŁĄD] Telegram nie skonfigurowany w ~/.config/after15/config.json");
            eprintln!(
                "Dodaj sekcję: \"telegram\": {{ \"bot_token\": \"...\", \"chat_id\": \"...\" }}"
            );
            std::process::exit(1);
        }
        return false;
    }

    if !path.exists() {
        if exit_on_error {
            eprintln!("[BŁĄD] Plik nie istnieje: {}", path.display());
            std::process::exit(1);
        }
        return false;
    }

    let url = format!(
        "https://api.telegram.org/bot{}/sendDocument",
        config.telegram.bot_token
    );

    let output = Command::new("curl")
        .args([
            "-s",
            "--connect-timeout",
            "5",
            "-X",
            "POST",
            &url,
            "-F",
            &format!("chat_id={}", config.telegram.chat_id),
            "-F",
            &format!("document=@{}", path.display()),
            "-F",
            &format!("caption={}", caption),
        ])
        .output();

    match output {
        Ok(result) => {
            if result.status.success() {
                let body = String::from_utf8_lossy(&result.stdout);
                if body.contains("\"ok\":true") {
                    eprintln!("Wysłano na Telegram: {}", path.display());
                    return true;
                } else if exit_on_error {
                    eprintln!("[BŁĄD] Telegram API zwrócił błąd: {}", body);
                    std::process::exit(1);
                }
            } else if exit_on_error {
                eprintln!(
                    "[BŁĄD] curl zakończył się błędem: {}",
                    String::from_utf8_lossy(&result.stderr)
                );
                std::process::exit(1);
            }
        }
        Err(e) => {
            if exit_on_error {
                eprintln!("[BŁĄD] Nie można uruchomić curl: {}", e);
                eprintln!("Zainstaluj curl: sudo apt install curl");
                std::process::exit(1);
            }
        }
    }
    false
}

fn send_telegram_message(message: &str, config: &config::Config) {
    use std::process::Command;

    if !config.telegram.is_configured() {
        eprintln!("[BŁĄD] Telegram nie skonfigurowany w ~/.config/after15/config.json");
        eprintln!("Dodaj sekcję: \"telegram\": {{ \"bot_token\": \"...\", \"chat_id\": \"...\" }}");
        std::process::exit(1);
    }

    let url = format!(
        "https://api.telegram.org/bot{}/sendMessage",
        config.telegram.bot_token
    );

    let output = Command::new("curl")
        .args([
            "-s",
            "-X",
            "POST",
            &url,
            "-F",
            &format!("chat_id={}", config.telegram.chat_id),
            "-F",
            "parse_mode=HTML",
            "-F",
            &format!("text={}", message),
        ])
        .output();

    match output {
        Ok(result) => {
            if result.status.success() {
                let body = String::from_utf8_lossy(&result.stdout);
                if !body.contains("\"ok\":true") {
                    eprintln!("[BŁĄD] Telegram API zwrócił błąd: {}", body);
                    std::process::exit(1);
                }
            } else {
                eprintln!(
                    "[BŁĄD] curl zakończył się błędem: {}",
                    String::from_utf8_lossy(&result.stderr)
                );
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("[BŁĄD] Nie można uruchomić curl: {}", e);
            eprintln!("Zainstaluj curl: sudo apt install curl");
            std::process::exit(1);
        }
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn build_telegram_month_table(
    daily_projects: &HashMap<chrono::NaiveDate, HashMap<String, jsonl::ProjectHours>>,
    config: &config::Config,
    month_filter: Option<&str>,
) -> (String, String) {
    let (month_name, year, filtered_dates) =
        match get_month_info_for_telegram(daily_projects, month_filter) {
            Ok(info) => info,
            Err(err) => {
                return (
                    month_filter.unwrap_or("bieżący miesiąc").to_string(),
                    format!("Brak danych: {}", err),
                )
            }
        };

    let tracked_path = &config.projects.tracked_path;
    let hourly_weekday = config.salary.base_monthly_net / config.salary.hours_per_month
        * config.salary.overtime_multiplier_weekday;
    let hourly_weekend = config.salary.base_monthly_net / config.salary.hours_per_month
        * config.salary.overtime_multiplier_weekend;

    let mut totals: HashMap<String, jsonl::ProjectHours> = HashMap::new();
    for date in &filtered_dates {
        if let Some(day_projects) = daily_projects.get(date) {
            for (proj_name, hours) in day_projects {
                let normalized = report::normalize_project_name(proj_name, tracked_path);
                if config.projects.excluded_projects.contains(&normalized) {
                    continue;
                }
                let entry = totals.entry(normalized).or_default();
                entry.weekday_hours += hours.weekday_hours;
                entry.weekend_hours += hours.weekend_hours;
            }
        }
    }

    let mut sorted: Vec<_> = totals.iter().collect();
    sorted.sort_by(|a, b| {
        let total_a = a.1.weekday_hours + a.1.weekend_hours;
        let total_b = b.1.weekday_hours + b.1.weekend_hours;
        total_b.partial_cmp(&total_a).unwrap()
    });

    let mut rows: Vec<String> = Vec::new();
    rows.push(format!(
        "{:20} {:>5} {:>5} {:>5} {:>6}",
        "Projekt", "D", "Wk", "S", "PLN"
    ));
    rows.push("-".repeat(44));

    let mut total_hours = 0.0;
    let mut total_pln = 0.0;

    for (name, hours) in sorted.iter().take(10) {
        let day_h = hours.weekday_hours;
        let wk_h = hours.weekend_hours;
        let sum = day_h + wk_h;
        if sum < 0.01 {
            continue;
        }
        let pln = day_h * hourly_weekday + wk_h * hourly_weekend;
        total_hours += sum;
        total_pln += pln;
        rows.push(format!(
            "{:20} {:>5} {:>5} {:>5} {:>6}",
            truncate_str(name, 20),
            report::format_hm(day_h),
            report::format_hm(wk_h),
            report::format_hm(sum),
            format!("{:.0}", pln)
        ));
    }

    rows.push("-".repeat(44));
    rows.push(format!(
        "{:20} {:>5} {:>5} {:>5} {:>6}",
        "SUMA",
        "",
        "",
        report::format_hm(total_hours),
        format!("{:.0}", total_pln)
    ));

    let label = format!("{} {}", month_name, year);
    (label, rows.join("\n"))
}

fn get_month_info_for_telegram(
    daily_projects: &HashMap<chrono::NaiveDate, HashMap<String, jsonl::ProjectHours>>,
    month_filter: Option<&str>,
) -> Result<(String, i32, Vec<chrono::NaiveDate>), String> {
    let filtered_dates: Vec<chrono::NaiveDate> = if let Some(filter) = month_filter {
        let parts: Vec<&str> = filter.split('-').collect();
        if parts.len() != 2 {
            return Err("Nieprawidłowy format miesiąca (YYYY-MM)".to_string());
        }
        let year: i32 = parts[0].parse().map_err(|_| "Nieprawidłowy rok")?;
        let month: u32 = parts[1].parse().map_err(|_| "Nieprawidłowy miesiąc")?;

        daily_projects
            .keys()
            .filter(|d| d.year() == year && d.month() == month)
            .copied()
            .collect()
    } else {
        let today = chrono::Local::now().date_naive();
        daily_projects
            .keys()
            .filter(|d| d.year() == today.year() && d.month() == today.month())
            .copied()
            .collect()
    };

    if filtered_dates.is_empty() {
        return Err("Brak danych dla wybranego miesiąca".to_string());
    }

    let first_date = filtered_dates.iter().min().unwrap();
    let month_name = match first_date.month() {
        1 => "styczeń",
        2 => "luty",
        3 => "marzec",
        4 => "kwiecień",
        5 => "maj",
        6 => "czerwiec",
        7 => "lipiec",
        8 => "sierpień",
        9 => "wrzesień",
        10 => "październik",
        11 => "listopad",
        12 => "grudzień",
        _ => "?",
    }
    .to_string();

    Ok((month_name, first_date.year(), filtered_dates))
}

fn truncate_str(value: &str, max_len: usize) -> String {
    if value.chars().count() <= max_len {
        value.to_string()
    } else {
        format!("{}...", value.chars().take(max_len - 3).collect::<String>())
    }
}

fn send_telegram_backup_silent(config: &config::Config) -> bool {
    let summary_path = dirs::data_dir()
        .or_else(|| dirs::home_dir().map(|p| p.join(".local/share")))
        .map(|p| p.join("claude-overtime/daily_summary.json"));

    let Some(path) = summary_path else {
        return false;
    };

    if !path.exists() {
        return false;
    }

    let summary = archive::load_summary();
    let days_count = summary.days.len();
    let file_size = std::fs::metadata(&path)
        .map(|m| {
            let kb = m.len() as f64 / 1024.0;
            format!("{:.1} KB", kb)
        })
        .unwrap_or_else(|_| "?".to_string());
    let date_now = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();

    let caption = format!(
        "\u{1F4E6} Backup daily_summary.json\n\u{1F4C5} {}\n\u{1F4CA} Dni: {} | Rozmiar: {}",
        date_now, days_count, file_size
    );

    send_telegram_file(&path, &caption, config, false)
}

fn send_telegram_backup(config: &config::Config) {
    let summary_path = dirs::data_dir()
        .or_else(|| dirs::home_dir().map(|p| p.join(".local/share")))
        .map(|p| p.join("claude-overtime/daily_summary.json"));

    let Some(path) = summary_path else {
        eprintln!("[BŁĄD] Nie można znaleźć daily_summary.json");
        std::process::exit(1);
    };

    let summary = archive::load_summary();
    let days_count = summary.days.len();
    let file_size = std::fs::metadata(&path)
        .map(|m| {
            let kb = m.len() as f64 / 1024.0;
            format!("{:.1} KB", kb)
        })
        .unwrap_or_else(|_| "?".to_string());
    let date_now = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();

    let caption = format!(
        "\u{1F4E6} Backup daily_summary.json\n\u{1F4C5} {}\n\u{1F4CA} Dni: {} | Rozmiar: {}",
        date_now, days_count, file_size
    );

    send_telegram_file(&path, &caption, config, true);
}
