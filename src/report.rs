use chrono::{Datelike, Local, NaiveDate};
use colored::*;
use std::collections::HashMap;
use tabled::{
    settings::{object::Columns, Alignment, Modify, Style},
    Table, Tabled,
};

use crate::config::Config;
use crate::jsonl::ProjectHours;
use crate::schedule::{get_shift_type, ShiftType};

#[derive(Clone)]
pub struct DayReport {
    pub date: NaiveDate,
    pub hours: f64,
    pub shift_type: ShiftType,
    pub from_daily_summary: bool,
}

pub fn print_full_report(
    daily: &HashMap<NaiveDate, f64>,
    projects: &HashMap<NaiveDate, HashMap<String, ProjectHours>>,
    config: &Config,
    month_filter: Option<&str>,
) {
    let today = Local::now().date_naive();

    let filtered_daily: HashMap<NaiveDate, f64> = if let Some(filter) = month_filter {
        daily
            .iter()
            .filter(|(date, _)| {
                let month_str = format!("{}-{:02}", date.year(), date.month());
                month_str == filter
            })
            .map(|(d, h)| (*d, *h))
            .collect()
    } else {
        daily.clone()
    };

    let mut days: Vec<DayReport> = filtered_daily
        .iter()
        .filter(|(date, hours)| **hours > 0.0 || **date == today)
        .map(|(date, hours)| DayReport {
            date: *date,
            hours: *hours,
            shift_type: get_shift_type(*date),
            from_daily_summary: *date != today,
        })
        .collect();

    days.sort_by_key(|d| d.date);

    let total_hours: f64 = filtered_daily.values().sum();

    if let Some(filter) = month_filter {
        println!(
            "{}",
            format!("💰 NADGODZINY ZA {}: {}", filter, format_hm(total_hours))
                .cyan()
                .bold()
        );
    } else {
        println!(
            "{}",
            format!("💰 SUMA_NADGODZIN: {}", format_hm(total_hours))
                .cyan()
                .bold()
        );
    }
    println!();

    if !days.is_empty() {
        println!("{}", "📋 SZCZEGÓŁY DZIENNE:".cyan().bold());
        println!();
        print_daily_table(&days);
        println!();
    }

    if month_filter.is_none() {
        let current_month = format!("{}-{:02}", today.year(), today.month());
        let current_month_hours: f64 = daily
            .iter()
            .filter(|(d, _)| format!("{}-{:02}", d.year(), d.month()) == current_month)
            .map(|(_, h)| h)
            .sum();

        println!(
            "{}",
            format!(
                "💰 SUMA_NADGODZIN_BIEŻĄCY_MIESIĄC ({}): {}",
                current_month,
                format_hm(current_month_hours)
            )
            .cyan()
            .bold()
        );
        println!();

        print_monthly_stats(daily);
        println!();

        print_summary_stats(daily);
        println!();

        println!("{}", "🔍 ŹRÓDŁA DANYCH:".cyan().bold());
        println!("  💾 Dane z daily_summary (przetworzone)");
        println!("  📄 Dane z plików JSONL (bieżące)");
        println!();
    }

    let filtered_projects: HashMap<NaiveDate, HashMap<String, ProjectHours>> =
        if let Some(filter) = month_filter {
            projects
                .iter()
                .filter(|(date, _)| {
                    let month_str = format!("{}-{:02}", date.year(), date.month());
                    month_str == filter
                })
                .map(|(d, p)| (*d, p.clone()))
                .collect()
        } else {
            projects.clone()
        };

    print_project_tables(&filtered_daily, &filtered_projects, config, month_filter);
}

