# Handoff: After15 — webowy UI kalkulatora nadgodzin

## Overview
Webowa nakładka na istniejące CLI **after15** (repo: `jarx88/after15-core`, Rust). UI pokrywa: kalendarz miesiąca z nadgodzinami, widok dnia (drawer z timeline sesji vs okno pracy), ręczną edycję dnia (manual_override), raport miesięczny z PLN, ranking projektów oraz akcję rebuild. Całość po polsku, single-user, localhost/Tailscale — bez logowania.

## About the Design Files
Plik `After15.dc.html` w tym pakiecie to **referencja designu w HTML** — interaktywny prototyp z mockowymi danymi, nie kod produkcyjny. Zadanie: **odtworzyć ten design w docelowym środowisku**. Nie ma jeszcze frontendu — rekomendacja: cienki backend HTTP (Axum) wokół istniejących modułów Rust + lekki frontend (np. SvelteKit/React/vanilla+htmx — do wyboru). Backend NIE duplikuje logiki: czyta przez `archive::load_summary()` / `save_summary()`, `schedule::get_shift_type()`, session-detection z `jsonl.rs`.

## Fidelity
**High-fidelity.** Kolory, typografia, spacing i interakcje są docelowe — odtworzyć 1:1.

## Architektura API (propozycja)
- `GET /api/month/2026-07` → dni miesiąca: `{date, hours, shift, manual_override, projects[]}` + suma
- `GET /api/day/2026-07-11` → sesje (odpowiednik `--explain`): `{start, end, projects:[{name, share}], overtime}` + okno pracy + suma wyliczona/ręczna
- `PUT /api/day/2026-07-11` `{hours}` → ustawia manual_override (walidacja jak `tui/state.rs::parse_hours`: "H:MM" lub dziesiętnie, 0–24, minuty 0–59)
- `DELETE /api/day/.../override` → zdejmuje flagę (dzień przeliczy się z JSONL)
- `POST /api/day/.../lock` → flaga manual_override bez zmiany godzin (odpowiednik klawisza `m` w TUI)
- `POST /api/rebuild` → `archive_overtime()` z poszanowaniem manual_override
- `GET /api/projects?mode=overtime|all` → suma per projekt od 2025-07-28
- `GET /api/report/2026-07.pdf` → istniejący generator `pdf.rs`; CSV do dogenerowania

## Screens / Views

