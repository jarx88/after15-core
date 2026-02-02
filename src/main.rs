mod config;
mod schedule;
mod overtime;
mod jsonl;
mod report;
mod archive;
mod pdf;

use clap::Parser;
use chrono::{Local, Datelike};
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
    
    #[arg(long, help = "Debug output")]
    debug: bool,
}

fn main() {
    let cli = Cli::parse();
    let config = config::load_config();
    
    if let Some(explain_date_str) = &cli.explain {
        match chrono::NaiveDate::parse_from_str(explain_date_str, "%Y-%m-%d") {
            Ok(explain_date) => {
                print_explain(explain_date, cli.debug);
                return;
            }
            Err(_) => {
                eprintln!("[BŁĄD] Nieprawidłowy format daty: {} (użyj YYYY-MM-DD)", explain_date_str);
                std::process::exit(1);
            }
        }
    }
    
    let summary = jsonl::load_daily_summary_full(cli.debug);
    let mut daily_hours = summary.hours;
    let mut daily_projects = summary.projects;
    
    let today = Local::now().date_naive();
    let recent_data = jsonl::load_recent_overtime(7, cli.debug);
    
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
    
    if cli.pdf {
        match pdf::generate_pdf(&daily_projects, &config, cli.month.as_deref()) {
            Ok(path) => println!("PDF wygenerowany: {}", path.display()),
            Err(e) => {
                eprintln!("[BLAD] {}", e);
                std::process::exit(1);
            }
        }
    } else if cli.statusline {
        print_statusline(&daily_hours);
    } else {
        report::print_full_report(&daily_hours, &daily_projects, &config, cli.month.as_deref());
    }
}

fn print_statusline(daily: &HashMap<chrono::NaiveDate, f64>) {
    let today = Local::now().date_naive();
    let today_hours = daily.get(&today).copied().unwrap_or(0.0);
    
    let month_hours: f64 = daily.iter()
        .filter(|(d, _)| d.year() == today.year() && d.month() == today.month())
        .map(|(_, h)| h)
        .sum();
    
    let icon = if schedule::is_overtime_hour(Local::now()) { "🌙" } else { "🏢" };
    
    println!("{} {}/{}", icon, format_hm(today_hours), format_hm(month_hours));
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
        Some(w) => format!("{}:00-{}:00 = regularne, reszta = nadgodziny", 
            w.start.format("%H"), w.end.format("%H")),
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
    
    println!("{}", format!("Znaleziono {} sesji:", sessions.len()).green());
    println!();
    
    let mut total_overtime_secs: f64 = 0.0;
    
    for (i, session) in sessions.iter().enumerate() {
        let start_local = session.start_time.and_utc().with_timezone(&Warsaw).naive_local();
        let end_local = session.end_time.and_utc().with_timezone(&Warsaw).naive_local();
        
        let overtime_result = overtime::calculate_session_overtime(session, date, false);
        let overtime_hours = overtime_result.get(&date).copied().unwrap_or(0.0);
        let overtime_secs = overtime_hours * 3600.0;
        total_overtime_secs += overtime_secs;
        
        let duration_mins = session.duration_seconds / 60;
        let overtime_mins = (overtime_secs / 60.0).round() as i64;
        
        println!("{}. {} → {}", 
            i + 1,
            start_local.format("%H:%M:%S").to_string().white(),
            end_local.format("%H:%M:%S").to_string().white()
        );
        
        let real_projects: Vec<_> = session.project_counts
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
                } else { 0 };
                let proj_overtime_mins = if total_real_records > 0 {
                    (overtime_mins as f64 * count as f64 / total_real_records as f64).round() as i64
                } else { 0 };
                let h = proj_overtime_mins / 60;
                let m = proj_overtime_mins % 60;
                
                if overtime_mins > 0 {
                    println!("     • {} ({}%) → {}:{:02} nadgodzin", 
                        display_name.cyan(), pct, h, m);
                } else {
                    println!("     • {} ({}%)", display_name.cyan(), pct);
                }
            }
        }
        
        println!("   Czas trwania: {} min", duration_mins);
        
        if overtime_mins > 0 {
            let h = overtime_mins / 60;
            let m = overtime_mins % 60;
            println!("   {}", format!("Nadgodziny sesji: {}:{:02}", h, m).red().bold());
        } else {
            println!("   Nadgodziny: 0:00 (w oknie regularnym)");
        }
        println!();
    }
    
    let total_h = (total_overtime_secs / 3600.0).floor() as i64;
    let total_m = ((total_overtime_secs % 3600.0) / 60.0).round() as i64;
    
    println!("{}", "─".repeat(40));
    println!("{}", format!("SUMA NADGODZIN: {}:{:02}", total_h, total_m).yellow().bold());
}