fn print_daily_table(days: &[DayReport]) {
    #[derive(Tabled)]
    struct DayRow {
        #[tabled(rename = "Data")]
        date: String,
        #[tabled(rename = "Nadgodziny")]
        hours: String,
        #[tabled(rename = "Typ")]
        shift_type: String,
        #[tabled(rename = "Okno nadgodzin")]
        window: String,
    }

    let rows: Vec<DayRow> = days
        .iter()
        .map(|d| {
            let emoji = get_day_emoji(&d.shift_type);
            let source = if d.from_daily_summary { "💾" } else { "📄" };
            let date_str = format!("{} {} {}", emoji, d.date, source);

            let hours_str = format_hm(d.hours);
            let shift_str = shift_type_name(&d.shift_type);
            let window_str = overtime_window(&d.shift_type);

            DayRow {
                date: date_str,
                hours: hours_str,
                shift_type: shift_str,
                window: window_str,
            }
        })
        .collect();

    let table = Table::new(rows)
        .with(Style::rounded())
        .with(Modify::new(Columns::single(1)).with(Alignment::center()))
        .with(Modify::new(Columns::single(2)).with(Alignment::center()))
        .to_string();

    println!("{}", table);
}

fn print_monthly_stats(daily: &HashMap<NaiveDate, f64>) {
    println!("{}", "📊 STATYSTYKI MIESIĘCZNE:".cyan().bold());
    println!();

    let mut monthly: HashMap<String, f64> = HashMap::new();
    for (date, hours) in daily {
        let month_key = format!("{}-{:02}", date.year(), date.month());
        *monthly.entry(month_key).or_insert(0.0) += hours;
    }

    let mut months: Vec<_> = monthly.iter().collect();
    months.sort_by(|(a, _), (b, _)| a.cmp(b));

    for (month, hours) in months {
        let hours_str = format!(
            "{:.0}:{:02}h",
            hours.floor(),
            ((hours.fract() * 60.0).round() as i64)
        );
        let colored = if *hours > 0.0 {
            hours_str.red()
        } else {
            hours_str.green()
        };
        println!("  {}: {}", month, colored);
    }
}

fn print_summary_stats(daily: &HashMap<NaiveDate, f64>) {
    println!("{}", "📈 PODSUMOWANIE:".cyan().bold());

    let days_with_overtime = daily.values().filter(|h| **h > 0.0).count();
    let total_hours: f64 = daily.values().sum();
    let avg_hours = if days_with_overtime > 0 {
        total_hours / days_with_overtime as f64
    } else {
        0.0
    };

    let max_day = daily
        .iter()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .map(|(d, h)| (d, h));

    println!("  📅 Dni z nadgodzinami: {}", days_with_overtime);
    println!("  📈 Średnia dzienna: {}", format_hm(avg_hours));

    if let Some((date, hours)) = max_day {
        println!("  🔥 Największy dzień: {} ({})", date, format_hm(*hours));
    }
}

