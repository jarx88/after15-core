//! Wysyłka na Telegram przez curl. Wspólne dla CLI (`--backup`, `--pdf-telegram`)
//! i pętli serwisu web (dobowa kopia archiwum).

use std::path::{Path, PathBuf};

use crate::{archive, config::Config};

fn data_file(name: &str) -> Option<PathBuf> {
    dirs::data_dir()
        .or_else(|| dirs::home_dir().map(|p| p.join(".local/share")))
        .map(|p| p.join("claude-overtime").join(name))
}

pub fn send_file(path: &Path, caption: &str, config: &Config) -> Result<(), String> {
    if !config.telegram.is_configured() {
        return Err("Telegram nie skonfigurowany w ~/.config/after15/config.json".to_string());
    }
    if !path.exists() {
        return Err(format!("Plik nie istnieje: {}", path.display()));
    }
    let url = format!(
        "https://api.telegram.org/bot{}/sendDocument",
        config.telegram.bot_token
    );
    let result = std::process::Command::new("curl")
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
        .output()
        .map_err(|e| format!("Nie można uruchomić curl: {e} (sudo apt install curl)"))?;
    if !result.status.success() {
        return Err(format!(
            "curl zakończył się błędem: {}",
            String::from_utf8_lossy(&result.stderr)
        ));
    }
    let body = String::from_utf8_lossy(&result.stdout);
    if !body.contains("\"ok\":true") {
        return Err(format!("Telegram API zwrócił błąd: {body}"));
    }
    eprintln!("Wysłano na Telegram: {}", path.display());
    Ok(())
}

/// Kopia daily_summary.json z podpisem (data, liczba dni, rozmiar).
pub fn send_backup(config: &Config) -> Result<(), String> {
    let path = data_file("daily_summary.json").ok_or("Nie można znaleźć daily_summary.json")?;
    if !path.exists() {
        return Err("Nie można znaleźć daily_summary.json".to_string());
    }
    let days_count = archive::load_summary().days.len();
    let file_size = std::fs::metadata(&path)
        .map(|m| format!("{:.1} KB", m.len() as f64 / 1024.0))
        .unwrap_or_else(|_| "?".to_string());
    let date_now = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
    let caption = format!(
        "\u{1F4E6} Backup daily_summary.json\n\u{1F4C5} {}\n\u{1F4CA} Dni: {} | Rozmiar: {}",
        date_now, days_count, file_size
    );
    send_file(&path, &caption, config)
}

/// Raz dziennie, cicho. Znacznik i lock wspólne dla paska Claude i serwisu web.
pub fn auto_daily_backup(config: &Config) {
    use fs2::FileExt;
    if !config.telegram.is_configured() {
        return;
    }
    let (Some(lock_path), Some(marker)) = (
        data_file(".telegram_backup.lock"),
        data_file(".telegram_last_backup"),
    ) else {
        return;
    };
    if let Some(parent) = lock_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(lock) = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&lock_path)
    else {
        return;
    };
    if lock.try_lock_exclusive().is_err() {
        return;
    }
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    if std::fs::read_to_string(&marker).is_ok_and(|c| c.trim() == today) {
        return;
    }
    match send_backup(config) {
        Ok(()) => {
            let _ = std::fs::write(&marker, &today);
        }
        Err(message) => eprintln!("[WARN] Kopia na Telegram nieudana: {message}"),
    }
}
