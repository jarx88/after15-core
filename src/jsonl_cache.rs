//! Cache plików JSONL dla panelu web.
//!
//! Zamiast przechodzić całe drzewo sesji i parsować pliki dnia przy każdym
//! żądaniu trzymamy indeks plików (ścieżka, rozmiar, czasy) i sparsowane
//! rekordy per plik. Pliki sesji są tylko dopisywane, więc zmiana pliku to
//! zwykle doczytanie nowych linii od zapamiętanego stanu parsera. Rekordy
//! zapisujemy też na dysk, żeby start serwera nie parsował gigabajtów od nowa.
//!
//! Dzień liczony stąd jest identyczny z `jsonl::load_sessions_for_date`: ten
//! sam zbiór plików (`JsonlIndex::touching`) i te same rekordy (`FileParser`).

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, NaiveDate};

use crate::config::Config;
use crate::jsonl::{self, FileParser, Session, TimestampRecord};

struct JsonlFile {
    path: PathBuf,
    len: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
}

pub struct JsonlIndex {
    files: Vec<JsonlFile>,
    scanned_at: Instant,
    projects_key: u64,
}

impl JsonlIndex {
    fn scan() -> Self {
        // Jak `find_jsonl_files`: plik bez metadanych nie przechodzi filtra dat.
        let files = jsonl::scan_jsonl_files()
            .into_iter()
            .map(|(path, metadata)| JsonlFile {
                len: metadata.len(),
                modified: metadata.modified().ok(),
                created: metadata.created().ok(),
                path,
            })
            .collect();
        JsonlIndex {
            files,
            scanned_at: Instant::now(),
            projects_key: projects_key(),
        }
    }

    /// Te same pliki co `jsonl::files_touching_date(date)`.
    fn touching(&self, date: NaiveDate) -> impl Iterator<Item = &JsonlFile> {
        let (min_modified, max_created) = jsonl::touching_date_bounds(date);
        self.files.iter().filter(move |file| {
            jsonl::passes_date_filter(
                file.modified,
                file.created,
                Some(min_modified),
                Some(max_created),
            )
        })
    }

    /// Odcisk plików dnia: ten sam odcisk = te same sesje.
    pub fn fingerprint(&self, date: NaiveDate) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.projects_key.hash(&mut hasher);
        for file in self.touching(date) {
            file.path.hash(&mut hasher);
            file.len.hash(&mut hasher);
            file.modified.hash(&mut hasher);
        }
        hasher.finish()
    }
}

static INDEX: Mutex<Option<Arc<JsonlIndex>>> = Mutex::new(None);

/// Indeks nie starszy niż `max_age`; starszy skanujemy od nowa (~40 ms).
/// `Duration::ZERO` wymusza skan — tak robią akcje, które mają widzieć stan bieżący.
pub fn index(max_age: Duration) -> Arc<JsonlIndex> {
    let mut current = lock(&INDEX);
    if let Some(index) = current
        .as_ref()
        .filter(|index| index.scanned_at.elapsed() <= max_age)
    {
        return index.clone();
    }
    let index = Arc::new(JsonlIndex::scan());
    *current = Some(index.clone());
    index
}

/// Odcisk katalogów w ~/Programowanie. Od ich istnienia zależy przypisanie
/// worktree t3 do projektu, więc inna lista unieważnia sparsowane rekordy.
fn projects_key() -> u64 {
    let mut names: Vec<_> = dirs::home_dir()
        .and_then(|home| fs::read_dir(home.join(jsonl::PROGRAMOWANIE_DIR)).ok())
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| (entry.file_name(), entry.path().is_dir()))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    let mut hasher = DefaultHasher::new();
    names.hash(&mut hasher);
    hasher.finish()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Rekordy jednego pliku w chwili parsowania.
#[derive(Clone, Default)]
struct Snapshot {
    committed: Arc<Vec<TimestampRecord>>,
    pending: Option<TimestampRecord>,
    hidden: bool,
}

impl Snapshot {
    fn records(&self) -> impl Iterator<Item = &TimestampRecord> {
        let committed: &[TimestampRecord] = if self.hidden { &[] } else { &self.committed };
        committed
            .iter()
            .chain(self.pending.iter().filter(|_| !self.hidden))
    }
}

