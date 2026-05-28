pub mod render;
pub mod state;

use std::io;

use chrono::{Datelike, Local};
use ratatui::crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

use crate::archive;
use state::EditState;

pub fn run() {
    let summary = archive::load_summary();
    let now = Local::now().date_naive();
    let mut st = EditState::new(summary, now.year(), now.month());

    if let Err(e) = run_loop(&mut st) {
        eprintln!("[BŁĄD TUI] {}", e);
    }
}

fn run_loop(st: &mut EditState) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let res = event_loop(&mut terminal, st);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;
    res
}

fn event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    st: &mut EditState,
) -> io::Result<()> {
    loop {
        terminal.draw(|f| render::draw(f, st))?;

        let Event::Key(key) = event::read()? else { continue; };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        if st.editing.is_some() {
            match key.code {
                KeyCode::Enter => st.commit_edit(),
                KeyCode::Esc => st.cancel_edit(),
                KeyCode::Backspace => st.backspace(),
                KeyCode::Char(c) => st.input_char(c),
                _ => {}
            }
            continue;
        }

        match key.code {
            KeyCode::Char('q') => {
                if st.dirty {
                    st.status = "Niezapisane zmiany: 's' zapisz, 'q' wyjdź bez zapisu, inny klawisz anuluj".to_string();
                    if confirm_quit(terminal, st)? {
                        return Ok(());
                    }
                } else {
                    return Ok(());
                }
            }
            KeyCode::Char('s') => {
                let summary = st.finalize_for_save();
                match archive::save_summary(summary) {
                    Ok(()) => st.status = "zapisano na dysk ✓".to_string(),
                    Err(e) => st.status = format!("BŁĄD ZAPISU: {}", e),
                }
            }
            KeyCode::Char('m') => st.toggle_manual(),
            KeyCode::Up => st.move_up(),
            KeyCode::Down => st.move_down(),
            KeyCode::Left => st.prev_month(),
            KeyCode::Right => st.next_month(),
            KeyCode::Enter => st.begin_edit(),
            _ => {}
        }
    }
}

fn confirm_quit<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    st: &mut EditState,
) -> io::Result<bool> {
    terminal.draw(|f| render::draw(f, st))?;
    let Event::Key(key) = event::read()? else { return Ok(false); };
    if key.kind != KeyEventKind::Press {
        return Ok(false);
    }
    match key.code {
        KeyCode::Char('q') => Ok(true),
        KeyCode::Char('s') => {
            let summary = st.finalize_for_save();
            let _ = archive::save_summary(summary);
            Ok(true)
        }
        _ => {
            st.status.clear();
            Ok(false)
        }
    }
}
