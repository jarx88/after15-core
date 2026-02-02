# AFTER15 - PROJECT KNOWLEDGE BASE

**Generated:** 2026-02-02
**Commit:** f039639
**Branch:** main

## OVERVIEW

Rust CLI tool calculating overtime hours from Claude Code session logs. Parses JSONL session data, applies shift-based overtime rules (21-day rotating schedule), generates terminal/PDF reports with Polish UI.

## STRUCTURE

```
after15-core/
├── src/
│   ├── main.rs        # CLI entry (clap), orchestrates all modules
│   ├── config.rs      # Loads ~/.config/after15/config.json
│   ├── schedule.rs    # Shift types: Regular/Afternoon/Weekend/SaturdayAfternoon
│   ├── overtime.rs    # Core calculation: which hours = overtime
│   ├── jsonl.rs       # Parses Claude Code JSONL, builds sessions
│   ├── report.rs      # Terminal tables (tabled crate), colored output
│   ├── archive.rs     # Persists to ~/.local/share/claude-overtime/
│   └── pdf.rs         # PDF generation (printpdf, Liberation fonts)
├── Cargo.toml         # Dependencies, release profile (LTO+strip)
└── Cargo.lock         # Pinned versions
```

## WHERE TO LOOK

| Task | Location | Notes |
|------|----------|-------|
| Add CLI flag | `main.rs:15-33` | clap derive macro, `Cli` struct |
| Change shift hours | `schedule.rs:68-84` | `get_regular_work_window()` |
| Modify overtime logic | `overtime.rs:50-77` | `calculate_overtime_for_day()` |
| Parse new JSONL fields | `jsonl.rs:24-39` | `JsonlEntry`, `ToolInput` structs |
| Change report format | `report.rs:130-170` | `print_daily_table()` |
| Modify PDF layout | `pdf.rs:33-369` | `generate_pdf()`, constants at top |
| Add config option | `config.rs:4-45` | Add to `SalaryConfig` or `ProjectsConfig` |

## CODE MAP

| Symbol | Type | Location | Role |
|--------|------|----------|------|
| `Cli` | struct | main.rs:18 | CLI argument definition |
| `ShiftType` | enum | schedule.rs:41 | Regular/Afternoon/Weekend/SaturdayAfternoon |
| `Session` | struct | jsonl.rs:12 | Parsed work session with project counts |
| `ProjectHours` | struct | jsonl.rs:61 | weekday_hours + weekend_hours |
| `Config` | struct | config.rs:39 | salary + projects configuration |
| `DayReport` | struct | report.rs:13 | Single day's overtime data |

## CONVENTIONS

### Shift Schedule (Hardcoded)
- **21-day cycle** starting 2025-07-28
- Regular: 6:00-15:00 (Mon-Fri)
- Afternoon: 15:00-21:00 (during afternoon week)
- Saturday (afternoon week): 8:00-14:00
- Weekend: all hours = overtime

### Session Detection
- **30-min gap** = new session (`SESSION_GAP_SECONDS = 1800`)
- **5-min minimum** session duration (`MIN_SESSION_SECONDS = 300`)

### Project Extraction
- Extracts from file paths containing `/Programowanie/`
- Format: `-home-jarx-Programowanie-{project_name}`

### Data Paths
- Config: `~/.config/after15/config.json`
- Archive: `~/.local/share/claude-overtime/daily_summary.json`
- Source: `~/.claude/projects/` and `~/.claude/transcripts/`

## ANTI-PATTERNS

- **NO** mocking file system in tests - pure function tests only
- **NO** changing shift cycle without updating `schedule.rs` constants
- **NO** non-Liberation fonts in PDF (must be installed: `fonts-liberation`)

## UNIQUE STYLES

- **Polish UI**: All user-facing text in Polish (CLI output, PDF, errors)
- **English code**: Variable names, comments, module names in English
- **Warsaw timezone**: All time calculations use `chrono_tz::Europe::Warsaw`
- **Inline tests**: `#[cfg(test)] mod tests` in schedule.rs, overtime.rs, jsonl.rs

## COMMANDS

```bash
# Build
cargo build --release

# Run
./target/release/after15                    # Full report
./target/release/after15 --statusline       # Compact: "icon today/month"
./target/release/after15 --month 2026-01    # Filter by month
./target/release/after15 --explain 2026-01-15  # Debug specific date
./target/release/after15 --pdf              # Generate PDF report
./target/release/after15 --debug            # Verbose output

# Test
cargo test

# Lint (if clippy installed)
cargo clippy -- -D warnings
```

## NOTES

- **Edition 2024 in Cargo.toml** - may need update to "2021" for older rustc
- **No CI/CD** - manual build/test workflow
- **14 unit tests** covering schedule, overtime, jsonl modules
- **report.rs, archive.rs, pdf.rs** have no tests (I/O heavy)
- **Release profile**: aggressive optimization (LTO, strip, opt-level="z")