/// Liczba bajtów sprzed `offset`, którymi sprawdzamy, że plik tylko urósł.
const TAIL_CHECK: usize = 64;

#[derive(Clone)]
struct Parsed {
    /// Rozmiar i mtime z chwili parsowania — zgodne z indeksem = nic nowego.
    len: u64,
    modified: Option<SystemTime>,
    /// Bajty przetworzone do końca ostatniej pełnej linii.
    offset: u64,
    tail_check: Vec<u8>,
    /// Stan po ostatniej pełnej linii.
    parser: FileParser,
    committed: Arc<Vec<TimestampRecord>>,
    /// Niedokończona ostatnia linia (agent właśnie pisze) — liczona jak w
    /// `lines()`, ale na kopii stanu, żeby jej dokończenie trafiło do parsera.
    pending: Option<TimestampRecord>,
    hidden: bool,
}

impl Parsed {
    fn new(path: &Path) -> Self {
        Parsed {
            len: 0,
            modified: None,
            offset: 0,
            tail_check: Vec::new(),
            parser: FileParser::new(path),
            committed: Arc::default(),
            pending: None,
            hidden: false,
        }
    }

    fn matches(&self, file: &JsonlFile) -> bool {
        self.len == file.len && self.modified == file.modified
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            committed: self.committed.clone(),
            pending: self.pending.clone(),
            hidden: self.hidden,
        }
    }
}

/// Bajty tuż przed `offset` są te same co przy poprzednim parsowaniu.
fn same_prefix(file: &mut File, previous: &Parsed) -> bool {
    let start = previous.offset - previous.tail_check.len() as u64;
    let mut bytes = vec![0; previous.tail_check.len()];
    file.seek(SeekFrom::Start(start)).is_ok()
        && file.read_exact(&mut bytes).is_ok()
        && bytes == previous.tail_check
}

fn remember_tail(tail: &mut Vec<u8>, line: &[u8]) {
    tail.extend_from_slice(&line[line.len().saturating_sub(TAIL_CHECK)..]);
    let excess = tail.len().saturating_sub(TAIL_CHECK);
    tail.drain(..excess);
}

/// Parsuje plik: od zapamiętanego stanu, gdy plik tylko urósł, inaczej od zera.
/// Linie dzielimy jak `BufRead::lines()` + `flatten()` w `collect_timestamps_from_file`.
/// Zwraca też liczbę przeczytanych bajtów (0 = plik bez zmian).
fn parse(path: &Path, previous: Option<Parsed>) -> std::io::Result<(Parsed, u64)> {
    let mut file = File::open(path)?;
    let metadata = file.metadata()?;
    let (len, modified) = (metadata.len(), metadata.modified().ok());
    let mut state = match previous {
        Some(previous) if previous.len == len && previous.modified == modified => {
            return Ok((previous, 0));
        }
        // Ten sam rozmiar przy znanym, innym mtime = możliwy zapis w miejscu.
        Some(previous)
            if len >= previous.offset
                && !(len == previous.len && previous.modified.is_some())
                && same_prefix(&mut file, &previous) =>
        {
            previous
        }
        _ => Parsed::new(path),
    };
    file.seek(SeekFrom::Start(state.offset))?;
    let start = state.offset;
    let mut reader = BufReader::new(file);
    let committed = Arc::make_mut(&mut state.committed);
    let mut line = Vec::new();
    let mut read = 0;
    state.pending = None;
    state.hidden = state.parser.is_t3_metadata_session;
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        if line.last() != Some(&b'\n') {
            read = line.len() as u64;
            let mut parser = state.parser.clone();
            state.pending = std::str::from_utf8(&line)
                .ok()
                .and_then(|text| parser.parse_line(text));
            state.hidden = parser.is_t3_metadata_session;
            break;
        }
        state.offset += line.len() as u64;
        remember_tail(&mut state.tail_check, &line);
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if let Ok(text) = std::str::from_utf8(&line)
            && let Some(record) = state.parser.parse_line(text)
        {
            committed.push(record);
        }
        state.hidden = state.parser.is_t3_metadata_session;
    }
    state.len = len;
    state.modified = modified;
    // Co najmniej 1: zmieniony plik bez nowych linii też trzeba zapisać.
    let read = (state.offset - start + read).max(1);
    Ok((state, read))
}

