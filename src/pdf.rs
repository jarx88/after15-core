use chrono::{Datelike, NaiveDate};
use printpdf::path::{PaintMode, WindingOrder};
use printpdf::*;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;

use crate::config::Config;
use crate::jsonl::ProjectHours;
use crate::report::normalize_project_name;

const FONT_DIRS: &[&str] = &[
    "/usr/share/fonts/liberation",
    "/usr/share/fonts/truetype/liberation",
    "/usr/share/fonts/liberation-sans",
    "/usr/share/fonts/truetype/liberation-sans",
];

// Colors (RGB 0-1)
const PRIMARY: (f32, f32, f32) = (0.118, 0.227, 0.373); // #1e3a5f - dark blue
const ACCENT: (f32, f32, f32) = (0.153, 0.682, 0.376); // #27ae60 - green
const HEADER_BG: (f32, f32, f32) = (0.204, 0.286, 0.369); // #344961 - table header
const ROW_ALT: (f32, f32, f32) = (0.961, 0.969, 0.976); // #f5f7f9 - zebra stripe
const TEXT_DARK: (f32, f32, f32) = (0.173, 0.243, 0.314); // #2c3e50
const WHITE: (f32, f32, f32) = (1.0, 1.0, 1.0);

// Page dimensions (A4 in mm)
const PAGE_W: f32 = 210.0;
const PAGE_H: f32 = 297.0;
const MARGIN: f32 = 20.0;

