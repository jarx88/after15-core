use serde::Deserialize;
use std::fs;

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
}

impl Default for ProjectsConfig {
    fn default() -> Self {
        Self {
            tracked_path: "Programowanie".to_string(),
            excluded_projects: vec![],
        }
    }
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct Config {
    #[serde(default)]
    pub salary: SalaryConfig,
    #[serde(default)]
    pub projects: ProjectsConfig,
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
}

pub fn load_config() -> Config {
    let config_path = dirs::config_dir()
        .map(|p| p.join("after15/config.json"))
        .or_else(|| dirs::home_dir().map(|p| p.join(".config/after15/config.json")));

    if let Some(path) = config_path {
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(config) = serde_json::from_str(&content) {
                    return config;
                }
            }
        }
    }

    Config::default()
}