type Slot = Arc<Mutex<Option<Parsed>>>;

struct Store {
    projects_key: u64,
    slots: Mutex<HashMap<PathBuf, Slot>>,
    /// Bajty sparsowane od ostatniego zapisu na dysk (0 = nic do zapisania).
    unsaved: AtomicU64,
}

static STORE: RwLock<Option<Arc<Store>>> = RwLock::new(None);

fn store(projects_key: u64) -> Arc<Store> {
    let current = |store: &Option<Arc<Store>>| {
        store
            .as_ref()
            .filter(|store| store.projects_key == projects_key)
            .cloned()
    };
    if let Some(store) = current(&STORE.read().unwrap_or_else(|e| e.into_inner())) {
        return store;
    }
    let mut guard = STORE.write().unwrap_or_else(|e| e.into_inner());
    if let Some(store) = current(&guard) {
        return store;
    }
    let store = Arc::new(Store {
        projects_key,
        slots: Mutex::new(load_persisted(projects_key).unwrap_or_default()),
        unsaved: AtomicU64::new(0),
    });
    *guard = Some(store.clone());
    store
}

/// Aktualne rekordy plików; zmienione pliki doczytuje równolegle, największe pierwsze.
fn snapshots(store: &Store, files: &[&JsonlFile]) -> Vec<Snapshot> {
    let slots: Vec<Slot> = {
        let mut map = lock(&store.slots);
        files
            .iter()
            .map(|file| match map.get(&file.path) {
                Some(slot) => slot.clone(),
                None => map.entry(file.path.clone()).or_default().clone(),
            })
            .collect()
    };
    let mut out: Vec<Option<Snapshot>> = slots
        .iter()
        .zip(files)
        .map(|(slot, file)| {
            lock(slot)
                .as_ref()
                .filter(|parsed| parsed.matches(file))
                .map(Parsed::snapshot)
        })
        .collect();
    let mut todo: Vec<usize> = (0..files.len()).filter(|&i| out[i].is_none()).collect();
    if todo.is_empty() {
        return out.into_iter().flatten().collect();
    }
    todo.sort_by_key(|&i| std::cmp::Reverse(files[i].len));
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(todo.len());
    let done: Vec<(usize, Snapshot, u64)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    while let Some(&i) = todo.get(next.fetch_add(1, Ordering::Relaxed)) {
                        let (snapshot, read) = refresh(&slots[i], files[i]);
                        done.push((i, snapshot, read));
                    }
                    done
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap_or_default())
            .collect()
    });
    for (i, snapshot, read) in done {
        store.unsaved.fetch_add(read, Ordering::Relaxed);
        out[i] = Some(snapshot);
    }
    out.into_iter().map(Option::unwrap_or_default).collect()
}

fn refresh(slot: &Slot, file: &JsonlFile) -> (Snapshot, u64) {
    let mut parsed = lock(slot);
    if let Some(current) = parsed.as_ref().filter(|parsed| parsed.matches(file)) {
        return (current.snapshot(), 0);
    }
    match parse(&file.path, parsed.take()) {
        Ok((state, read)) => {
            let snapshot = state.snapshot();
            *parsed = Some(state);
            (snapshot, read)
        }
        // Błąd odczytu: wynik jak z pełnego parsowania, bez zapamiętywania.
        Err(_) => (
            Snapshot {
                committed: Arc::new(jsonl::collect_timestamps_from_file(&file.path)),
                ..Snapshot::default()
            },
            0,
        ),
    }
}

