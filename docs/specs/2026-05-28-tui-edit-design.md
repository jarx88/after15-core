# after15 — tryb edycji dni `--edit` (TUI)

Data: 2026-05-28
Status: zaakceptowany (do implementacji)

## Cel

Pełnoekranowy TUI pozwalający ręcznie edytować liczbę godzin w danym dniu.
Każda edycja godzin ustawia flagę `manual_override=true`, dzięki czemu kolejne
przeliczenia (`archive_overtime()` / `--rebuild`) **nie nadpisują** tego dnia.

Mechanizm flagi już istnieje (`DayEntry.manual_override`, `archive.rs:33`; guard w
`archive.rs:303`). Dziś jedyny sposób jej ustawienia to ręczna edycja
`~/.local/share/claude-overtime/daily_summary.json`. Ten feature eksponuje to w UI.

## Zasada nadrzędna

TUI **nie duplikuje** logiki liczenia ani persystencji. Czyta i pisze wyłącznie
przez istniejące `archive::load_summary()` / `archive::save_summary()`, które już
zapewniają: exclusive lock (`fs2`), atomic write (temp + rename), walidację ≤24h/dzień,
14 backupów w `./backups/`. Zero nowego kodu zapisu.

## Wejście

- Nowa flaga `--edit` w strukturze `Cli` (`src/main.rs`). Odpala moduł `tui`.
- Reszta CLI bez zmian (`--statusline`, `--rebuild`, `--explain`, `--pdf`, ...).
- Nowe zależności: `ratatui`, `crossterm`.

## Layout

Jeden miesiąc naraz. Wyświetlane są wszystkie dni kalendarzowe 1..N wybranego
miesiąca; godziny brane z archiwum, a jeśli dnia nie ma w pliku — `0:00` (wiersz
wirtualny).

```
 after15 · edycja dni                 maj 2026
┌────────────┬───────┬───────────┬────┐
│ Data       │ Godz. │ Zmiana    │ ✎  │
├────────────┼───────┼───────────┼────┤
│ 2026-05-26 │  2:30 │ afternoon │    │
│▶2026-05-27 │ [1:15]│ regular   │ ✎  │
│ 2026-05-28 │  0:00 │ regular   │    │
└────────────┴───────┴───────────┴────┘
 ↑↓ ruch · ←→ miesiąc · Enter edytuj · m flaga · s zapis · q wyjście
 Σ miesiąc: 48:47
```

- Kolumny: **Data**, **Godz.** (edytowalne), **Zmiana** (read-only), **✎** (wskaźnik `manual_override`).
- Stopka: suma godzin miesiąca + skróty klawiszowe.

## Klawisze

| Klawisz | Akcja |
|---|---|
| `↑` / `↓` | ruch kursora po dniach |
| `←` / `→` | poprzedni / następny miesiąc |
| `Enter` | edycja godzin bieżącego dnia (pole inline) |
| `Esc` | anuluj edycję inline |
| `m` | przełącz `manual_override` bieżącego dnia (bez zmiany godzin) |
| `s` | zapis na dysk |
| `q` | wyjście (prompt przy niezapisanych zmianach) |

## Edycja godzin

- `Enter` → pole inline w kolumnie Godz.
- Akceptowane formaty: `H:MM` (np. `1:15`) oraz ułamek dziesiętny (np. `1.25`).
- Po zatwierdzeniu:
  - ustaw `hours`,
  - przelicz `formatted` (reużycie istniejącego formatera godzin — zlokalizować
    w `report.rs`/`main.rs`),
  - ustaw `manual_override = true` oraz `processed = true`.
- Walidacja: `0 ≤ h ≤ 24`. Wartość spoza zakresu lub niepoprawny format → komunikat
  błędu w stopce, brak zapisu wartości.

## Flaga `m`

Przełącza `manual_override` bieżącego dnia bez zmiany godzin:
- wyczyszczenie flagi → dzień znów policzy się z JSONL przy następnym runie,
- ustawienie flagi → zablokowanie obecnie policzonej wartości.

(Jedyne odstępstwo od "tylko godziny" — drobne, świadomie zaakceptowane.)

## Edycja dnia spoza archiwum

Ustawienie wartości >0 na wirtualnym wierszu `0:00` tworzy nowy `DayEntry`:
`hours` = wpisana wartość, `shift` wyliczony automatem (`schedule`), `processed=true`,
`manual_override=true`. To naturalna konsekwencja edycji godzin, nie osobny tryb
"dodawania dni".

## Zapis

`s` (lub `q` przy niezapisanych zmianach po potwierdzeniu):
1. przelicz sumy miesięcy — wydzielić helper `recalc_months()` z obecnej inline
   logiki w `archive_overtime()` (`archive.rs:345-362`), współdzielony przez TUI i
   istniejący kod,
2. `archive::save_summary()`,
3. potwierdzenie w stopce.

## Obsługa błędów

- Brak pliku archiwum → pusty stan (jak `load_summary()` dziś).
- Konflikt locka → obsługuje istniejący `save_summary()`.
- Błąd zapisu → komunikat w stopce, **nie** wychodzimy z aplikacji.

## Struktura modułu (izolacja i testowalność)

Nowy katalog `src/tui/`:

- **`state.rs`** — czysty `EditState`: lista wierszy bieżącego miesiąca, kursor,
  bufor edycji, flaga `dirty`. Metody: `move_up/move_down`, `prev_month/next_month`,
  `begin_edit`, `input_char`, `commit_edit() -> Result`, `toggle_manual`,
  `apply_to_summary(&mut DailySummaryFile)`. Brak zależności od terminala →
  w pełni unit-testowalny.
- **`render.rs`** — rysuje widżety ratatui z `&EditState`. Zero logiki biznesowej.
- **`mod.rs`** — setup/teardown terminala (raw mode, alternate screen), pętla
  zdarzeń crossterm, delegacja klawiszy do `EditState`.

### Reużycie istniejącego kodu

- `archive::{load_summary, save_summary, DayEntry, DailySummaryFile}`
- `schedule` — wyliczenie `shift` dla daty
- formater `H:MM` (zlokalizować — pole `formatted` generowane gdzieś w `report.rs`/`main.rs`)
- `recalc_months()` — helper wydzielony z `archive_overtime()`

## Testy jednostkowe

- parser godzin: `1:15`→1.25, `2.5`→2.5, odrzucenie złego formatu i wartości >24/<0,
- generacja `formatted` z `hours`,
- `commit_edit` ustawia `manual_override=true` i `processed=true`,
- `toggle_manual` przełącza flagę bez zmiany godzin,
- `apply_to_summary` + `recalc_months` daje poprawną sumę miesiąca po edycji,
- edycja wirtualnego dnia tworzy `DayEntry` z poprawnym `shift`.

## Poza zakresem (YAGNI)

- edycja breakdownu per-projekt,
- jawne dodawanie/usuwanie dni jako osobny tryb,
- edycja typu zmiany (`shift`) ręcznie,
- zmiana `anchor_date` / `work_window_overrides` z TUI.
