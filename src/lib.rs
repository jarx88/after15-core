pub mod archive;
pub mod config;
pub mod jsonl;
pub mod overtime;
pub mod pdf;
pub mod report;
pub mod schedule;
pub mod tui;
pub mod web;

use chrono::NaiveDate;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Serialize)]
pub struct RebuildStats {
    pub updated: usize,
    pub total_days: usize,
}

pub fn rebuild_archive(config: &config::Config, debug: bool) -> Result<RebuildStats, String> {
    let fresh = jsonl::load_all_overtime(config, debug);
    let mut summary = archive::load_summary_checked()?;

    if !summary.days.is_empty() {
        archive::force_backup()?;
    }
    summary.version = 2;

    let mut updated = 0;
    for (date, hours) in &fresh.hours {
        let key = date.format("%Y-%m-%d").to_string();
        if summary.days.get(&key).is_some_and(|day| day.manual_override) {
            continue;
        }
        // Notes survive a rebuild even though the entry is rewritten.
        let note = summary.days.get(&key).and_then(|day| day.note.clone());
        let mut entry = archive::day_entry(*date, *hours, fresh.projects.get(date), false, config);
        entry.note = note;
        summary.days.insert(key, entry);
        updated += 1;
    }

    archive::recalc_months(&mut summary);
    archive::save_summary(&summary)?;
    Ok(RebuildStats {
        updated,
        total_days: summary.days.len(),
    })
}

#[derive(Clone)]
pub struct ProjectTotal {
    pub name: String,
    pub hours: jsonl::ProjectHours,
    pub first_seen: NaiveDate,
    pub last_seen: NaiveDate,
    /// Kwota netto liczona dniami: stawka z `day_rate(date)` razy godziny dodatkowe tego dnia.
    pub amount_pln: f64,
    /// Godziny dodatkowe z dni po przejsciu na B2B.
    pub b2b_hours: f64,
}

pub fn calculate_project_totals(
    daily_projects: &HashMap<NaiveDate, HashMap<String, jsonl::ProjectHours>>,
    config: &config::Config,
    full: bool,
) -> Vec<ProjectTotal> {
    let mut totals: HashMap<String, ProjectTotal> = HashMap::new();
    for (date, projects) in daily_projects {
        for (raw_name, hours) in projects {
            let name = report::normalize_project_name(raw_name, &config.projects.tracked_path);
            if config.projects.excluded_projects.contains(&name) {
                continue;
            }
            let value = hours.weekday_hours
                + hours.weekend_hours
                + if full { hours.regular_hours } else { 0.0 };
            if value < 0.0001 {
                continue;
            }
            let total = totals.entry(name.clone()).or_insert_with(|| ProjectTotal {
                name,
                hours: jsonl::ProjectHours::default(),
                first_seen: *date,
                last_seen: *date,
                amount_pln: 0.0,
                b2b_hours: 0.0,
            });
            total.hours.weekday_hours += hours.weekday_hours;
            total.hours.weekend_hours += hours.weekend_hours;
            total.hours.regular_hours += hours.regular_hours;
            let extra = hours.weekday_hours + hours.weekend_hours;
            total.amount_pln += extra * config.day_rate(*date);
            if config.is_b2b(*date) {
                total.b2b_hours += extra;
            }
            total.first_seen = total.first_seen.min(*date);
            total.last_seen = total.last_seen.max(*date);
        }
    }
    let mut totals: Vec<_> = totals.into_values().collect();
    totals.sort_by(|a, b| {
        let value = |p: &ProjectTotal| {
            p.hours.weekday_hours
                + p.hours.weekend_hours
                + if full { p.hours.regular_hours } else { 0.0 }
        };
        value(b).total_cmp(&value(a))
    });
    totals
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn day_projects(
        entries: &[(NaiveDate, f64, f64)],
    ) -> HashMap<NaiveDate, HashMap<String, jsonl::ProjectHours>> {
        let mut out: HashMap<NaiveDate, HashMap<String, jsonl::ProjectHours>> = HashMap::new();
        for (date, weekday, weekend) in entries {
            out.entry(*date).or_default().insert(
                "proj".to_string(),
                jsonl::ProjectHours {
                    weekday_hours: *weekday,
                    weekend_hours: *weekend,
                    ..Default::default()
                },
            );
        }
        out
    }

    #[test]
    fn amount_pln_uses_per_day_rate_across_cutover() {
        let cfg = config::Config {
            billing: config::BillingConfig {
                b2b_from: Some(NaiveDate::from_ymd_opt(2026, 9, 2).unwrap()),
                ..Default::default()
            },
            ..config::Config::default()
        };
        let before = NaiveDate::from_ymd_opt(2026, 9, 1).unwrap();
        let after = NaiveDate::from_ymd_opt(2026, 9, 2).unwrap();
        let data = day_projects(&[(before, 2.0, 0.0), (after, 3.0, 0.0)]);

        let totals = calculate_project_totals(&data, &cfg, false);
        assert_eq!(totals.len(), 1);
        let expected = 2.0 * cfg.overtime_rate_weekday() + 3.0 * 140.0;
        assert!((totals[0].amount_pln - expected).abs() < 1e-6);
        assert!((totals[0].b2b_hours - 3.0).abs() < 1e-9);
    }

    #[test]
    fn amount_pln_before_cutover_matches_old_month_level_math() {
        let cfg = config::Config::default();
        let weekday = NaiveDate::from_ymd_opt(2026, 8, 27).unwrap();
        let weekend = NaiveDate::from_ymd_opt(2026, 8, 29).unwrap();
        let data = day_projects(&[(weekday, 2.0, 0.0), (weekend, 0.0, 4.0)]);

        let totals = calculate_project_totals(&data, &cfg, false);
        let expected = 2.0 * cfg.overtime_rate_weekday() + 4.0 * cfg.overtime_rate_weekend();
        assert!((totals[0].amount_pln - expected).abs() < 1e-6);
        assert_eq!(totals[0].b2b_hours, 0.0);
    }
}
