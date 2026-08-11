//! Swift-facing mirror of `pi_render::RowSpec` and its nested types — the
//! data `ChatSink::on_history_replaced` pushes. Plain UniFFI `Record`s with
//! `From` conversions, same pattern as `session_index::SessionRecord: From<
//! pi_sessions::SidebarRow>`.

#[derive(Clone, uniffi::Record)]
pub struct ColoredSpanRecord {
    pub text: String,
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl From<pi_render::highlight::ColoredSpan> for ColoredSpanRecord {
    fn from(span: pi_render::highlight::ColoredSpan) -> Self {
        let (red, green, blue) = span.color;
        Self {
            text: span.text,
            red,
            green,
            blue,
        }
    }
}

#[derive(Clone, uniffi::Record)]
pub struct CodeLineRecord {
    pub spans: Vec<ColoredSpanRecord>,
}

impl From<pi_render::highlight::CodeLine> for CodeLineRecord {
    fn from(line: pi_render::highlight::CodeLine) -> Self {
        Self {
            spans: line
                .spans
                .into_iter()
                .map(ColoredSpanRecord::from)
                .collect(),
        }
    }
}

#[derive(Clone, uniffi::Record)]
pub struct TableCellRecord {
    pub text: String,
    pub header: bool,
}

impl From<pi_render::segmenter::TableCell> for TableCellRecord {
    fn from(cell: pi_render::segmenter::TableCell) -> Self {
        Self {
            text: cell.text,
            header: cell.header,
        }
    }
}

/// Mirrors `pi_render::RowSpec` field-for-field; `kind` becomes an owned
/// `String` (UniFFI has no `&'static str`).
#[derive(Clone, uniffi::Record)]
pub struct RowRecord {
    pub kind: String,
    pub markdown: Option<String>,
    pub text: String,
    pub lang: String,
    pub level: i32,
    pub detail: String,
    pub running: bool,
    pub elapsed: String,
    pub first: bool,
    pub raw: String,
    pub code_lines: Vec<CodeLineRecord>,
    pub table_rows: Vec<Vec<TableCellRecord>>,
}

impl From<pi_render::RowSpec> for RowRecord {
    fn from(spec: pi_render::RowSpec) -> Self {
        Self {
            kind: spec.kind.to_string(),
            markdown: spec.markdown,
            text: spec.text,
            lang: spec.lang,
            level: spec.level,
            detail: spec.detail,
            running: spec.running,
            elapsed: spec.elapsed,
            first: spec.first,
            raw: spec.raw,
            code_lines: spec
                .code_lines
                .into_iter()
                .map(CodeLineRecord::from)
                .collect(),
            table_rows: spec
                .table_rows
                .into_iter()
                .map(|row| row.into_iter().map(TableCellRecord::from).collect())
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colored_span_conversion_splits_the_rgb_tuple() {
        let span = pi_render::highlight::ColoredSpan {
            text: "fn".to_string(),
            color: (10, 20, 30),
        };
        let record = ColoredSpanRecord::from(span);
        assert_eq!(record.text, "fn");
        assert_eq!((record.red, record.green, record.blue), (10, 20, 30));
    }

    #[test]
    fn row_record_conversion_preserves_every_field() {
        let spec = pi_render::RowSpec {
            kind: "code",
            markdown: None,
            text: "fn main() {}".to_string(),
            lang: "rust".to_string(),
            level: 0,
            detail: String::new(),
            running: false,
            elapsed: String::new(),
            first: true,
            raw: "```rust\nfn main() {}\n```".to_string(),
            code_lines: pi_render::highlight::highlight_lines("fn main() {}", "rust", true),
            table_rows: vec![vec![pi_render::segmenter::TableCell {
                text: "A".to_string(),
                header: true,
            }]],
        };
        let record = RowRecord::from(spec);
        assert_eq!(record.kind, "code");
        assert_eq!(record.text, "fn main() {}");
        assert_eq!(record.lang, "rust");
        assert!(record.first);
        assert!(!record.code_lines.is_empty());
        assert_eq!(record.table_rows.len(), 1);
        assert_eq!(record.table_rows[0][0].text, "A");
        assert!(record.table_rows[0][0].header);
    }
}
