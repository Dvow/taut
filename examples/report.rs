//! A yearly sales report: a monthly chart and some commentary, saved as PDF or printed. The
//! window shows the chart as the library draws charts; the pages redraw it with page shapes and
//! text, so it is sharp at any size and its text searchable, and the commentary goes on its own
//! page, laid out as a document.
//!   cargo run -p taut --example report
//!   cargo run -p taut --example report -- report.pdf   (writes the file, and shows nothing)
#![windows_subsystem = "windows"]
use taut::{Align, Chart, Color, Font, Length, Markdown, Paper, Path, Pen, PrintJob, PrintOutcome, Role, Series, Ui, Window, geom::rect};

/// The months, and what was sold in each, and what came back.
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const SOLD: [f32; 12] = [412.0, 388.0, 455.0, 501.0, 532.0, 610.0, 588.0, 571.0, 640.0, 702.0, 765.0, 820.0];
const BACK: [f32; 12] = [21.0, 18.0, 25.0, 22.0, 30.0, 28.0, 35.0, 31.0, 27.0, 33.0, 38.0, 41.0];

/// What the window keeps: where the file was saved, printing on its way, and how either went.
#[derive(Default)]
pub struct Report {
    saved: Option<String>,
    printing: Option<PrintJob>,
    status: String,
}

/// The report's words, written in Markdown.
pub fn words() -> String {
    let (sold, back): (f32, f32) = (SOLD.iter().sum(), BACK.iter().sum());
    let best = (0..12).max_by(|a, b| SOLD[*a].total_cmp(&SOLD[*b])).unwrap_or(0);
    let mut text = format!(
        "# The year in sales\n\n{sold:.0} things were sold over the year, and {back:.0} came back: {:.1} in every hundred. \
         The best month was **{}**, with {:.0}; the year ended higher than it began in every quarter.\n\n## Each quarter\n\n| Quarter | Sold | Back |\n| --- | ---: | ---: |\n",
        back / sold * 100.0,
        MONTHS[best],
        SOLD[best]
    );
    for quarter in 0..4 {
        let months = quarter * 3..quarter * 3 + 3;
        text += &format!("| {} | {:.0} | {:.0} |\n", ["First", "Second", "Third", "Fourth"][quarter], SOLD[months.clone()].iter().sum::<f32>(), BACK[months].iter().sum::<f32>());
    }
    text += "\n## What is next\n\n- Keep what sold best in the autumn.\n- Look into what came back in November and December.\n- Write the next report in April.\n";
    text
}

/// The pages of the report: the chart, drawn with the page's shapes and words, on the first;
/// the words of the report after it.
pub fn paper() -> Paper {
    let mut paper = Paper::new();
    paper.title = "The year in sales".into();
    paper.page(Paper::A4);
    let (black, dim) = (Color::rgb(0x1a202c), Color::rgb(0x4a5568));
    // (A screen reader reads the parts in this order: the heading, what the chart shows, the
    // report's words.)
    paper.begin_heading(1);
    paper.text("The year in sales", 72.0, 72.0, Font::new(28.0).bold(), black);
    paper.end();
    paper.text("Things sold and things that came back, by month", 72.0, 116.0, Font::new(14.0), dim);
    paper.begin(Role::Picture);
    paper.describe("A chart by month: sales rose from 412 in January to 820 in December; returns from 21 to 41.");
    chart(&mut paper, rect(72.0, 170.0, Paper::A4.w - 144.0, 420.0));
    paper.end();
    paper.markdown(&Markdown::parse(&words()), Paper::A4, 72.0);
    // A number at the foot of every page: decoration, which readers skip.
    let pages = paper.pages();
    for page in 0..pages {
        paper.set_page(page);
        let number = format!("{} of {pages}", page + 1);
        let wide = paper.measure(&number, Font::new(11.0));
        paper.begin_decoration();
        paper.text(&number, (Paper::A4.w - wide) / 2.0, Paper::A4.h - 48.0, Font::new(11.0), dim);
        paper.end();
    }
    paper
}