pub fn generate_pdf(
    daily_projects: &HashMap<NaiveDate, HashMap<String, ProjectHours>>,
    config: &Config,
    month_filter: Option<&str>,
) -> Result<PathBuf, String> {
    let (month_name, year, filtered_dates) = get_month_info(daily_projects, month_filter)?;
    let project_totals = calculate_project_totals(daily_projects, &filtered_dates, config);

    let (doc, page1, layer1) = PdfDocument::new(
        &format!("Raport nadgodzin - {} {}", month_name, year),
        Mm(PAGE_W),
        Mm(PAGE_H),
        "Layer 1",
    );

    let layer = doc.get_page(page1).get_layer(layer1);

    // Load fonts
    let font_regular = load_font(&doc, "LiberationSans-Regular.ttf")?;
    let font_bold = load_font(&doc, "LiberationSans-Bold.ttf")?;

    let mut y = PAGE_H - MARGIN;

    // === HEADER BANNER ===
    let header_height = 35.0;
    draw_rect(
        &layer,
        MARGIN,
        y - header_height,
        PAGE_W - 2.0 * MARGIN,
        header_height,
        PRIMARY,
    );

    // Title
    layer.set_fill_color(Color::Rgb(Rgb::new(WHITE.0, WHITE.1, WHITE.2, None)));
    layer.use_text(
        &format!("RAPORT NADGODZIN"),
        24.0,
        Mm(MARGIN + 10.0),
        Mm(y - 15.0),
        &font_bold,
    );

    // Month/Year
    layer.use_text(
        &format!("{} {}", month_name.to_uppercase(), year),
        14.0,
        Mm(PAGE_W - MARGIN - 60.0),
        Mm(y - 15.0),
        &font_regular,
    );

    y -= header_height + 8.0;

    // === SUBTITLE ===
    layer.set_fill_color(Color::Rgb(Rgb::new(
        TEXT_DARK.0,
        TEXT_DARK.1,
        TEXT_DARK.2,
        None,
    )));
    layer.use_text("Jaroslaw Hartwich", 12.0, Mm(MARGIN), Mm(y), &font_bold);
    y -= 5.0;
    layer.use_text(
        "Nadgodziny spedzone na kodowaniu ponad wymiar pracy",
        10.0,
        Mm(MARGIN),
        Mm(y),
        &font_regular,
    );

    y -= 15.0;

    // === TABLE ===
    let col_widths = [75.0, 25.0, 25.0, 25.0, 20.0]; // Project, Hours, Type, PLN, %
    let row_height = 8.0;
    let table_x = MARGIN;

    // Table header
    draw_rect(
        &layer,
        table_x,
        y - row_height,
        PAGE_W - 2.0 * MARGIN,
        row_height,
        HEADER_BG,
    );

    layer.set_fill_color(Color::Rgb(Rgb::new(WHITE.0, WHITE.1, WHITE.2, None)));
    let headers = ["PROJEKT", "GODZINY", "TYP", "PLN", "%"];
    let mut x = table_x + 3.0;
    for (i, header) in headers.iter().enumerate() {
        layer.use_text(*header, 9.0, Mm(x), Mm(y - 5.5), &font_bold);
        x += col_widths[i];
    }
    y -= row_height;

    // Calculate totals and rates
    let tracked_path = &config.projects.tracked_path;
    let hourly_weekday = config.salary.base_monthly_net / config.salary.hours_per_month
        * config.salary.overtime_multiplier_weekday;
    let hourly_weekend = config.salary.base_monthly_net / config.salary.hours_per_month
        * config.salary.overtime_multiplier_weekend;

    // Sort projects by total hours
    let mut sorted_projects: Vec<_> = project_totals.iter().collect();
    sorted_projects.sort_by(|a, b| {
        let total_a = a.1.weekday_hours + a.1.weekend_hours;
        let total_b = b.1.weekday_hours + b.1.weekend_hours;
        total_b.partial_cmp(&total_a).unwrap()
    });

    // Calculate grand totals
    let mut grand_total_hours = 0.0;
    let mut grand_total_pln = 0.0;
    for (_, hours) in &sorted_projects {
        let total = hours.weekday_hours + hours.weekend_hours;
        let pln = hours.weekday_hours * hourly_weekday + hours.weekend_hours * hourly_weekend;
        grand_total_hours += total;
        grand_total_pln += pln;
    }

    // Table rows
    let mut row_idx = 0;
    for (proj_name, hours) in &sorted_projects {
        let display_name = normalize_project_name(proj_name, tracked_path);
        let total_hours = hours.weekday_hours + hours.weekend_hours;

        if total_hours < 0.01 {
            continue;
        }

        // Weekday row
        if hours.weekday_hours > 0.01 {
            let pln = hours.weekday_hours * hourly_weekday;
            let pct = (hours.weekday_hours / grand_total_hours * 100.0).round();

            if row_idx % 2 == 1 {
                draw_rect(
                    &layer,
                    table_x,
                    y - row_height,
                    PAGE_W - 2.0 * MARGIN,
                    row_height,
                    ROW_ALT,
                );
            }

            layer.set_fill_color(Color::Rgb(Rgb::new(
                TEXT_DARK.0,
                TEXT_DARK.1,
                TEXT_DARK.2,
                None,
            )));
            let mut x = table_x + 3.0;
            layer.use_text(
                &truncate(&display_name, 28),
                9.0,
                Mm(x),
                Mm(y - 5.5),
                &font_regular,
            );
            x += col_widths[0];
            layer.use_text(
                &format_hours(hours.weekday_hours),
                9.0,
                Mm(x),
                Mm(y - 5.5),
                &font_regular,
            );
            x += col_widths[1];
            layer.use_text("dzien", 9.0, Mm(x), Mm(y - 5.5), &font_regular);
            x += col_widths[2];
            layer.use_text(
                &format!("{:.0}", pln),
                9.0,
                Mm(x),
                Mm(y - 5.5),
                &font_regular,
            );
            x += col_widths[3];
            layer.use_text(
                &format!("{:.0}%", pct),
                9.0,
                Mm(x),
                Mm(y - 5.5),
                &font_regular,
            );

            y -= row_height;
            row_idx += 1;
        }

        // Weekend row
        if hours.weekend_hours > 0.01 {
            let pln = hours.weekend_hours * hourly_weekend;
            let pct = (hours.weekend_hours / grand_total_hours * 100.0).round();
            let name = if hours.weekday_hours > 0.01 {
                "".to_string()
            } else {
                display_name.clone()
            };

            if row_idx % 2 == 1 {
                draw_rect(
                    &layer,
                    table_x,
                    y - row_height,
                    PAGE_W - 2.0 * MARGIN,
                    row_height,
                    ROW_ALT,
                );
            }

            // Weekend text in different color
            layer.set_fill_color(Color::Rgb(Rgb::new(
                TEXT_DARK.0,
                TEXT_DARK.1,
                TEXT_DARK.2,
                None,
            )));
            let mut x = table_x + 3.0;
            layer.use_text(&truncate(&name, 28), 9.0, Mm(x), Mm(y - 5.5), &font_regular);
            x += col_widths[0];
            layer.use_text(
                &format_hours(hours.weekend_hours),
                9.0,
                Mm(x),
                Mm(y - 5.5),
                &font_regular,
            );
            x += col_widths[1];
            layer.set_fill_color(Color::Rgb(Rgb::new(0.6, 0.4, 0.0, None))); // orange for weekend
            layer.use_text("weekend", 9.0, Mm(x), Mm(y - 5.5), &font_bold);
            layer.set_fill_color(Color::Rgb(Rgb::new(
                TEXT_DARK.0,
                TEXT_DARK.1,
                TEXT_DARK.2,
                None,
            )));
            x += col_widths[2];
            layer.use_text(
                &format!("{:.0}", pln),
                9.0,
                Mm(x),
                Mm(y - 5.5),
                &font_regular,
            );
            x += col_widths[3];
            layer.use_text(
                &format!("{:.0}%", pct),
                9.0,
                Mm(x),
                Mm(y - 5.5),
                &font_regular,
            );

            y -= row_height;
            row_idx += 1;
        }
    }

    // === TOTAL ROW ===
    y -= 3.0;
    draw_rect(
        &layer,
        table_x,
        y - row_height - 2.0,
        PAGE_W - 2.0 * MARGIN,
        row_height + 2.0,
        ACCENT,
    );

    layer.set_fill_color(Color::Rgb(Rgb::new(WHITE.0, WHITE.1, WHITE.2, None)));
    let mut x = table_x + 3.0;
    layer.use_text("SUMA", 10.0, Mm(x), Mm(y - 6.0), &font_bold);
    x += col_widths[0];
    layer.use_text(
        &format_hours(grand_total_hours),
        10.0,
        Mm(x),
        Mm(y - 6.0),
        &font_bold,
    );
    x += col_widths[1];
    x += col_widths[2];
    layer.use_text(
        &format!("{:.0} PLN", grand_total_pln),
        10.0,
        Mm(x),
        Mm(y - 6.0),
        &font_bold,
    );

    y -= row_height + 15.0;

    layer.set_fill_color(Color::Rgb(Rgb::new(0.5, 0.5, 0.5, None)));
    layer.use_text(
        &format!(
            "Stawka netto: {:.0} PLN/h (dzien), {:.0} PLN/h (weekend)",
            hourly_weekday, hourly_weekend
        ),
        8.0,
        Mm(MARGIN),
        Mm(y),
        &font_regular,
    );
    y -= 4.0;
    layer.use_text(
        "Wszystkie kwoty sa netto dla pracownika",
        8.0,
        Mm(MARGIN),
        Mm(y),
        &font_regular,
    );
    y -= 4.0;
    layer.use_text(
        &format!(
            "Wygenerowano: {}",
            chrono::Local::now().format("%Y-%m-%d %H:%M")
        ),
        8.0,
        Mm(MARGIN),
        Mm(y),
        &font_regular,
    );

    // Save PDF
    let output_path = get_output_path(&month_name, year);
    let file =
        File::create(&output_path).map_err(|e| format!("Nie mozna utworzyc pliku: {}", e))?;
    doc.save(&mut BufWriter::new(file))
        .map_err(|e| format!("Blad zapisu PDF: {}", e))?;

    Ok(output_path)
}

