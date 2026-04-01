use chrono::{NaiveDate, NaiveTime};
use serde::{Deserialize, Deserializer};
use std::convert::TryFrom;
use std::fs;

use crate::schedule::WorkWindow;

#[derive(Debug, Deserialize, Clone)]
pub struct SalaryConfig {
    pub base_monthly_net: f64,
    pub hours_per_month: f64,
    pub overtime_multiplier_weekday: f64,
    pub overtime_multiplier_weekend: f64,
}

impl Default for SalaryConfig {
    fn default() -> Self {
        Self {
            base_monthly_net: 8000.0,
            hours_per_month: 168.0,
            overtime_multiplier_weekday: 1.5,
            overtime_multiplier_weekend: 2.0,
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct ProjectsConfig {
    pub tracked_path: String,
    #[serde(default)]
    pub excluded_projects: Vec<String>,
    #[serde(default)]
    pub excluded_sources: Vec<String>,
}

impl Default for ProjectsConfig {
    fn default() -> Self {
        Self {
            tracked_path: "Programowanie".to_string(),
            excluded_projects: vec![],
            excluded_sources: vec![],
        }
    }
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct TelegramConfig {
    #[serde(default)]
    pub bot_token: String,
    #[serde(default)]
    pub chat_id: String,
}

impl TelegramConfig {
    pub fn is_configured(&self) -> bool {
        !self.bot_token.is_empty() && !self.chat_id.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct WorkWindowOverride {
    pub date: NaiveDate,
    pub start: NaiveTime,
    pub end: NaiveTime,
}

#[derive(Debug, Deserialize)]
struct WorkWindowOverrideRaw {
    #[serde(deserialize_with = "deserialize_date")]
    date: NaiveDate,
    #[serde(deserialize_with = "deserialize_time")]
    start: NaiveTime,
    #[serde(deserialize_with = "deserialize_time")]
    end: NaiveTime,
}

impl TryFrom<WorkWindowOverrideRaw> for WorkWindowOverride {
    type Error = String;

    fn try_from(raw: WorkWindowOverrideRaw) -> Result<Self, Self::Error> {
        if raw.start >= raw.end {
            return Err(format!(
                "Nieprawidlowe okno pracy dla {}: start ({}) musi byc wczesniej niz end ({})",
                raw.date,
                raw.start.format("%H:%M"),
                raw.end.format("%H:%M")
            ));
        }

        Ok(Self {
            date: raw.date,
            start: raw.start,
            end: raw.end,
        })
    }
}

impl<'de> Deserialize<'de> for WorkWindowOverride {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = WorkWindowOverrideRaw::deserialize(deserializer)?;
        Self::try_from(raw).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct Config {
    #[serde(default)]
    pub salary: SalaryConfig,
    #[serde(default)]
    pub projects: ProjectsConfig,
    #[serde(default)]
    pub telegram: TelegramConfig,
    #[serde(default)]
    pub work_window_overrides: Vec<WorkWindowOverride>,
}

impl Config {
    pub fn hourly_rate(&self) -> f64 {
        self.salary.base_monthly_net / self.salary.hours_per_month
    }

    pub fn overtime_rate_weekday(&self) -> f64 {
        self.hourly_rate() * self.salary.overtime_multiplier_weekday
    }

    pub fn overtime_rate_weekend(&self) -> f64 {
        self.hourly_rate() * self.salary.overtime_multiplier_weekend
    }

    pub fn is_source_excluded(&self, raw_project_name: &str) -> bool {
        self.projects
            .excluded_sources
            .iter()
            .any(|s| raw_project_name.contains(s.as_str()))
    }

    pub fn work_window_override(&self, date: NaiveDate) -> Option<WorkWindow> {
        self.work_window_overrides
            .iter()
            .find(|entry| entry.date == date)
            .map(|entry| WorkWindow {
                start: entry.start,
                end: entry.end,
            })
    }
}

fn deserialize_date<'de, D>(deserializer: D) -> Result<NaiveDate, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    NaiveDate::parse_from_str(&value, "%Y-%m-%d").map_err(serde::de::Error::custom)
}

fn deserialize_time<'de, D>(deserializer: D) -> Result<NaiveTime, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    NaiveTime::parse_from_str(&value, "%H:%M").map_err(serde::de::Error::custom)
}

pub fn load_config() -> Config {
    let config_path = dirs::config_dir()
        .map(|p| p.join("after15/config.json"))
        .or_else(|| dirs::home_dir().map(|p| p.join(".config/after15/config.json")));

    let Some(path) = config_path else {
        eprintln!("[WARN] Nie znaleziono katalogu konfiguracji, uzywam domyslnych wartosci");
        return Config::default();
    };

    if !path.exists() {
        return Config::default();
    }

    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "[BLAD] Nie mozna odczytac pliku konfiguracji {:?}: {}. \
                 Plik istnieje ale jest nieczytelny — sprawdz uprawnienia!",
                path, e
            );
            std::process::exit(1);
        }
    };

    match serde_json::from_str(&content) {
        Ok(config) => config,
        Err(e) => {
            eprintln!(
                "[BLAD] Plik konfiguracji {:?} jest uszkodzony: {}. \
                 Napraw JSON lub usun plik aby uzyc domyslnych wartosci.",
                path, e
            );
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, NaiveTime};

    #[test]
    fn test_deserializes_work_window_override() {
        let config: Config = serde_json::from_str(
            r#"{
                "work_window_overrides": [
                    {
                        "date": "2026-03-11",
                        "start": "15:00",
                        "end": "21:00"
                    }
                ]
            }"#,
        )
        .unwrap();

        let window = config
            .work_window_override(NaiveDate::from_ymd_opt(2026, 3, 11).unwrap())
            .unwrap();

        assert_eq!(window.start, NaiveTime::from_hms_opt(15, 0, 0).unwrap());
        assert_eq!(window.end, NaiveTime::from_hms_opt(21, 0, 0).unwrap());
    }

    #[test]
    fn test_rejects_invalid_work_window_override() {
        let result = serde_json::from_str::<Config>(
            r#"{
                "work_window_overrides": [
                    {
                        "date": "2026-03-11",
                        "start": "21:00",
                        "end": "15:00"
                    }
                ]
            }"#,
        );

        assert!(result.is_err());
    }
}
