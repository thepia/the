//! Render-independent result data: commands build a [`Report`] of [`Table`]s, [`Record`]s and text,
//! and a renderer turns it into the requested output format. Commands never format output
//! themselves, so every command (and sub-subcommand) renders consistently and a new format is added
//! in one place.
//!
//! Human rendering shows only the columns/fields marked `human`, aligned. On a terminal it adds color
//! (a cell's [`Tone`], bold headers, dim labels and placeholders), a dim `─` rule under the header and
//! truncation; piped or redirected it stays aligned but plain and untruncated (cli-style-guide §8).
//! JSON always carries every column with typed values (lists stay arrays, missing values are `null`)
//! and is never colored.

use std::path::PathBuf;

use anstyle::Style;
use serde_json::{Map, Value};

use crate::output::Format;
use crate::style::{self, Term};

/// Semantic emphasis for a cell; color only reinforces the word, which carries the meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Warn,
    Fail,
    Dim,
}

impl Tone {
    fn style(self) -> Style {
        match self {
            Tone::Ok => style::OK,
            Tone::Warn => style::WARN,
            Tone::Fail => style::FAIL,
            Tone::Dim => style::DIM,
        }
    }
}

/// One typed value.
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Text(String),
    /// Text with emphasis in human output; a plain string in JSON.
    Toned(Tone, String),
    Path(PathBuf),
    Bool(bool),
    List(Vec<String>),
    /// Structured data; JSON keeps it as is, human output shows it compactly.
    Json(Value),
    Empty,
}

impl Cell {
    pub fn opt(s: Option<impl Into<String>>) -> Self {
        s.map_or(Cell::Empty, |s| Cell::Text(s.into()))
    }

    pub fn toned(tone: Tone, s: impl Into<String>) -> Self {
        Cell::Toned(tone, s.into())
    }

    fn human(&self) -> String {
        match self {
            Cell::Text(s) | Cell::Toned(_, s) => s.clone(),
            Cell::Path(p) => p.display().to_string(),
            Cell::Bool(b) => if *b { "yes" } else { "no" }.to_owned(),
            Cell::List(l) if l.is_empty() => "-".to_owned(),
            Cell::List(l) => l.join(", "),
            Cell::Json(Value::String(s)) => s.clone(),
            Cell::Json(v) => v.to_string(),
            Cell::Empty => "-".to_owned(),
        }
    }

    /// Placeholders and negatives recede; toned cells carry their tone.
    fn style(&self) -> Option<Style> {
        match self {
            Cell::Toned(t, _) => Some(t.style()),
            Cell::Empty | Cell::Bool(false) => Some(style::DIM),
            Cell::List(l) if l.is_empty() => Some(style::DIM),
            _ => None,
        }
    }

    fn json(&self) -> Value {
        match self {
            Cell::Text(s) | Cell::Toned(_, s) => Value::String(s.clone()),
            Cell::Path(p) => Value::String(p.display().to_string()),
            Cell::Bool(b) => Value::Bool(*b),
            Cell::List(l) => Value::from(l.clone()),
            Cell::Json(v) => v.clone(),
            Cell::Empty => Value::Null,
        }
    }
}

impl From<&str> for Cell {
    fn from(s: &str) -> Self {
        Cell::Text(s.to_owned())
    }
}

impl From<String> for Cell {
    fn from(s: String) -> Self {
        Cell::Text(s)
    }
}

impl From<PathBuf> for Cell {
    fn from(p: PathBuf) -> Self {
        Cell::Path(p)
    }
}

impl From<Vec<String>> for Cell {
    fn from(l: Vec<String>) -> Self {
        Cell::List(l)
    }
}

/// How human output is rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Human {
    pub term: Term,
    /// Print table header rows (`--no-header` turns them off).
    pub header: bool,
}

#[cfg(test)]
impl Human {
    /// Plain (piped) rendering with headers.
    pub const PLAIN: Human = Human {
        term: Term::PLAIN,
        header: true,
    };
}

#[derive(Debug, Clone)]
pub struct Column {
    /// JSON key.
    pub key: &'static str,
    /// Human header.
    pub header: &'static str,
    /// Shown in human output (JSON always includes every column).
    pub human: bool,
    /// On a terminal, longer values are truncated to this many characters.
    pub max_width: Option<usize>,
}

