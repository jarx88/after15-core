use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
    Frame,
};

use crate::archive::format_hm;
use crate::tui::state::EditState;

const MONTHS_PL: [&str; 12] = [
    "styczeń", "luty", "marzec", "kwiecień", "maj", "czerwiec",
    "lipiec", "sierpień", "wrzesień", "październik", "listopad", "grudzień",
];

pub fn draw(f: &mut Frame, st: &EditState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(3), Constraint::Length(2)])
        .split(f.area());

    // Nagłówek
    let title = format!(
        " after15 · edycja dni                 {} {}",
        MONTHS_PL[(st.month - 1) as usize],
        st.year
    );
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            title,
            Style::default().add_modifier(Modifier::BOLD),
        ))),
        chunks[0],
    );

    // Tabela
    let header = Row::new(vec![
        Cell::from("Data"),
        Cell::from("Godz."),
        Cell::from("Zmiana"),
        Cell::from("✎"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let editing_idx = if st.editing.is_some() { Some(st.cursor) } else { None };

    let rows: Vec<Row> = st.rows.iter().enumerate().map(|(i, r)| {
        let hours_cell = if Some(i) == editing_idx {
            format!("[{}]", st.editing.as_deref().unwrap_or(""))
        } else {
            format_hm(r.hours)
        };
        let flag = if r.manual_override { "✎" } else { "" };
        let mut style = Style::default();
        if i == st.cursor {
            style = style.bg(Color::Blue).fg(Color::White);
        } else if !r.existed {
            style = style.fg(Color::DarkGray);
        }
        Row::new(vec![
            Cell::from(r.date.format("%Y-%m-%d").to_string()),
            Cell::from(hours_cell),
            Cell::from(r.shift.clone()),
            Cell::from(flag),
        ])
        .style(style)
    }).collect();

    let table = Table::new(
        rows,
        [Constraint::Length(12), Constraint::Length(8), Constraint::Length(20), Constraint::Length(3)],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL));

    f.render_widget(table, chunks[1]);

    // Stopka
    let month_key = format!("{}-{:02}", st.year, st.month);
    let month_sum = st
        .summary
        .months
        .get(&month_key)
        .map(|m| m.formatted.clone())
        .unwrap_or_else(|| "0:00".to_string());
    let help = "↑↓ ruch · ←→ miesiąc · Enter edytuj · m flaga · s zapis · q wyjście";
    let footer = format!(" {}\n Σ miesiąc: {}   {}", help, month_sum, st.status);
    f.render_widget(
        Paragraph::new(footer).style(Style::default().fg(Color::Gray)),
        chunks[2],
    );
}
