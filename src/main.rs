use chrono::{Datelike, Local};
use clap::Parser;
use std::collections::HashMap;

use after15::{archive, config, jsonl, overtime, pdf, report, schedule, telegram, tui, web};

use report::format_hm;

#[derive(Parser)]
#[command(name = "after15")]
#[command(about = "Kalkulator nadgodzin z sesji Claude Code i Codex")]
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

    #[arg(
        long,
        help = "Pokaż sumę godzin per projekt od początku bieżącego roku"
    )]
    project_totals: bool,

    #[arg(
        long,
        help = "Z --project-totals: uwzględnij także godziny w ramach godzin pracy"
    )]
    full: bool,

    #[arg(long, help = "Interaktywna edycja dni (TUI)")]
    edit: bool,

    #[arg(long, help = "Uruchom webowy interfejs")]
    serve: bool,

    #[arg(long, default_value = "127.0.0.1:4315", help = "Adres serwera web")]
    bind: String,
}

fn main() {
    let cli = Cli::parse();
    let config = config::load_config();

    if cli.serve {
        web::serve(&cli.bind);
        return;
    }

    if cli.edit {
        tui::run();
        return;
    }

    if cli.rebuild {
        let _lock = archive::lock_archive();
        match after15::rebuild_archive(&config, cli.debug, None) {
            Ok(stats) => println!(
                "Przebudowano archiwum: {} dni z JSONL, {} dni łącznie (manual_override zachowane)",
                stats.updated, stats.total_days
            ),
            Err(e) => {
                eprintln!("[BŁĄD] Nie udało się przebudować archiwum: {}", e);
                std::process::exit(1);
            }
        }
        return;
    }

    if cli.backup {
        exit_on_telegram_error(telegram::send_backup(&config));
        return;
    }

    if cli.statusline {
        let summary = jsonl::load_daily_summary_full(&config, false);
        let mut daily_hours = summary.hours;
        let mut daily_projects = summary.projects;
        let today = Local::now().date_naive();

        if let Some(_daily_lock) = archive::lock_daily_automation() {
            if archive::needs_daily_archive() {
                let recent_data = jsonl::load_recent_overtime(1, &config, false);
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
                archive::archive_overtime(&daily_hours, &daily_projects, &config, false);
                telegram::auto_daily_backup(&config);
                archive::mark_daily_archive_done();
            }
        }

        let today_data = jsonl::load_today_overtime(&config, false);
        for (date, hours) in today_data.hours {
            if date == today {
                daily_hours.insert(date, hours);
            }
        }

        print_statusline(&daily_hours, &config);
        return;
    }

    telegram::auto_daily_backup(&config);

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

    let summary = jsonl::load_daily_summary_full(&config, cli.debug);
    let mut daily_hours = summary.hours;
    let mut daily_projects = summary.projects;

    let today = Local::now().date_naive();
    let recent_data = jsonl::load_recent_overtime(1, &config, cli.debug);

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

    archive::archive_overtime(&daily_hours, &daily_projects, &config, cli.debug);

    if cli.project_totals {
        print_project_totals(&daily_projects, &config, cli.full);
        return;
    }

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
                exit_on_telegram_error(telegram::send_file(&path, &caption, &config));
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

fn print_project_totals(
    daily_projects: &HashMap<chrono::NaiveDate, HashMap<String, jsonl::ProjectHours>>,
    config: &config::Config,
    full: bool,
) {
    use colored::*;

    let totals = after15::calculate_project_totals(daily_projects, config, full);

    if totals.is_empty() {
        println!("{}", "Brak danych projektowych w archiwum.".red());
        return;
    }

    let global_first = totals.iter().map(|p| p.first_seen).min();
    let global_last = totals.iter().map(|p| p.last_seen).max();

    println!();
    let header_title = if full {
        "[SUMA GODZIN PER PROJEKT — pełny kontekst (nadgodziny + godziny pracy)]"
    } else {
        "[SUMA GODZIN PER PROJEKT — od początku bieżącego roku]"
    };
    println!("{}", header_title.cyan().bold());
    if let (Some(f), Some(l)) = (global_first, global_last) {
        println!("Zakres danych: {} → {}", f, l);
    }
    if full {
        println!(
            "{}",
            "Uwaga: \"W pracy\" pochodzi z retention JSONL (~365 dni); starsze dni mają 0:00."
                .dimmed()
                .to_string()
        );
    }
    println!();

    if full {
        println!(
            "{:<28} {:>8} {:>8} {:>8} {:>8} {:>10} {:>12} {:>12}",
            "Projekt", "Dzień", "Weekend", "Nadg.", "W pracy", "PLN", "Pierwszy", "Ostatni"
        );
        println!("{}", "─".repeat(102));
    } else {
        println!(
            "{:<28} {:>8} {:>8} {:>8} {:>10} {:>12} {:>12}",
            "Projekt", "Dzień", "Weekend", "Suma", "PLN", "Pierwszy", "Ostatni"
        );
        println!("{}", "─".repeat(92));
    }

    let mut total_weekday = 0.0;
    let mut total_weekend = 0.0;
    let mut total_regular = 0.0;
    let mut total_pln = 0.0;

    for project in &totals {
        let name = &project.name;
        let hours = &project.hours;
        let day_h = hours.weekday_hours;
        let wk_h = hours.weekend_hours;
        let reg_h = hours.regular_hours;
        let overtime_sum = day_h + wk_h;
        let pln = project.amount_pln;
        total_weekday += day_h;
        total_weekend += wk_h;
        total_regular += reg_h;
        total_pln += pln;

        let first = project.first_seen.to_string();
        let last = project.last_seen.to_string();

        if full {
            println!(
                "{:<28} {:>8} {:>8} {:>8} {:>8} {:>10} {:>12} {:>12}",
                truncate_str(name, 28),
                report::format_hm(day_h),
                report::format_hm(wk_h),
                report::format_hm(overtime_sum),
                report::format_hm(reg_h),
                format!("{:.0}", pln),
                first,
                last,
            );
        } else {
            println!(
                "{:<28} {:>8} {:>8} {:>8} {:>10} {:>12} {:>12}",
                truncate_str(name, 28),
                report::format_hm(day_h),
                report::format_hm(wk_h),
                report::format_hm(overtime_sum),
                format!("{:.0}", pln),
                first,
                last,
            );
        }
    }

    if full {
        println!("{}", "─".repeat(102));
        let overtime_total = total_weekday + total_weekend;
        let grand_total = overtime_total + total_regular;
        println!(
            "{:<28} {:>8} {:>8} {:>8} {:>8} {:>10}",
            "SUMA".bold(),
            report::format_hm(total_weekday),
            report::format_hm(total_weekend),
            report::format_hm(overtime_total),
            report::format_hm(total_regular),
            format!("{:.0}", total_pln).yellow().bold().to_string(),
        );
        println!(
            "{}",
            format!(
                "Łącznie czasu nad projektami: {}",
                report::format_hm(grand_total)
            )
            .yellow()
            .bold()
        );
    } else {
        println!("{}", "─".repeat(92));
        let grand_total = total_weekday + total_weekend;
        println!(
            "{:<28} {:>8} {:>8} {:>8} {:>10}",
            "SUMA".bold(),
            report::format_hm(total_weekday),
            report::format_hm(total_weekend),
            report::format_hm(grand_total).yellow().bold().to_string(),
            format!("{:.0}", total_pln).yellow().bold().to_string(),
        );
    }
    println!();
}

fn print_statusline(daily: &HashMap<chrono::NaiveDate, f64>, config: &config::Config) {
    let today = Local::now().date_naive();
    let today_hours = daily.get(&today).copied().unwrap_or(0.0);

    let month_hours: f64 = daily
        .iter()
        .filter(|(d, _)| d.year() == today.year() && d.month() == today.month())
        .map(|(_, h)| h)
        .sum();

    let now = Local::now();
    let icon = if schedule::is_overtime_hour(now, config.effective_work_window(today)) {
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

    let shift_name = if cfg.is_b2b(date) {
        "B2B"
    } else {
        match cfg.effective_shift(date) {
            schedule::ShiftType::Regular => "REGULARNA",
            schedule::ShiftType::Afternoon => "POPOŁUDNIOWA",
            schedule::ShiftType::Weekend => "WEEKEND",
            schedule::ShiftType::SaturdayAfternoon => "SOBOTA (zmiana popołudniowa)",
        }
    };

    let override_window = cfg.work_window_override(date);
    let window = cfg.effective_work_window(date);
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
    if override_window.is_some() {
        println!("Wyjątek: {}", "okno pracy nadpisane z configu".yellow());
    }
    println!("Okno pracy: {}", window_desc);
    println!();

    let sessions = jsonl::load_sessions_for_date(date, &cfg, debug);

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

        let overtime_result = overtime::calculate_session_overtime(session, date, &cfg, false);
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

/// CLI: błąd wysyłki kończy program z kodem 1.
fn exit_on_telegram_error(result: Result<(), String>) {
    if let Err(message) = result {
        eprintln!("[BŁĄD] {message}");
        std::process::exit(1);
    }
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
                );
            }
        };

    let month_projects: HashMap<chrono::NaiveDate, HashMap<String, jsonl::ProjectHours>> =
        filtered_dates
            .iter()
            .filter_map(|d| daily_projects.get(d).map(|p| (*d, p.clone())))
            .collect();
    let sorted = after15::calculate_project_totals(&month_projects, config, false);

    let mut rows: Vec<String> = Vec::new();
    rows.push(format!(
        "{:20} {:>5} {:>5} {:>5} {:>6}",
        "Projekt", "D", "Wk", "S", "PLN"
    ));
    rows.push("-".repeat(44));

    let mut total_hours = 0.0;
    let mut total_pln = 0.0;

    for project in sorted.iter().take(10) {
        let day_h = project.hours.weekday_hours;
        let wk_h = project.hours.weekend_hours;
        let sum = day_h + wk_h;
        if sum < 0.01 {
            continue;
        }
        let pln = project.amount_pln;
        total_hours += sum;
        total_pln += pln;
        rows.push(format!(
            "{:20} {:>5} {:>5} {:>5} {:>6}",
            truncate_str(&project.name, 20),
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