impl Column {
    pub fn new(key: &'static str, header: &'static str) -> Self {
        Self {
            key,
            header,
            human: true,
            max_width: None,
        }
    }

    /// A column carried only in machine output.
    pub fn json_only(key: &'static str) -> Self {
        Self {
            key,
            header: "",
            human: false,
            max_width: None,
        }
    }

    pub fn max_width(mut self, w: usize) -> Self {
        self.max_width = Some(w);
        self
    }
}

#[derive(Debug, Clone)]
pub struct Table {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Cell>>,
}

impl Table {
    pub fn new(columns: Vec<Column>) -> Self {
        Self {
            columns,
            rows: Vec::new(),
        }
    }

    /// Append a row; it must have one cell per column.
    pub fn push(&mut self, row: Vec<Cell>) {
        assert_eq!(
            row.len(),
            self.columns.len(),
            "row width must match the columns"
        );
        self.rows.push(row);
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    fn human(&self, h: Human) -> String {
        let cols: Vec<usize> = (0..self.columns.len())
            .filter(|&i| self.columns[i].human)
            .collect();
        let max = |i: usize| {
            if h.term.tty {
                self.columns[i].max_width
            } else {
                None
            }
        };
        // (plain text, style) per shown cell; widths are measured on the plain text.
        let cells: Vec<Vec<(String, Option<Style>)>> = self
            .rows
            .iter()
            .map(|r| {
                cols.iter()
                    .map(|&i| (fit(&r[i].human(), max(i)), r[i].style()))
                    .collect()
            })
            .collect();
        let widths: Vec<usize> = cols
            .iter()
            .enumerate()
            .map(|(j, &i)| {
                let header = if h.header {
                    self.columns[i].header.chars().count()
                } else {
                    0
                };
                cells
                    .iter()
                    .map(|r| r[j].0.chars().count())
                    .chain([header])
                    .max()
                    .unwrap_or(0)
            })
            .collect();

        let line = |row: &[(String, Option<Style>)]| {
            let mut s = String::new();
            for (j, (text, style)) in row.iter().enumerate() {
                if j > 0 {
                    s.push_str("  ");
                }
                s.push_str(&match style {
                    Some(st) => h.term.paint(*st, text),
                    None => text.clone(),
                });
                if j + 1 < row.len() {
                    s.push_str(&" ".repeat(widths[j] - text.chars().count()));
                }
            }
            format!("{}\n", s.trim_end())
        };

        let mut out = String::new();
        if h.header {
            let header: Vec<_> = cols
                .iter()
                .map(|&i| (self.columns[i].header.to_owned(), Some(style::HEAD)))
                .collect();
            out.push_str(&line(&header));
            if h.term.tty {
                let rule: Vec<_> = widths
                    .iter()
                    .map(|&w| ("─".repeat(w), Some(style::DIM)))
                    .collect();
                out.push_str(&line(&rule));
            }
        }
        for row in &cells {
            out.push_str(&line(row));
        }
        out
    }

    fn json(&self) -> Value {
        Value::Array(
            self.rows
                .iter()
                .map(|r| {
                    let mut m = Map::new();
                    for (c, v) in self.columns.iter().zip(r) {
                        m.insert(c.key.to_owned(), v.json());
                    }
                    Value::Object(m)
                })
                .collect(),
        )
    }
}

/// An ordered set of fields describing one thing (rendered `key: value`, one per line).
#[derive(Debug, Clone, Default)]
pub struct Record {
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone)]
pub struct Field {
    pub key: String,
    pub value: Cell,
    pub human: bool,
}

impl Record {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn field(mut self, key: impl Into<String>, value: impl Into<Cell>) -> Self {
        self.fields.push(Field {
            key: key.into(),
            value: value.into(),
            human: true,
        });
        self
    }

    pub fn json_only(mut self, key: impl Into<String>, value: impl Into<Cell>) -> Self {
        self.fields.push(Field {
            key: key.into(),
            value: value.into(),
            human: false,
        });
        self
    }

