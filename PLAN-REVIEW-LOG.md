# PLAN-REVIEW-LOG

- Started: 2026-07-11
- Round: 1 → REVISE (8 findings) — log: /tmp/codex-round-1.log
- Codex session id: 019f52a5-52e4-7880-bda6-4c6c4cd6d124

## Round 1 → 2 changes
1. rebuild: użycie ścieżki `rebuild_archive` (load_all_overtime, nadpis non-manual), współdzielona funkcja z Result + stats.
2. Mutacje: globalny tokio Mutex + spawn_blocking; flock fail → 503 bez zapisu.
3. Nowy `load_summary_checked() -> Result` — serwer nie umiera na uszkodzonym JSON.
4. DELETE override odtwarza cały wpis (hours/formatted/projects); lock nieobecnego dnia zapisuje wartość z JSONL.
5. Sesje klipowane do dnia kalendarzowego (Warsaw) przed czasem/overtime/timeline.
6. Month API: projekty z podziałem weekday/weekend/regular, stawki PLN z config w odpowiedzi, `source` per dzień.
7. PDF pod globalnym mutexem — brak wyścigu o deterministyczny plik.
8. Exit criteria: konkretna lista testów jednostkowych + pełny smoke wszystkich endpointów.

- Round: 2 → REVISE (7 findings) — log: /tmp/codex-round-2.log

## Round 2 → 3 changes
1. Rebuild: usunięto błędne twierdzenie o pomijaniu dzisiejszego dnia — semantyka 1:1 z CLI.
2. DELETE: wpis nigdy nie usuwany (0:00 zostaje) — invariant save_summary nienaruszony.
3. tokio + feature `sync`; dev-dep `tower` do testów HTTP.
4. Wyścig PDF z CLI --pdf: nazwany zaakceptowanym ryzykiem (istnieje już dziś), poza zakresem.
5. Testy mutacji: izolowany fixture XDG_DATA_HOME/HOME w tmpdir, nigdy produkcyjne archiwum.
6. Test współbieżności: N równoległych PUT in-process → spójny plik.
7. Kontrakt JSON /api/projects + 400 na zły mode.

- Round: 3 → REVISE (3 findings) — log: /tmp/codex-round-3.log

## Round 3 → 4 changes
1. Strikte parsowanie parametrów daty/miesiąca we wszystkich route'ach → 400, testy walidacji ścieżek.
2. Testy mutacji w jednym pliku integracyjnym tests/web.rs (własny proces, env raz, sekwencyjnie).
3. Smoke (w tym PDF) na serwerze z HOME/XDG w tmpdir — realny $HOME nietykany.

- Round: 4 → REVISE (2 findings) — log: /tmp/codex-round-4.log

## Round 4 → 5 changes
1. Minimalny src/lib.rs (moduły + web::router()) — umożliwia testy in-process; main.rs używa lib.
2. Test load_summary_checked przeniesiony do tests/web.rs (izolowany proces).

- Round: 5 → **VERDICT: APPROVED** — log: /tmp/codex-round-5.log