fn draw_rect(layer: &PdfLayerReference, x: f32, y: f32, w: f32, h: f32, color: (f32, f32, f32)) {
    layer.set_fill_color(Color::Rgb(Rgb::new(color.0, color.1, color.2, None)));

    let points = vec![
        (Point::new(Mm(x), Mm(y)), false),
        (Point::new(Mm(x + w), Mm(y)), false),
        (Point::new(Mm(x + w), Mm(y + h)), false),
        (Point::new(Mm(x), Mm(y + h)), false),
    ];

    let polygon = Polygon {
        rings: vec![points],
        mode: PaintMode::Fill,
        winding_order: WindingOrder::NonZero,
    };

    layer.add_polygon(polygon);
}

fn load_font(doc: &PdfDocumentReference, filename: &str) -> Result<IndirectFontRef, String> {
    for dir in FONT_DIRS {
        let path = format!("{}/{}", dir, filename);
        if std::path::Path::new(&path).exists() {
            let font_data = std::fs::read(&path)
                .map_err(|e| format!("Nie mozna wczytac fontu {}: {}", path, e))?;
            return doc
                .add_external_font(&*font_data)
                .map_err(|e| format!("Nie mozna dodac fontu: {}", e));
        }
    }
    Err(format!(
        "Nie znaleziono fontu {}. Zainstaluj fonts-liberation.",
        filename
    ))
}