    fn human(&self, h: Human) -> String {
        let shown: Vec<&Field> = self.fields.iter().filter(|f| f.human).collect();
        let key_w = shown
            .iter()
            .map(|f| f.key.chars().count())
            .max()
            .unwrap_or(0)
            + 1;
        let mut out = String::new();
        for f in shown {
            // Lists render one item per line in a record, aligned under the first.
            let value = match &f.value {
                Cell::List(l) if !l.is_empty() => l.join("\n"),
                other => other.human(),
            };
            let paint_value = |v: &str| match f.value.style() {
                Some(st) => h.term.paint(st, v),
                None => v.to_owned(),
            };
            let label = format!("{}:", f.key);
            let pad = " ".repeat(key_w - label.chars().count());
            let mut lines = value.lines();
            let first = lines.next().unwrap_or("");
            let head = format!(
                "{}{pad} {}",
                h.term.paint(style::DIM, &label),
                paint_value(first)
            );
            out.push_str(head.trim_end());
            out.push('\n');
            for l in lines {
                out.push_str(format!("{:key_w$} {}", "", paint_value(l)).trim_end());
                out.push('\n');
            }
        }
        out
    }

    fn json(&self) -> Value {
        Value::Object(
            self.fields
                .iter()
                .map(|f| (f.key.clone(), f.value.json()))
                .collect(),
        )
    }
}

#[derive(Debug, Clone)]
pub enum Block {
    Table(Table),
    Record(Record),
    Text(String),
}

#[derive(Debug, Clone)]
pub struct Section {
    /// JSON key.
    pub key: &'static str,
    pub block: Block,
    pub human: bool,
    pub json: bool,
}

/// A command's whole result: ordered sections.
///
/// JSON: a report whose only section is a record renders as that object; otherwise an object keyed
/// by section. Human: the shown, non-empty sections, separated by blank lines.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub sections: Vec<Section>,
}

impl Report {
    pub fn new() -> Self {
        Self::default()
    }

    fn add(mut self, key: &'static str, block: Block, human: bool, json: bool) -> Self {
        self.sections.push(Section {
            key,
            block,
            human,
            json,
        });
        self
    }

    pub fn table(self, key: &'static str, table: Table) -> Self {
        self.add(key, Block::Table(table), true, true)
    }

    pub fn record(self, key: &'static str, record: Record) -> Self {
        self.add(key, Block::Record(record), true, true)
    }

    pub fn text(self, key: &'static str, text: impl Into<String>) -> Self {
        self.add(key, Block::Text(text.into()), true, true)
    }

    /// A section carried only in machine output.
    pub fn json_only(self, key: &'static str, block: Block) -> Self {
        self.add(key, block, false, true)
    }

    /// A section carried only in human output.
    pub fn human_only(self, key: &'static str, block: Block) -> Self {
        self.add(key, block, true, false)
    }

    pub fn render(&self, format: Format, h: Human) -> String {
        match format {
            Format::Human => self.human(h),
            Format::Json => {
                let mut s =
                    serde_json::to_string_pretty(&self.json()).expect("JSON values serialize");
                s.push('\n');
                s
            }
        }
    }

    fn human(&self, h: Human) -> String {
        let parts: Vec<String> = self
            .sections
            .iter()
            .filter(|s| s.human)
            .filter_map(|s| match &s.block {
                Block::Table(t) if t.is_empty() => None,
                Block::Table(t) => Some(t.human(h)),
                Block::Record(r) => Some(r.human(h)),
                Block::Text(t) if t.trim().is_empty() => None,
                Block::Text(t) => Some(format!("{}\n", t.trim_end())),
            })
            .collect();
        parts.join("\n")
    }

    fn json(&self) -> Value {
        let json_sections: Vec<&Section> = self.sections.iter().filter(|s| s.json).collect();
        if let [
            Section {
                block: Block::Record(r),
                ..
            },
        ] = json_sections.as_slice()
        {
            return r.json();
        }
        let mut m = Map::new();
        for s in json_sections {
            let v = match &s.block {
                Block::Table(t) => t.json(),
                Block::Record(r) => r.json(),
                Block::Text(t) => Value::String(t.clone()),
            };
            m.insert(s.key.to_owned(), v);
        }
        Value::Object(m)
    }
}