/// A chart of the months in `r`: a bar for what was sold, a line for what came back (on a
/// side of its own), with lines across at round numbers, the numbers beside them, the months
/// under the bars, and what each colour is under it all.
fn chart(paper: &mut Paper, r: taut::Rect) {
    let (sold, back, line, dim) = (Color::rgb(0x2b6cb0), Color::rgb(0xdd6b20), Color::rgb(0xe2e8f0), Color::rgb(0x4a5568));
    let small = Font::new(12.0);
    let plot = rect(r.x + 48.0, r.y, r.w - 96.0, r.h - 64.0);
    let (most, most_back) = (1000.0, 50.0);
    for step in 0..=5 {
        let y = plot.bottom() - plot.h * step as f32 / 5.0;
        paper.rect(rect(plot.x, y, plot.w, 1.0), line);
        let label = format!("{}", step * 200);
        let wide = paper.measure(&label, small);
        paper.text(&label, plot.x - 8.0 - wide, y - 8.0, small, dim);
        paper.text(&format!("{}", step * 10), plot.right() + 8.0, y - 8.0, small, back);
    }
    let slot = plot.w / 12.0;
    let mut path = Path::new();
    for (month, name) in MONTHS.iter().enumerate() {
        let x = plot.x + slot * month as f32;
        let high = plot.h * SOLD[month] / most;
        paper.rounded(rect(x + slot * 0.2, plot.bottom() - high, slot * 0.6, high), 3.0, sold);
        let wide = paper.measure(name, small);
        paper.text(name, x + (slot - wide) / 2.0, plot.bottom() + 8.0, small, dim);
        let y = plot.bottom() - plot.h * BACK[month] / most_back;
        match month {
            0 => _ = path.move_to(x + slot / 2.0, y),
            _ => _ = path.line_to(x + slot / 2.0, y),
        }
    }
    paper.outline(&path, Pen::new(3.0), back);
    for (month, came) in BACK.iter().enumerate() {
        let mut dot = Path::new();
        dot.circle(plot.x + slot * (month as f32 + 0.5), plot.bottom() - plot.h * came / most_back, 4.5);
        paper.shape(&dot, back);
    }
    let key = plot.bottom() + 40.0;
    paper.rounded(rect(plot.x, key, 14.0, 14.0), 3.0, sold);
    paper.text("Sold (left)", plot.x + 22.0, key - 2.0, small, dim);
    paper.rounded(rect(plot.x + 140.0, key + 5.0, 14.0, 4.0), 2.0, back);
    paper.text("Came back (right)", plot.x + 162.0, key - 2.0, small, dim);
}

/// The window: the chart as the library draws one, and buttons to keep the report as a PDF
/// file and to print it.
pub fn describe(report: &mut Report, ui: &mut Ui) {
    let t = ui.theme;
    ui.column().pad(16.0).gap(12.0).grow().open();
    ui.text("The year in sales", t.font.bold(), t.text);
    let rows = [Series::bars("Sold", &SOLD), Series::line("Came back", &BACK).color(t.red)];
    ui.chart(&Chart::of(&rows).categories(&MONTHS), 560.0, 260.0);
    ui.row().gap(8.0).align(Align::Start, Align::Center).open();
    if ui.primary_button("Save as PDF").clicked {
        let file = std::env::temp_dir().join("report.pdf");
        report.status = match std::fs::write(&file, paper().to_pdf()) {
            Ok(()) => format!("Saved as {}", file.display()),
            Err(error) => format!("Not saved: {error}"),
        };
        report.saved = Some(file.display().to_string());
    }
    if ui.button("Print").clicked && report.printing.is_none() {
        report.printing = Some(taut::print(paper(), "The year in sales"));
    }
    if let Some(done) = report.printing.as_ref().and_then(PrintJob::outcome) {
        report.printing = None;
        report.status = match done {
            PrintOutcome::Sent => "Sent to the printer".into(),
            PrintOutcome::Closed => String::new(),
            _ => "There is no printing here".into(),
        };
    }
    ui.el().w(Length::Grow(1.0)).text(&report.status, t.font, t.text_dim).clip().show();
    ui.close();
    ui.close();
}

fn main() {
    // With a name, the file is written there, and that is all.
    if let Some(name) = std::env::args().nth(1) {
        std::fs::write(&name, paper().to_pdf()).expect("the file is written");
        return;
    }
    let mut report = Report::default();
    taut::run(Window { title: "Report", width: 600.0, height: 400.0, ..Default::default() }, move |ui| describe(&mut report, ui));
}

#[cfg(test)]
mod tests {
    use super::*;
    use taut::testing::Trial;

    #[test]
    fn the_report_is_a_chart_and_a_page_of_words() {
        let paper = paper();
        assert!(paper.pages() >= 2, "{} pages", paper.pages());
        let file = paper.to_pdf();
        assert!(file.starts_with(b"%PDF-") && file.len() > 2_000);
    }

    #[test]
    fn the_report_is_saved_from_its_window() {
        let mut trial = Trial::new(Report::default(), describe);
        trial.resize(600.0, 400.0, 1.0);
        trial.press(taut::Role::Button, "Save as PDF");
        let saved = trial.app.saved.clone().expect("saved");
        assert!(std::fs::read(&saved).expect("the file is there").starts_with(b"%PDF-"));
        assert!(trial.app.status.starts_with("Saved as"));
    }
}
