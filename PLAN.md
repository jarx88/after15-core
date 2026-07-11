# PLAN: Web UI dla after15 (frozen plan)

## Problem
after15 ma tylko CLI/TUI. Potrzebny webowy UI (kalendarz, drawer dnia, raport, projekty) dostępny w sieci Tailscale przez Caddy, odtwarzający 1:1 design z `design_handoff_after15_web_ui/After15.dc.html`.

## Decyzje użytkownika (Stage 1)
- Frontend: **vanilla** — adaptacja prototypu HTML (bez node, bez build stepu).
- Backend: w tym samym binarnym crate, flaga `--serve` (styl CLI to flagi, nie subkomendy).
- Działa jako usługa: dostarczamy plik **systemd user unit** + instrukcję.
- Dostęp: Tailscale przez Caddy → domyślny bind `127.0.0.1:4315`, opcja `--bind <addr:port>`.
- CSV: **nie robimy** (YAGNI). Przycisk CSV w UI pomijamy.

## Rozwiązanie

### Backend (`src/web.rs`, jeden moduł)
Nowe zależności: `axum` (0.8), `tokio` (features: `rt-multi-thread`, `macros`, `sync`). Dev-dependency do testów HTTP: `tower` (`util`, dla `ServiceExt::oneshot`). Nic więcej.
Frontend osadzony w binarce przez `include_str!` (pojedynczy artefakt → prosta usługa systemd).

Endpointy (JSON, wg README handoffu, bez CSV):
- `GET /` → statyczny HTML (embed).
- Walidacja ścieżek: parametry `{YYYY-MM}` / `{YYYY-MM-DD}` parsowane strikte (`NaiveDate::parse_from_str` / format miesiąca) na wejściu każdego handlera; błąd → deterministyczne 400 z komunikatem, nigdy panic ani zapis pod zniekształconym kluczem.
- `GET /api/month/{YYYY-MM}` → dni miesiąca `{date, hours, formatted, shift, manual_override, source, projects:[{name, weekday_hours, weekend_hours, regular_hours}]}` + `{total_hours, total_formatted, days_count, rates:{weekday_pln, weekend_pln}}` (stawki z `config.overtime_rate_weekday/weekend`; PLN liczy frontend — podział weekday/weekend jest w archiwum per projekt). `source` per dzień: `"ręczne"` (manual_override) / `"archiwum"` / `"jsonl"` (dzień dzisiejszy liczony live z sesji). Shift z `schedule::get_shift_type`.
- `GET /api/day/{YYYY-MM-DD}` → sesje z `jsonl::load_sessions_for_date` **przycięte do żądanego dnia kalendarzowego (Europe/Warsaw)** — sesja przechodząca przez północ jest klipowana do 00:00/24:00 przed liczeniem czasu, overtime i pozycji na timeline: `{start, end, duration_s, overtime_h, projects:[{name, share}]}` + okno pracy z `schedule::get_regular_work_window` (+ `config.work_window_override`) + `{computed_hours, manual_override, stored_hours}`.
- `PUT /api/day/{date}` body `{"hours":"H:MM|dziesiętnie"}` → walidacja `tui::state::parse_hours`; semantyka `commit_edit`. Błąd walidacji → 422.
- `DELETE /api/day/{date}/override` → atomowo odtwarza cały wpis dnia z JSONL: hours, formatted, projects, flaga=false; brak sesji → wpis zostaje z 0:00 bez projects (**nigdy nie usuwamy wpisu** — respektuje invariant `save_summary`, który odrzuca plik z mniejszą liczbą dni). `recalc_months` po zmianie.
- `POST /api/day/{date}/lock` → `manual_override=true`. Dzień nieobecny w archiwum (zwłaszcza dzisiejszy): najpierw policz wartość z JSONL i ją zapisz — nie lockujemy 0:00.
- `POST /api/rebuild` → dokładnie ścieżka `rebuild_archive` z main.rs (`jsonl::load_all_overtime` → nadpisanie dni bez manual_override → `recalc_months` → save), **z zachowaniem jej faktycznej semantyki wobec dnia dzisiejszego — bez zmian zachowania względem CLI**. Wyciągnięta z main.rs do współdzielonej funkcji zwracającej `Result<RebuildStats,String>` (`{updated, total_days}`); main.rs i web wołają to samo. Błąd → 500 z komunikatem.
- `GET /api/projects?mode=overtime|all` → logika jak `--project-totals [--full]` (współdzielona funkcja). Kontrakt: `{mode, projects:[{name, hours, formatted, share_pct}], total_hours, total_formatted}`, sortowane malejąco po hours; `share_pct` = hours/total. `mode` inny niż `overtime|all` (lub brak) → 400 z komunikatem.
- `GET /api/report/{YYYY-MM}.pdf` → `pdf::generate_pdf(daily_projects, config, Some(month))`; `daily_projects` odtwarzane z archiwum. Endpoint czyta zwrócony `PathBuf` i streamuje; wykonanie pod globalnym mutexem serwera. Wyścig z równolegle odpalonym `after15 --pdf` z CLI istnieje już dziś (deterministyczna ścieżka pliku) i pozostaje **zaakceptowanym ryzykiem** single-user — poza zakresem.