/// One line per table cell (line breaks become spaces), truncated with an ellipsis to `max`.
fn fit(s: &str, max: Option<usize>) -> String {
    let line = s
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    match max {
        Some(max) if line.chars().count() > max => {
            let cut: String = line.chars().take(max.saturating_sub(1)).collect();
            format!("{}…", cut.trim_end())
        }
        _ => line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TTY: Human = Human {
        term: Term {
            color: false,
            tty: true,
        },
        header: true,
    };
    const COLOR: Human = Human {
        term: Term {
            color: true,
            tty: true,
        },
        header: true,
    };

    fn sample() -> Table {
        let mut t = Table::new(vec![
            Column::new("name", "NAME"),
            Column::new("tags", "TAGS"),
            Column::new("about", "ABOUT").max_width(8),
            Column::json_only("path"),
        ]);
        t.push(vec![
            "a".into(),
            Cell::List(vec!["x".into(), "y".into()]),
            Cell::toned(Tone::Ok, "short"),
            PathBuf::from("/p/a").into(),
        ]);
        t.push(vec![
            "long-name".into(),
            Cell::List(vec![]),
            "a much longer text\nsecond".into(),
            Cell::Empty,
        ]);
        t
    }

    fn render(t: Table, h: Human) -> String {
        Report::new().table("t", t).render(Format::Human, h)
    }

    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut esc = false;
        for c in s.chars() {
            match (esc, c) {
                (false, '\u{1b}') => esc = true,
                (true, 'm') => esc = false,
                (true, _) => {}
                (false, c) => out.push(c),
            }
        }
        out
    }

    #[test]
    fn piped_table_is_aligned_plain_and_untruncated() {
        assert_eq!(
            render(sample(), Human::PLAIN),
            "NAME       TAGS  ABOUT\na          x, y  short\nlong-name  -     a much longer text second\n"
        );
    }

    #[test]
    fn terminal_table_adds_a_rule_and_truncates() {
        assert_eq!(
            render(sample(), TTY),
            "NAME       TAGS  ABOUT\n─────────  ────  ───────\na          x, y  short\nlong-name  -     a much…\n"
        );
    }

    #[test]
    fn color_does_not_change_the_layout() {
        let colored = render(sample(), COLOR);
        assert!(colored.contains('\u{1b}'));
        assert!(colored.contains(&style::paint(true, style::OK, "short")));
        assert!(colored.contains(&style::paint(true, style::HEAD, "NAME")));
        assert_eq!(strip_ansi(&colored), render(sample(), TTY));
    }

    #[test]
    fn no_header_drops_header_and_rule() {
        let h = Human {
            header: false,
            ..TTY
        };
        assert_eq!(
            render(sample(), h),
            "a          x, y  short\nlong-name  -     a much…\n"
        );
        let h = Human {
            header: false,
            ..Human::PLAIN
        };
        assert!(render(sample(), h).starts_with("a          x, y  short\n"));
    }

    #[test]
    fn json_table_keeps_every_column_typed_and_uncolored() {
        let out = Report::new()
            .table("t", sample())
            .render(Format::Json, COLOR);
        assert!(!out.contains('\u{1b}'));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["t"][0]["tags"], serde_json::json!(["x", "y"]));
        assert_eq!(v["t"][0]["about"], "short");
        assert_eq!(v["t"][0]["path"], "/p/a");
        assert_eq!(v["t"][1]["path"], Value::Null);
        assert_eq!(v["t"][1]["about"], "a much longer text\nsecond");
    }

    #[test]
    fn record_renders_aligned_with_multiline_lists() {
        let r = Record::new()
            .field("name", "a")
            .field("files", Cell::List(vec!["x".into(), "y".into()]))
            .json_only("secret", "z");
        let report = Report::new().record("skill", r);
        assert_eq!(
            report.render(Format::Human, Human::PLAIN),
            "name:  a\nfiles: x\n       y\n"
        );
        assert_eq!(
            strip_ansi(&report.render(Format::Human, COLOR)),
            "name:  a\nfiles: x\n       y\n"
        );
        let v: Value = serde_json::from_str(&report.render(Format::Json, Human::PLAIN)).unwrap();
        assert_eq!(v["secret"], "z");
        assert_eq!(v["files"][1], "y");
    }

    #[test]
    fn report_sections_join_with_blank_lines_and_skip_empty() {
        let empty = Table::new(vec![Column::new("a", "A")]);
        let report = Report::new()
            .record("r", Record::new().field("k", "v"))
            .table("e", empty)
            .text("body", "# Title\n")
            .json_only("hidden", Block::Text("h".into()));
        assert_eq!(
            report.render(Format::Human, Human::PLAIN),
            "k: v\n\n# Title\n"
        );
        let v: Value = serde_json::from_str(&report.render(Format::Json, Human::PLAIN)).unwrap();
        assert_eq!(v["hidden"], "h");
        assert_eq!(v["e"], serde_json::json!([]));
    }
}