fn print_project_tables(
    _daily: &HashMap<NaiveDate, f64>,
    projects: &HashMap<NaiveDate, HashMap<String, ProjectHours>>,
    config: &Config,
    month_filter: Option<&str>,
) {
    let mut monthly_projects: HashMap<String, HashMap<String, ProjectHours>> = HashMap::new();
    let mut monthly_totals: HashMap<String, f64> = HashMap::new();

    for (date, day_projects) in projects {
        let month_key = format!("{}-{:02}", date.year(), date.month());
        let month_entry = monthly_projects.entry(month_key.clone()).or_default();

        for (project, hours) in day_projects {
            let normalized = normalize_project_name(project, &config.projects.tracked_path);

            if config.projects.excluded_projects.contains(&normalized) {
                continue;
            }

            let proj_entry = month_entry.entry(normalized).or_insert(ProjectHours {
                weekday_hours: 0.0,
                weekend_hours: 0.0,
            });
            proj_entry.weekday_hours += hours.weekday_hours;
            proj_entry.weekend_hours += hours.weekend_hours;

            let total_hours = hours.weekday_hours + hours.weekend_hours;
            *monthly_totals.entry(month_key.clone()).or_insert(0.0) += total_hours;
        }
    }

    let mut months: Vec<_> = monthly_projects.keys().cloned().collect();
    months.sort();
    months.reverse();

    let hourly_weekday = config.overtime_rate_weekday();
    let hourly_weekend = config.overtime_rate_weekend();

    let months_to_show = if month_filter.is_some() { 1 } else { 3 };
    for month in months.iter().take(months_to_show) {
        let total = monthly_totals.get(month).copied().unwrap_or(0.0);
        if total <= 0.0 {
            continue;
        }

        println!(
            "{}",
            format!("📁 PROJEKTY - {} (nadgodzin: {}):", month, format_hm(total))
                .cyan()
                .bold()
        );
        println!();

        if let Some(month_projects) = monthly_projects.get(month) {
            #[derive(Tabled)]
            struct ProjectRow {
                #[tabled(rename = "Projekt")]
                project: String,
                #[tabled(rename = "Dzień")]
                weekday: String,
                #[tabled(rename = "Wknd")]
                weekend: String,
                #[tabled(rename = "Suma")]
                total: String,
                #[tabled(rename = "PLN")]
                pln: String,
            }

            let mut rows: Vec<ProjectRow> = month_projects
                .iter()
                .map(|(name, hours)| {
                    let total_h = hours.weekday_hours + hours.weekend_hours;
                    let pln = (hours.weekday_hours * hourly_weekday)
                        + (hours.weekend_hours * hourly_weekend);

                    ProjectRow {
                        project: name.clone(),
                        weekday: format_hm(hours.weekday_hours),
                        weekend: format_hm(hours.weekend_hours),
                        total: format_hm(total_h),
                        pln: format!("{:.0} PLN", pln),
                    }
                })
                .collect();

            rows.sort_by(|a, b| a.project.cmp(&b.project));

            let table = Table::new(rows)
                .with(Style::rounded())
                .with(Modify::new(Columns::new(1..=4)).with(Alignment::right()))
                .to_string();

            println!("{}", table);

            let total_pln: f64 = month_projects
                .values()
                .map(|h| (h.weekday_hours * hourly_weekday) + (h.weekend_hours * hourly_weekend))
                .sum();

            println!(
                "  💰 Wynagrodzenie: {:.0} PLN netto ({:.0} PLN/h dzień, {:.0} PLN/h weekend)",
                total_pln, hourly_weekday, hourly_weekend
            );
            println!();
        }
    }
}

pub fn normalize_project_name(raw_name: &str, tracked_path: &str) -> String {
    if raw_name.is_empty() {
        return "Inne".to_string();
    }

    if raw_name.contains(tracked_path) {
        let pattern = format!("-home-jarx-{}-", tracked_path);
        let name = raw_name.replace(&pattern, "");
        let name = name.trim_matches('-');
        if name.is_empty() {
            "Inne".to_string()
        } else {
            name.to_string()
        }
    } else {
        "Inne".to_string()
    }
}

fn get_day_emoji(shift_type: &ShiftType) -> &'static str {
    match shift_type {
        ShiftType::Weekend => "🏠",
        ShiftType::SaturdayAfternoon => "📅",
        ShiftType::Afternoon => "🌆",
        ShiftType::Regular => "🏢",
    }
}

fn shift_type_name(shift_type: &ShiftType) -> String {
    match shift_type {
        ShiftType::Weekend => "Weekend".to_string(),
        ShiftType::SaturdayAfternoon => "Sobota".to_string(),
        ShiftType::Afternoon => "Popołudnie".to_string(),
        ShiftType::Regular => "Normalny".to_string(),
    }
}

fn overtime_window(shift_type: &ShiftType) -> String {
    match shift_type {
        ShiftType::Weekend => "cały dzień".to_string(),
        ShiftType::SaturdayAfternoon => "przed 8:00 i po 14:00".to_string(),
        ShiftType::Afternoon => "przed 15:00 i po 21:00".to_string(),
        ShiftType::Regular => "przed 6:00 i po 15:00".to_string(),
    }
}

pub fn format_hm(hours: f64) -> String {
    let total_minutes = (hours * 60.0).round() as i64;
    let h = total_minutes / 60;
    let m = total_minutes.abs() % 60;
    format!("{}:{:02}", h, m)
}