**Współbieżność i bezpieczeństwo zapisu:**
- Wszystkie operacje mutujące (PUT/DELETE/lock/rebuild/pdf) przechodzą przez **jeden globalny `tokio::sync::Mutex`** w stanie serwera i wykonują się w `spawn_blocking`. Single-user — serializacja jest poprawna i najprostsza. Wewnątrz dodatkowo `lock_archive()` (ochrona przed równoległym CLI/TUI); **jeśli flock się nie uda → 503, bez zapisu** (inaczej niż CLI, które kontynuuje).
- `archive::load_summary()` woła `process::exit(1)` przy błędzie parsowania — web dostaje nowy fallible wariant `load_summary_checked() -> Result<DailySummaryFile, String>` (istniejące `load_summary` deleguje do niego + exit); handler błąd → 500, serwer żyje.
- Wzorzec zapisu jak w TUI: lock → reload → merge → `recalc_months` → `save_summary`.

Refactor minimalny: wyciągnięcie logiki rebuild i project-totals z main.rs do funkcji współdzielonych (bez duplikacji), `load_summary_checked` w archive.rs. `tui::state::parse_hours` już `pub`. **Dodajemy minimalny `src/lib.rs`** (deklaracje istniejących modułów + `pub fn web::router()`), a `main.rs` używa crate-lib — umożliwia testy in-process w `tests/web.rs`; zero zmian logiki.

### Frontend (`src/web/index.html`, jeden plik)
Adaptacja `After15.dc.html`: zachować CSS/markup 1:1 (kolory, fonty IBM Plex z Google Fonts, spacing, timeline, toast), podmienić mock danych na `fetch` do API. Stan wg README: `currentMonth`, `selectedDay`, `activeTab`, `editValue`, `projectsMode`, `toast`, cache per miesiąc z inwalidacją po PUT/DELETE/POST rebuild. Klawiatura: ←/→/Enter/Esc. Przycisk CSV usunięty. `dane: HH:MM` = timestamp ostatniego rebuild/fetch.

### Usługa
`docs/after15-web.service` (systemd user unit: `ExecStart=%h/.cargo/bin/after15 --serve`, `Restart=on-failure`) + 3 linie instrukcji w README.

## Proof command
```
cargo test && cargo build --release
./target/release/after15 --serve &  # smoke: curl endpointów
curl -sf localhost:4315/api/month/2026-07 | head -c 200
curl -sf localhost:4315/ | grep -c AFTER15
```

## Exit criteria
- `cargo test` zielone. Nowe testy (jednostkowe, bez uruchamiania serwera): klipowanie sesji przez północ do dnia; DELETE override odtwarza pełny wpis (hours+projects) i przypadek bez sesji; lock dnia nieobecnego zapisuje wartość z JSONL, nie 0:00; PUT walidacja (422 na zły format); rebuild zachowuje manual_override i nie rusza dni spoza JSONL. Test `load_summary_checked` (Err na uszkodzonym JSON zamiast exit) żyje w `tests/web.rs` — funkcja czyta globalną ścieżkę XDG/HOME, więc tylko izolowany proces integracyjny jest bezpieczny.
- Testy HTTP in-process (`tower::ServiceExt::oneshot`) dla mutacji — **wszystkie w jednym pliku integracyjnym `tests/web.rs`** (osobny proces cargo), który raz na starcie ustawia `HOME`+`XDG_DATA_HOME` na tmpdir z fixture i wykonuje przypadki sekwencyjnie (serial w obrębie pliku) — zero interferencji z testami jednostkowymi w `src/` i z produkcyjnym archiwum. Testy walidacji ścieżek: zły miesiąc/data/pdf-route → 400.
- Test współbieżności: N równoległych PUT na różne dni → wszystkie zapisane, plik spójny.
- Smoke curl na żywym serwerze uruchomionym z `HOME`/`XDG_DATA_HOME` w tmpdir (kopiuje fixture): `/`, month, day, projects (oba mode + zły mode → 400), pdf (200 + `application/pdf`) — nic nie dotyka realnego `$HOME`.
- UI wizualnie zgodny z prototypem (te same tokeny/CSS).
- manual_override zachowany po rebuild (test istniejącego zachowania nie zepsuty).
- Codex nie commituje; commit tylko na życzenie użytkownika.

## Non-goals
CSV, auth/login, HTTPS (robi Caddy), multi-user, websockety/live-refresh, testy E2E przeglądarkowe.