/// Sesje dnia jak w `jsonl::load_sessions_for_date`, z rekordów z cache.
pub fn sessions_for_date(index: &JsonlIndex, date: NaiveDate, config: &Config) -> Vec<Session> {
    let store = store(index.projects_key);
    let files: Vec<&JsonlFile> = index.touching(date).collect();
    let snapshots = snapshots(&store, &files);
    let mut records = Vec::new();
    // Rekordy pliku dzielą Arc projektu — filtr liczymy raz na serię.
    let mut last: Option<(&Arc<str>, bool)> = None;
    for record in snapshots.iter().flat_map(Snapshot::records) {
        let counted = match last {
            Some((project, counted)) if Arc::ptr_eq(project, &record.project) => counted,
            _ => {
                let counted = jsonl::is_counted_source(config, &record.project);
                last = Some((&record.project, counted));
                counted
            }
        };
        if counted {
            records.push(record);
        }
    }
    maybe_save(&store);
    jsonl::sessions_touching_date(date, records)
}

/// Parsuje z góry pliki wszystkich `dates` naraz — plik wspólny dla wielu dni idzie raz.
pub fn prefetch(index: &JsonlIndex, dates: &[NaiveDate]) {
    let mut seen = HashSet::new();
    let files: Vec<&JsonlFile> = dates
        .iter()
        .flat_map(|date| index.touching(*date))
        .filter(|file| seen.insert(std::ptr::from_ref(*file)))
        .collect();
    let store = store(index.projects_key);
    snapshots(&store, &files);
    maybe_save(&store);
}

/// Czyści cache w pamięci (indeks i rekordy); `forget_disk` usuwa też zapis na dysku.
pub fn clear(forget_disk: bool) {
    *lock(&INDEX) = None;
    *STORE.write().unwrap_or_else(|e| e.into_inner()) = None;
    if forget_disk && let Some(path) = cache_path() {
        let _ = fs::remove_file(path);
    }
}

// --- zapis na dysk ---

const MAGIC: &[u8; 8] = b"A15JSONL";
const FORMAT: u32 = 1;
/// Rzadko, bo plik ma kilka MB; po restarcie zmiany od zapisu i tak doczytujemy przyrostowo.
const SAVE_INTERVAL: Duration = Duration::from_secs(300);
const BIG_PARSE_BYTES: u64 = 64 << 20;
const BIG_PARSE_INTERVAL: Duration = Duration::from_secs(10);

static SAVING: AtomicBool = AtomicBool::new(false);
static SAVER: AtomicBool = AtomicBool::new(false);
static LAST_SAVE: Mutex<Option<Instant>> = Mutex::new(None);

fn cache_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|dir| dir.join("after15/jsonl-records.bin"))
}

/// Zmiana kodu parsera unieważnia zapis — rekordy mogłyby wyjść inne.
fn parser_key() -> u64 {
    static KEY: LazyLock<u64> = LazyLock::new(|| {
        let mut hasher = DefaultHasher::new();
        include_str!("jsonl.rs").hash(&mut hasher);
        include_str!("jsonl_cache.rs").hash(&mut hasher);
        hasher.finish()
    });
    *KEY
}

fn maybe_save(store: &Arc<Store>) {
    if store.unsaved.load(Ordering::Relaxed) == 0 || SAVER.swap(true, Ordering::Acquire) {
        return;
    }
    let store = store.clone();
    std::thread::spawn(move || {
        loop {
            let unsaved = store.unsaved.load(Ordering::Relaxed);
            let current = STORE
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &store));
            if unsaved == 0 || !current {
                break;
            }
            // Po dużym parsowaniu (start bez zapisu, nowy miesiąc) zapisujemy
            // szybko, żeby restart go nie powtarzał; dopisane linie mogą poczekać.
            let interval = if unsaved >= BIG_PARSE_BYTES {
                BIG_PARSE_INTERVAL
            } else {
                SAVE_INTERVAL
            };
            let since = lock(&LAST_SAVE).map_or(Duration::MAX, |at| at.elapsed());
            if since >= interval {
                save(&store);
                break;
            }
            std::thread::sleep((interval - since).min(Duration::from_secs(1)));
        }
        SAVER.store(false, Ordering::Release);
    });
}

/// Zapis bieżącego stanu, jeśli coś się zmieniło (pętla godzinowa serwera).
pub fn save_if_dirty() {
    let store = STORE.read().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(store) = store.filter(|store| store.unsaved.load(Ordering::Relaxed) > 0) {
        save(&store);
    }
}