fn get_month_info(
    daily_projects: &HashMap<NaiveDate, HashMap<String, ProjectHours>>,
    month_filter: Option<&str>,
) -> Result<(String, i32, Vec<NaiveDate>), String> {
    let filtered_dates: Vec<NaiveDate> = if let Some(filter) = month_filter {
        let parts: Vec<&str> = filter.split('-').collect();
        if parts.len() != 2 {
            return Err("Nieprawidlowy format miesiaca (YYYY-MM)".to_string());
        }
        let year: i32 = parts[0].parse().map_err(|_| "Nieprawidlowy rok")?;
        let month: u32 = parts[1].parse().map_err(|_| "Nieprawidlowy miesiac")?;

        daily_projects
            .keys()
            .filter(|d| d.year() == year && d.month() == month)
            .copied()
            .collect()
    } else {
        let today = chrono::Local::now().date_naive();
        daily_projects
            .keys()
            .filter(|d| d.year() == today.year() && d.month() == today.month())
            .copied()
            .collect()
    };

    if filtered_dates.is_empty() {
        return Err("Brak danych dla wybranego miesiaca".to_string());
    }

    let first_date = filtered_dates.iter().min().unwrap();
    let month_name = get_polish_month_name(first_date.month());
    let year = first_date.year();

    Ok((month_name, year, filtered_dates))
}

fn calculate_project_totals(
    daily_projects: &HashMap<NaiveDate, HashMap<String, ProjectHours>>,
    filtered_dates: &[NaiveDate],
    config: &Config,
) -> HashMap<String, ProjectHours> {
    let mut totals: HashMap<String, ProjectHours> = HashMap::new();

    for date in filtered_dates {
        if let Some(day_projects) = daily_projects.get(date) {
            for (proj_name, hours) in day_projects {
                let normalized = normalize_project_name(proj_name, &config.projects.tracked_path);

                if config.projects.excluded_projects.contains(&normalized) {
                    continue;
                }

                let entry = totals.entry(proj_name.clone()).or_default();
                entry.weekday_hours += hours.weekday_hours;
                entry.weekend_hours += hours.weekend_hours;
            }
        }
    }

    totals
}

fn format_hours(hours: f64) -> String {
    let h = hours.floor() as i64;
    let m = ((hours - hours.floor()) * 60.0).round() as i64;
    format!("{}:{:02}", h, m)
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(max_len - 3).collect::<String>())
    }
}

fn get_polish_month_name(month: u32) -> String {
    match month {
        1 => "styczen",
        2 => "luty",
        3 => "marzec",
        4 => "kwiecien",
        5 => "maj",
        6 => "czerwiec",
        7 => "lipiec",
        8 => "sierpien",
        9 => "wrzesien",
        10 => "pazdziernik",
        11 => "listopad",
        12 => "grudzien",
        _ => "?",
    }
    .to_string()
}

fn get_output_path(month_name: &str, year: i32) -> PathBuf {
    let filename = format!("nadgodziny_{}_{}.pdf", month_name, year);

    if let Some(home) = dirs::home_dir() {
        home.join(&filename)
    } else {
        PathBuf::from(&filename)
    }
}
