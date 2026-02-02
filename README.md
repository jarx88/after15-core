# After15 - Kalkulator Nadgodzin

> Automatyczne liczenie nadgodzin z sesji Claude Code na podstawie harmonogramu zmianowego

## O projekcie

**After15** to narzedzie CLI napisane w Rust, ktore automatycznie analizuje logi sesji Claude Code i oblicza ile godzin przepracowales poza standardowym czasem pracy. Nazwa pochodzi od godziny 15:00 - konca regularnej zmiany.

### Jak to dziala?

```
Logi Claude Code (.jsonl)
         |
         v
   +-------------+
   |  after15    |  <-- Harmonogram zmian (21-dniowy cykl)
   +-------------+
         |
         v
  Raport nadgodzin
  (terminal / PDF)
```

Program:
1. Skanuje pliki JSONL z `~/.claude/projects/` i `~/.claude/transcripts/`
2. Wykrywa sesje pracy (przerwa >30 min = nowa sesja)
3. Przypisuje godziny do projektow na podstawie sciezek plikow
4. Oblicza ktore godziny to nadgodziny wedlug Twojego grafiku
5. Generuje raporty z podzialem na dni, miesiace i projekty

## Instalacja

### Wymagania
- Rust 1.70+ (lub nowszy)
- Czcionki Liberation (`sudo apt install fonts-liberation`)

### Kompilacja

```bash
git clone https://github.com/jarx88/after15-core.git
cd after15-core
cargo build --release
```

Binarke znajdziesz w `./target/release/after15`

### Opcjonalnie: Dodaj do PATH

```bash
sudo cp ./target/release/after15 /usr/local/bin/
```

## Uzycie

### Pelny raport

```bash
after15
```

Wyswietla:
- Szczegoly dzienne z typem zmiany
- Statystyki miesieczne
- Podzial na projekty

### Kompaktowy widok (statusbar)

```bash
after15 --statusline
```

Wynik: `🌙 2:30/45:15` (dzis/miesiac)

Ikony:
- 🏢 - w godzinach pracy
- 🌙 - nadgodziny

### Filtrowanie po miesiacu

```bash
after15 --month 2026-01
```

### Szczegoly konkretnego dnia

```bash
after15 --explain 2026-01-15
```

Pokazuje kazda sesje z tego dnia, projekty i jak obliczono nadgodziny.

### Raport PDF

```bash
after15 --pdf
```

Generuje `~/nadgodziny_styczen_2026.pdf` z profesjonalnym formatowaniem.

### Tryb debug

```bash
after15 --debug
```

Pokazuje szczegoly parsowania plikow i wykrywania sesji.

## Konfiguracja

Utworz plik `~/.config/after15/config.json`:

```json
{
  "projects": {
    "tracked_path": "Programowanie",
    "excluded_projects": ["sandbox", "test-project"]
  }
}
```

### Opcje konfiguracji

| Pole | Opis | Domyslnie |
|------|------|-----------|
| `tracked_path` | Fragment sciezki do projektow | "Programowanie" |
| `excluded_projects` | Projekty do pominiecia | [] |

## System zmian

Program obsluguje 21-dniowy cykl zmianowy:

### Typy zmian

| Zmiana | Godziny pracy | Nadgodziny |
|--------|---------------|------------|
| **Regularna** | 6:00 - 15:00 | przed 6:00, po 15:00 |
| **Popoludniowa** | 15:00 - 21:00 | przed 15:00, po 21:00 |
| **Sobota (popoludniowa)** | 8:00 - 14:00 | przed 8:00, po 14:00 |
| **Weekend** | - | caly dzien |

### Cykl 21-dniowy

```
Tydzien 1: Popoludniowa (pon-pt) + Sobota 8-14
Tydzien 2: Regularna
Tydzien 3: Regularna
[powtorz]
```

Pierwszy cykl zaczyna sie 28.07.2025 - mozesz to zmienic w `src/schedule.rs`.

## Przyklad raportu

```
💰 SUMA_NADGODZIN: 47:30

📋 SZCZEGOLY DZIENNE:

╭──────────────────────┬────────────┬────────────┬──────────────────────╮
│ Data                 │ Nadgodziny │    Typ     │ Okno nadgodzin       │
├──────────────────────┼────────────┼────────────┼──────────────────────┤
│ 🏢 2026-01-13 💾     │    3:45    │  Normalny  │ przed 6:00 i po 15:00│
│ 🏠 2026-01-18 💾     │    5:20    │  Weekend   │ caly dzien           │
│ 🌆 2026-01-20 📄     │    2:15    │ Popoludnie │ przed 15:00 i po 21:0│
╰──────────────────────┴────────────┴────────────┴──────────────────────╯

📁 PROJEKTY - 2026-01 (nadgodzin: 47:30):

╭─────────────────┬────────┬───────┬────────╮
│ Projekt         │  Dzien │  Wknd │  Suma  │
├─────────────────┼────────┼───────┼────────┤
│ farmaster2      │  12:30 │  8:00 │  20:30 │
│ after15-core    │  15:45 │  4:15 │  20:00 │
│ side-project    │   5:00 │  2:00 │   7:00 │
╰─────────────────┴────────┴───────┴────────╯
```

## Struktura projektu

```
after15-core/
├── src/
│   ├── main.rs        # CLI (clap)
│   ├── config.rs      # Ladowanie konfiguracji
│   ├── schedule.rs    # Logika zmian
│   ├── overtime.rs    # Obliczanie nadgodzin
│   ├── jsonl.rs       # Parser logow Claude
│   ├── report.rs      # Raporty terminalowe
│   ├── archive.rs     # Zapis do JSON
│   └── pdf.rs         # Generator PDF
├── Cargo.toml
└── AGENTS.md          # Dokumentacja dla AI
```

## Rozwoj

### Testy

```bash
cargo test
```

14 testow jednostkowych pokrywa logike zmian i obliczen.

### Linting

```bash
cargo clippy -- -D warnings
```

### Format

```bash
cargo fmt
```

## Znane ograniczenia

- Harmonogram zmian jest zahardkodowany w `schedule.rs`
- Wymaga czcionek Liberation do generowania PDF
- Parsuje tylko logi Claude Code (format JSONL)
- Strefa czasowa: Europe/Warsaw (zahardkodowana)

## Licencja

MIT

## Autor

Jaroslaw Hartwich

---

> *"Bo kazda minuta po 15:00 sie liczy"* 🌙