### 1. Topbar (stały, 44px, border-bottom 1px #23272e)
- Logo: `AFTER15` — IBM Plex Mono 600 14px, letter-spacing .06em, kolor akcentu #e8b04b; obok `kalkulator nadgodzin` 11px #565d68
- Zakładki: Kalendarz / Raport / Projekty — przyciski 12.5px, aktywna: tło #23272e + tekst #e6e8eb, nieaktywna: transparent + #8a919c, radius 2px
- Po prawej: `dane: HH:MM` (mono 11px #565d68) + przycisk `↻ Przelicz z JSONL` (1px border #23272e, hover border #3a414c)

### 2. Kalendarz (widok główny)
- Nagłówek: nawigacja ‹ lipiec 2026 › (przyciski 26×26px, 1px border), hint `←/→ miesiąc · Enter otwiera dziś · Esc zamyka`, po prawej „SUMA MIESIĄCA" (label 11.5px uppercase #8a919c) + wartość mono 20px 600 w akcencie + liczba dni
- Siatka 7 kolumn, komórki min-height 86px, separacja 1px #23272e (border-left/top na kontenerze, border-right/bottom na komórkach), tło komórki #131519, hover #171a1f, pusta #0c0e11
- Komórka: numer dnia (mono 12px #8a919c; dziś: kolor akcentu + inset ring 1px akcent; przyszłe dni #3a414c bez kropki), kropka 7×7px koloru zmiany, badge `RĘCZNE` (9px mono, tło akcent, tekst #0e1013) przy manual_override, nadgodziny prawy-dolny róg mono 15px 600 akcent (tylko gdy > 0)
- Wariant „lista": gęsta tabela `Data | Zmiana | Projekty | Źródło | Nadgodz.` (grid 120px 110px 1fr 90px 80px) — tylko dni z nadgodzinami
- Legenda pod siatką: kwadrat 8×8px + nazwa + okno (`Regularna 6–15`, `Popołudniowa 15–21`, `Sobota pop. 8–14`, `Weekend cały dzień`)
- Klawiatura: ←/→ miesiąc, Enter otwiera dziś, Esc zamyka drawer (ignorować gdy focus w input)

### 3. Drawer dnia (460px, border-left 1px, tło #101216, kalendarz zostaje widoczny)
- Header (sticky): data `poniedziałek, 7 lipca 2026` 14px 600; pod spodem kropka zmiany + nazwa + `okno 6:00–15:00` mono #565d68; badge RĘCZNE; przycisk ✕ 24×24px
- **Timeline „Sesje vs okno pracy"** — kluczowy element:
  - Oś auto-przycięta: od min(początek sesji, początek okna)−1h do max+1h, zaokrąglone do pełnych godzin; etykieta zakresu np. `5:00 — 18:00`
  - Kontener: 1px border #23272e, tło #0e1013, padding 8px 0 20px
  - Okno pracy: pionowy pas `rgba(255,255,255,0.035)` z 1px dashed #3a414c po bokach + mikro-label `okno pracy` (9px mono); weekend = brak pasa, label „brak okna — całość to nadgodziny"
  - Ticki godzinowe: linia 1px #1a1d22 + etykieta `H:00` 9px mono #565d68 pod osią; krok 1h (span ≤7h), 2h (≤12h), 3h (>12h)
  - Każda sesja = osobny wiersz (lane) 20px; segmenty 14px wysokości: część **w oknie** tło #2b313a border #3a414c, część **nadgodzinowa** tło = kolor akcentu; title z zakresem czasu; sesja cięta na granicach okna
  - Mini-legenda pod spodem: „w oknie pracy" / „nadgodziny"
- **Lista sesji**: karta per sesja (1px border, tło #0e1013): wiersz `6:31 → 10:57` mono 12.5px + czas trwania + `nadgodziny: H:MM` (akcent gdy >0, #565d68 gdy 0); pod spodem projekty: nazwa + `NN%` + godziny (share × czas sesji)
- **Stopka (sticky bottom, border-top)**: „SUMA DNIA" + wartość mono 22px 600 akcent; przy override dodatkowo `wyliczone: H:MM`; input `H:MM` (80px, focus border = akcent) + `Zapisz ręcznie` (tło akcent, tekst #0e1013) + `Przywróć wyliczone` (tylko przy override) + `Zablokuj wyliczone` (tylko bez override; flaga bez zmiany godzin); pod spodem nota o manual_override 10.5px #565d68

### 4. Raport miesięczny
- Ta sama nawigacja miesiąca; przyciski `Eksport PDF` (akcent) i `CSV` (ghost)
- Tabela dni (grid 110px 130px 1fr 90px 80px): Data | Zmiana (kropka+nazwa) | Projekty (`nazwa (H:MM), …`) | Źródło (`archiwum`/`jsonl`/`ręczne` — ręczne w akcencie) | Nadgodz.; wiersz klikalny → otwiera dzień w kalendarzu; stopka „Suma miesiąca"
- Panel boczny 380px „Podział per projekt" (grid 1fr 56px 56px 56px 72px): Projekt | Dzień | Wknd | Suma | **PLN**; stopka: `Wynagrodzenie netto` + suma PLN w akcencie + linia `71 PLN/h dzień · 95 PLN/h weekend` (stawki z config, nie hardcode)

### 5. Projekty (od początku monitorowania)
- Przełącznik segmentowy `Tylko nadgodziny / Wszystkie godziny`
- Tabela (grid 30px 160px 1fr 90px 70px): # | Projekt | pasek (8px, tło #1a1d22, wypełnienie akcent, szerokość ∝ max) | Godziny mono 600 | Udział %
- Stopka: suma; nota o proporcjonalnym przypisaniu godzin do projektów

### 6. Toast (fixed, prawy-dolny róg)
Tło #131519, border 1px #3a414c + border-left 2px akcent, mono 12.5px. Użycia: wynik rebuild, zapis korekty, błąd formatu, eksporty. Auto-hide 3s.

## Interactions & Behavior
- Klik dnia → drawer; Esc / ✕ zamyka; klik wiersza raportu → kalendarz + drawer tego dnia
- Zapis edycji: walidacja formatu, błąd → toast „Nieprawidłowy format — użyj H:MM"; sukces → manual_override=true, odświeżenie sum (dzień + miesiąc)
- Rebuild: POST, po sukcesie odświeżenie danych + aktualizacja `dane: HH:MM` + toast „…manual_override zachowane"
- Hover: tylko zmiany tła/border (bez animacji); przejścia natychmiastowe, zero animacji dekoracyjnych

## State Management
- `currentMonth (y, m)`, `selectedDay | null`, `activeTab`, `editValue`, `projectsMode ('ot'|'all')`, `toast`, `lastRebuildAt`
- Dane per miesiąc cache'owane po stronie klienta; inwalidacja po zapisie edycji i rebuild

## Design Tokens
- Tła: strona #0e1013, panel #131519, panel-2 #101216, hover #171a1f/#1a1d22, pusta komórka #0c0e11
- Bordery: #23272e (główny), #1a1d22 (wiersze), #3a414c (hover/dashed)
- Tekst: #e6e8eb, muted #8a919c, faint #565d68, disabled #3a414c
- Akcent (nadgodziny, CTA, dziś): **#e8b04b** (alternatywy: #4cc38a, #4cb8c4)
- Kolory zmian: regularna #5286c9, popołudniowa #9a7fd1, sobota pop. #3fa08c, weekend #cf5f5f
- Fonty: **IBM Plex Sans** (UI) + **IBM Plex Mono** (wszystkie liczby, godziny, daty, kody) — Google Fonts, wagi 400/500/600
- Radius: 2px (przyciski, inputy), 1px (kropki, paski); **zero cieni** — tylko 1px bordery
- Rozmiary tekstu: 9–11px (mikro/labelki, uppercase + letter-spacing .08–.1em), 12–13px (treść), 14–16px (nagłówki), 20–22px (sumy)
- Segment timeline: lane 20px / bar 14px; kropka zmiany 7×7px

## Logika domenowa (źródło prawdy: repo)
- Zmiany i cykl 21-dniowy z **dwoma anchorami**: 2025-07-28 oraz 2026-04-20 (valid_from 2026-04-06) — `src/schedule.rs`
- Okna: regular 6–15, afternoon 15–21, saturday_afternoon 8–14, weekend brak
- Parser godzin i semantyka edycji: `src/tui/state.rs` (`parse_hours`, `commit_edit`, `toggle_manual`)
- Format H:MM: `format_hm` w `archive.rs`; strefa Europe/Warsaw

## Assets
Brak assetów graficznych. Fonty z Google Fonts (IBM Plex Sans/Mono). Znaki ‹ › ✕ ↻ jako tekst.

## Files
- `After15.dc.html` — kompletny interaktywny prototyp (wszystkie widoki, mock danych, tweaki: wariant kalendarza siatka/lista, akcent, pokazywanie PLN)
