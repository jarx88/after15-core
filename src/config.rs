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
    /// Repozytoria spoza `tracked_path`, sciezki wzgledem katalogu domowego.
    /// Licza sie tylko do podsumowan git/AI, nie do godzin nadgodzin.
    #[serde(default)]
    pub extra_git_paths: Vec<String>,
    #[serde(default)]
    pub excluded_projects: Vec<String>,
    #[serde(default)]
    pub excluded_sources: Vec<String>,
}

impl Default for ProjectsConfig {
    fn default() -> Self {
        Self {
            tracked_path: "Programowanie".to_string(),
            extra_git_paths: vec![],
            excluded_projects: vec![],
            excluded_sources: vec![],
        }
    }
}

/// B2B regime: from `b2b_from` the shift cycle stops. Mon-Fri `work_start..work_end`
/// is the day job (not billed); everything else, incl. whole weekends, is B2B at
/// a flat `hourly_net`. Days before `b2b_from` keep the old overtime rules untouched.
#[derive(Debug, Deserialize, Clone)]
pub struct BillingConfig {
    #[serde(default, deserialize_with = "deserialize_opt_date")]
    pub b2b_from: Option<NaiveDate>,
    #[serde(default = "default_hourly_net")]
    pub hourly_net: f64,
    #[serde(default)]
    pub git_author_email: String,
    #[serde(default = "default_work_start", deserialize_with = "deserialize_time")]
    pub work_start: NaiveTime,
    #[serde(default = "default_work_end", deserialize_with = "deserialize_time")]
    pub work_end: NaiveTime,
}

fn default_hourly_net() -> f64 {
    140.0
}
fn default_work_start() -> NaiveTime {
    NaiveTime::from_hms_opt(7, 0, 0).unwrap()
}
fn default_work_end() -> NaiveTime {
    NaiveTime::from_hms_opt(15, 0, 0).unwrap()
}

impl Default for BillingConfig {
    fn default() -> Self {
        Self {
            b2b_from: None,
            hourly_net: default_hourly_net(),
            git_author_email: String::new(),
            work_start: default_work_start(),
            work_end: default_work_end(),
        }
    }
}

pub const B2B_SHIFT_LABEL: &str = "b2b";

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

#[derive(Debug, Deserialize, Clone)]
pub struct ShiftOverride {
    #[serde(deserialize_with = "deserialize_date")]
    pub from: NaiveDate,
    #[serde(deserialize_with = "deserialize_date")]
    pub to: NaiveDate,
    pub shift: String,
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
    pub billing: BillingConfig,
    #[serde(default)]
    pub work_window_overrides: Vec<WorkWindowOverride>,
    #[serde(default)]
    pub shift_overrides: Vec<ShiftOverride>,
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

    pub fn is_tracked_source(&self, raw_project_name: &str) -> bool {
        raw_project_name.contains(&self.projects.tracked_path)
    }

    pub fn shift_override(&self, date: NaiveDate) -> Option<crate::schedule::ShiftType> {
        self.shift_overrides
            .iter()
            .find(|entry| entry.from <= date && date <= entry.to)
            .and_then(|entry| crate::schedule::shift_from_str(&entry.shift))
    }

    /// True from `billing.b2b_from` (inclusive) onwards.
    pub fn is_b2b(&self, date: NaiveDate) -> bool {
        self.billing.b2b_from.is_some_and(|from| date >= from)
    }

    pub fn effective_shift(&self, date: NaiveDate) -> crate::schedule::ShiftType {
        if self.is_b2b(date) {
            return if crate::schedule::is_weekend(date) {
                crate::schedule::ShiftType::Weekend
            } else {
                crate::schedule::ShiftType::Regular
            };
        }
        self.shift_override(date)
            .unwrap_or_else(|| crate::schedule::get_shift_type(date))
    }

    pub fn effective_work_window(&self, date: NaiveDate) -> Option<WorkWindow> {
        if let Some(window) = self.work_window_override(date) {
            return Some(window);
        }
        if self.is_b2b(date) {
            return if crate::schedule::is_weekend(date) {
                None
            } else {
                Some(WorkWindow {
                    start: self.billing.work_start,
                    end: self.billing.work_end,
                })
            };
        }
        crate::schedule::window_for_shift(self.effective_shift(date))
    }

    /// Label stored in the archive / shown in UI: "b2b" after the cutover, else the shift name.
    pub fn shift_label(&self, date: NaiveDate) -> String {
        if self.is_b2b(date) {
            B2B_SHIFT_LABEL.to_string()
        } else {
            crate::schedule::shift_str(self.effective_shift(date)).to_string()
        }
    }

    /// PLN/h for extra hours on `date`: flat B2B rate after cutover, old overtime rate before.
    pub fn day_rate(&self, date: NaiveDate) -> f64 {
        if self.is_b2b(date) {
            self.billing.hourly_net
        } else if crate::schedule::is_weekend(date) {
            self.overtime_rate_weekend()
        } else {
            self.overtime_rate_weekday()
        }
    }

    /// Git author filter active for `date` (email), None = no filter.
    pub fn git_author_for(&self, date: NaiveDate) -> Option<&str> {
        (self.is_b2b(date) && !self.billing.git_author_email.is_empty())
            .then_some(self.billing.git_author_email.as_str())
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

fn deserialize_opt_date<'de, D>(deserializer: D) -> Result<Option<NaiveDate>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    value
        .map(|v| NaiveDate::parse_from_str(&v, "%Y-%m-%d").map_err(serde::de::Error::custom))
        .transpose()
}

fn deserialize_time<'de, D>(deserializer: D) -> Result<NaiveTime, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    NaiveTime::parse_from_str(&value, "%H:%M").map_err(serde::de::Error::custom)
}

pub fn config_file_path() -> Option<std::path::PathBuf> {
    dirs::config_dir()
        .map(|p| p.join("after15/config.json"))
        .or_else(|| dirs::home_dir().map(|p| p.join(".config/after15/config.json")))
}

pub fn load_config() -> Config {
    let config_path = config_file_path();

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
    fn b2b_regime_after_cutover() {
        let config: Config = serde_json::from_str(
            r#"{"billing":{"b2b_from":"2026-09-02","git_author_email":"git@jarx.pl"}}"#,
        )
        .unwrap();
        let before = NaiveDate::from_ymd_opt(2026, 9, 1).unwrap();
        let after = NaiveDate::from_ymd_opt(2026, 9, 2).unwrap();
        let sat = NaiveDate::from_ymd_opt(2026, 9, 5).unwrap();

        assert!(!config.is_b2b(before));
        assert!(config.is_b2b(after));
        assert_eq!(config.shift_label(after), "b2b");
        assert_ne!(config.shift_label(before), "b2b");
        let w = config.effective_work_window(after).unwrap();
        assert_eq!(w.start, NaiveTime::from_hms_opt(7, 0, 0).unwrap());
        assert_eq!(w.end, NaiveTime::from_hms_opt(15, 0, 0).unwrap());
        assert!(config.effective_work_window(sat).is_none());
        assert_eq!(config.day_rate(after), 140.0);
        assert_eq!(config.day_rate(sat), 140.0);
        assert_eq!(config.git_author_for(after), Some("git@jarx.pl"));
        assert_eq!(config.git_author_for(before), None);
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