fn save(store: &Arc<Store>) {
    if SAVING.swap(true, Ordering::Acquire) {
        return;
    }
    *lock(&LAST_SAVE) = Some(Instant::now());
    let unsaved = store.unsaved.swap(0, Ordering::Relaxed);
    if let Err(error) = write_persisted(store) {
        eprintln!("[WARN] Nie zapisano cache JSONL: {error}");
        store.unsaved.fetch_add(unsaved, Ordering::Relaxed);
    }
    SAVING.store(false, Ordering::Release);
}

struct Writer {
    bytes: Vec<u8>,
    strings: HashMap<Arc<str>, u32>,
}

impl Writer {
    fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }
    fn u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }
    fn u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }
    fn blob(&mut self, value: &[u8]) {
        self.u32(value.len() as u32);
        self.bytes.extend_from_slice(value);
    }
    fn string(&mut self, value: &Arc<str>) -> u32 {
        let next = self.strings.len() as u32;
        *self.strings.entry(value.clone()).or_insert(next)
    }
    fn time(&mut self, value: Option<SystemTime>) {
        match value.and_then(|time| time.duration_since(UNIX_EPOCH).ok()) {
            Some(since) => {
                self.u8(1);
                self.u64(since.as_secs());
                self.u32(since.subsec_nanos());
            }
            None => self.u8(0),
        }
    }
}

fn write_file(out: &mut Writer, path: &Path, parsed: &Parsed) {
    out.blob(path.as_os_str().as_bytes());
    // Niedokończonej linii nie zapisujemy: rozmiar = offset i brak mtime
    // wymuszą doczytanie jej po starcie.
    let exact = parsed.pending.is_none() && parsed.offset == parsed.len;
    out.u64(if exact { parsed.len } else { parsed.offset });
    out.time(if exact { parsed.modified } else { None });
    out.u64(parsed.offset);
    out.blob(&parsed.tail_check);
    let parser = &parsed.parser;
    let default_project = out.string(&parser.default_project);
    out.u32(default_project);
    let codex_project = parser
        .codex_project
        .as_ref()
        .map_or(u32::MAX, |project| out.string(project));
    out.u32(codex_project);
    out.u8(u8::from(parser.is_transcript)
        | u8::from(parser.is_codex) << 1
        | u8::from(parser.first_user_seen) << 2
        | u8::from(parser.is_t3_metadata_session) << 3);
    out.u32(parsed.committed.len() as u32);
    for record in parsed.committed.iter() {
        out.u64(record.timestamp.and_utc().timestamp() as u64);
    }
    // Projekt zapisany seriami — w pliku zwykle jeden na wszystkie rekordy.
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for record in parsed.committed.iter() {
        let project = out.string(&record.project);
        match runs.last_mut() {
            Some((count, last)) if *last == project => *count += 1,
            _ => runs.push((1, project)),
        }
    }
    out.u32(runs.len() as u32);
    for (count, project) in runs {
        out.u32(count);
        out.u32(project);
    }
}

fn write_persisted(store: &Arc<Store>) -> std::io::Result<()> {
    let Some(path) = cache_path() else {
        return Ok(());
    };
    let entries: Vec<(PathBuf, Parsed)> = {
        let slots: Vec<(PathBuf, Slot)> = lock(&store.slots)
            .iter()
            .map(|(path, slot)| (path.clone(), slot.clone()))
            .collect();
        slots
            .into_iter()
            .filter_map(|(path, slot)| Some((path, lock(&slot).clone()?)))
            .collect()
    };
    let mut body = Writer {
        bytes: Vec::new(),
        strings: HashMap::new(),
    };
    let mut written = 0u32;
    for (path, parsed) in &entries {
        // Rekordy mają pełne sekundy; inne (niemożliwe) pomijamy, zamiast je psuć.
        let whole_seconds = parsed
            .committed
            .iter()
            .all(|record| record.timestamp.and_utc().timestamp_subsec_nanos() == 0);
        if whole_seconds && path.exists() {
            write_file(&mut body, path, parsed);
            written += 1;
        }
    }
    let mut table: Vec<(&Arc<str>, u32)> = body.strings.iter().map(|(s, i)| (s, *i)).collect();
    table.sort_by_key(|(_, index)| *index);
    let mut out = Writer {
        bytes: Vec::with_capacity(body.bytes.len() + 1024),
        strings: HashMap::new(),
    };
    out.bytes.extend_from_slice(MAGIC);
    out.u32(FORMAT);
    out.u64(parser_key());
    out.u64(store.projects_key);
    out.u32(table.len() as u32);
    for (string, _) in table {
        out.blob(string.as_bytes());
    }
    out.u32(written);
    out.bytes.extend_from_slice(&body.bytes);

    // Po `clear()` ten store jest już nieaktualny — nie nadpisujemy świeższego stanu.
    let current = STORE
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, store));
    if !current {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("bin.tmp");
    fs::write(&tmp, &out.bytes)?;
    fs::rename(&tmp, &path)
}

struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        if len > self.bytes.len() {
            return None;
        }
        let (head, rest) = self.bytes.split_at(len);
        self.bytes = rest;
        Some(head)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn blob(&mut self) -> Option<&'a [u8]> {
        let len = self.u32()? as usize;
        self.take(len)
    }
    fn time(&mut self) -> Option<Option<SystemTime>> {
        Some(match self.u8()? {
            0 => None,
            _ => {
                let secs = self.u64()?;
                let nanos = self.u32()?;
                if nanos >= 1_000_000_000 {
                    return None;
                }
                Some(UNIX_EPOCH.checked_add(Duration::new(secs, nanos))?)
            }
        })
    }
}

fn load_persisted(projects_key: u64) -> Option<HashMap<PathBuf, Slot>> {
    let bytes = fs::read(cache_path()?).ok()?;
    let mut input = Reader { bytes: &bytes };
    if input.take(MAGIC.len())? != MAGIC
        || input.u32()? != FORMAT
        || input.u64()? != parser_key()
        || input.u64()? != projects_key
    {
        return None;
    }
    let strings = (0..input.u32()?)
        .map(|_| Some(Arc::<str>::from(std::str::from_utf8(input.blob()?).ok()?)))
        .collect::<Option<Vec<_>>>()?;
    let string = |index: u32| strings.get(index as usize).cloned();
    let mut slots = HashMap::new();
    for _ in 0..input.u32()? {
        let path = PathBuf::from(std::ffi::OsString::from_vec(input.blob()?.to_vec()));
        let len = input.u64()?;
        let modified = input.time()?;
        let offset = input.u64()?;
        let tail_check = input.blob()?.to_vec();
        let default_project = string(input.u32()?)?;
        let codex_project = match input.u32()? {
            u32::MAX => None,
            index => Some(string(index)?),
        };
        let flags = input.u8()?;
        let parser = FileParser {
            default_project,
            is_transcript: flags & 1 != 0,
            is_codex: flags & 2 != 0,
            codex_project,
            first_user_seen: flags & 4 != 0,
            is_t3_metadata_session: flags & 8 != 0,
        };
        let count = input.u32()? as usize;
        if count > input.bytes.len() / 8 {
            return None;
        }
        let timestamps = (0..count)
            .map(|_| DateTime::from_timestamp(input.u64()? as i64, 0).map(|t| t.naive_utc()))
            .collect::<Option<Vec<_>>>()?;
        let mut records = Vec::with_capacity(count);
        for _ in 0..input.u32()? {
            let run = input.u32()? as usize;
            let project = string(input.u32()?)?;
            for timestamp in timestamps.get(records.len()..records.len() + run)? {
                records.push(TimestampRecord {
                    timestamp: *timestamp,
                    project: project.clone(),
                    is_codex: parser.is_codex,
                });
            }
        }
        if records.len() != count {
            return None;
        }
        let parsed = Parsed {
            len,
            modified,
            offset,
            tail_check,
            hidden: parser.is_t3_metadata_session,
            parser,
            committed: Arc::new(records),
            pending: None,
        };
        slots.insert(path, Arc::new(Mutex::new(Some(parsed))));
    }
    Some(slots)
}
