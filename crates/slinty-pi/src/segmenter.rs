//! Splits assistant markdown into renderable segments.
//!
//! Slint's `StyledText` handles an inline CommonMark subset (bold, italic,
//! strikethrough, inline code, links, lists, `<u>`, `<font color>`), but not
//! headings, fenced code blocks, tables, images, or block quotes. So we walk
//! the source with pulldown-cmark at block level and hand prose blocks to
//! StyledText verbatim while extracting the constructs it can't render.
//!
//! Re-segmenting runs on every streaming flush (~30 Hz) over the growing
//! message, so this stays allocation-light and single-pass. pulldown-cmark
//! treats an unclosed fence as a code block to EOF, which renders streamed
//! code live.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableCell {
    pub text: String,
    pub header: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableColumn {
    pub cells: Vec<TableCell>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// Markdown that StyledText renders directly.
    Prose(String),
    Heading {
        level: u8,
        text: String,
    },
    Code {
        lang: String,
        code: String,
    },
    /// Row-major source pivoted to column-major for rendering (each column
    /// is a vertical run of cells, easiest shape for a Slint grid).
    Table(Vec<TableColumn>),
}

pub fn segment_markdown(source: &str) -> Vec<Segment> {
    let mut segments: Vec<Segment> = Vec::new();
    let mut depth = 0usize;
    // Start of the current run of verbatim prose blocks, if any.
    let mut prose_start: Option<usize> = None;
    // State of the top-level block currently being walked.
    enum Block {
        Prose,
        Heading {
            level: u8,
            text: String,
        },
        Code {
            lang: String,
            code: String,
        },
        Table {
            in_head: bool,
            rows: Vec<Vec<TableCell>>,
            current_row: Vec<TableCell>,
            current_cell: String,
        },
    }
    let mut block: Option<Block> = None;

    let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
    let parser = Parser::new_ext(source, options).into_offset_iter();

    let flush_prose = |segments: &mut Vec<Segment>, start: &mut Option<usize>, end: usize| {
        if let Some(s) = start.take() {
            let text = source[s..end].trim();
            if !text.is_empty() {
                segments.push(Segment::Prose(text.to_string()));
            }
        }
    };

    for (event, range) in parser {
        match event {
            Event::Start(tag) => {
                // `Table` opens a top-level block like Heading/Code below, but
                // TableHead/TableRow/TableCell nest *inside* that already-open
                // block, so `depth` is never 0 for them — handle them first,
                // unconditionally, whenever a table block is open.
                if let Some(Block::Table {
                    in_head,
                    current_cell,
                    ..
                }) = block.as_mut()
                {
                    match &tag {
                        Tag::TableHead => *in_head = true,
                        Tag::TableCell => current_cell.clear(),
                        _ => {}
                    }
                }
                if depth == 0 {
                    block = Some(match &tag {
                        Tag::Heading { level, .. } => {
                            flush_prose(&mut segments, &mut prose_start, range.start);
                            Block::Heading {
                                level: heading_level(*level),
                                text: String::new(),
                            }
                        }
                        Tag::CodeBlock(kind) => {
                            flush_prose(&mut segments, &mut prose_start, range.start);
                            let lang = match kind {
                                CodeBlockKind::Fenced(info) => info
                                    .split_whitespace()
                                    .next()
                                    .unwrap_or_default()
                                    .to_string(),
                                CodeBlockKind::Indented => String::new(),
                            };
                            Block::Code {
                                lang,
                                code: String::new(),
                            }
                        }
                        Tag::Table(_) => {
                            flush_prose(&mut segments, &mut prose_start, range.start);
                            Block::Table {
                                in_head: false,
                                rows: Vec::new(),
                                current_row: Vec::new(),
                                current_cell: String::new(),
                            }
                        }
                        _ => {
                            if prose_start.is_none() {
                                prose_start = Some(range.start);
                            }
                            Block::Prose
                        }
                    });
                }
                depth += 1;
            }
            Event::End(tag_end) => {
                depth = depth.saturating_sub(1);
                // Same reasoning as above: table internals close before the
                // table itself does, at depth > 0.
                if let Some(Block::Table {
                    in_head,
                    rows,
                    current_row,
                    current_cell,
                }) = block.as_mut()
                {
                    match tag_end {
                        // pulldown-cmark puts header cells directly under
                        // TableHead, with no nested TableRow — so TableHead's
                        // end must flush the row too, same as TableRow's.
                        TagEnd::TableHead => {
                            *in_head = false;
                            rows.push(std::mem::take(current_row));
                        }
                        TagEnd::TableRow => rows.push(std::mem::take(current_row)),
                        TagEnd::TableCell => current_row.push(TableCell {
                            text: std::mem::take(current_cell).trim().to_string(),
                            header: *in_head,
                        }),
                        _ => {}
                    }
                }
                if depth == 0 {
                    match block.take() {
                        Some(Block::Heading { level, text }) => {
                            segments.push(Segment::Heading { level, text });
                        }
                        Some(Block::Code { lang, code }) => {
                            let code = code.strip_suffix('\n').unwrap_or(&code).to_string();
                            segments.push(Segment::Code { lang, code });
                        }
                        Some(Block::Table { rows, .. }) => {
                            segments.push(Segment::Table(pivot_table(rows)));
                        }
                        _ => {
                            // Prose runs accumulate until a non-prose block or EOF.
                            let _ = tag_end;
                        }
                    }
                }
            }
            Event::Text(t) => match block.as_mut() {
                Some(Block::Heading { text, .. }) => text.push_str(&t),
                Some(Block::Code { code, .. }) => code.push_str(&t),
                Some(Block::Table { current_cell, .. }) => current_cell.push_str(&t),
                _ => {}
            },
            Event::SoftBreak | Event::HardBreak => {
                if let Some(Block::Table { current_cell, .. }) = block.as_mut() {
                    current_cell.push(' ');
                }
            }
            Event::Code(t) => {
                if let Some(Block::Table { current_cell, .. }) = block.as_mut() {
                    current_cell.push_str(&t);
                }
                if let Some(Block::Heading { text, .. }) = block.as_mut() {
                    text.push_str(&t);
                }
            }
            _ => {}
        }
    }
    flush_prose(&mut segments, &mut prose_start, source.len());
    segments
}

/// Pivot row-major table cells (as they arrive from pulldown-cmark, one
/// `Vec<TableCell>` per row) into column-major `TableColumn`s.
fn pivot_table(rows: Vec<Vec<TableCell>>) -> Vec<TableColumn> {
    let Some(col_count) = rows.first().map(Vec::len) else {
        return Vec::new();
    };
    (0..col_count)
        .map(|i| TableColumn {
            cells: rows.iter().filter_map(|row| row.get(i).cloned()).collect(),
        })
        .collect()
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(source: &str) -> Vec<Segment> {
        segment_markdown(source)
    }

    fn cell(text: &str, header: bool) -> TableCell {
        TableCell {
            text: text.into(),
            header,
        }
    }

    #[test]
    fn plain_paragraph_is_prose() {
        assert_eq!(
            seg("Hello **world**."),
            vec![Segment::Prose("Hello **world**.".into())]
        );
    }

    #[test]
    fn consecutive_prose_blocks_stay_one_segment() {
        let s =
            "First paragraph.\n\n- a list\n- with items\n\nLast paragraph with [link](https://x).";
        assert_eq!(seg(s), vec![Segment::Prose(s.into())]);
    }

    #[test]
    fn table_pivots_rows_into_columns_with_header_flag() {
        let s = "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n| Bob | 25 |";
        assert_eq!(
            seg(s),
            vec![Segment::Table(vec![
                TableColumn {
                    cells: vec![cell("Name", true), cell("Alice", false), cell("Bob", false)]
                },
                TableColumn {
                    cells: vec![cell("Age", true), cell("30", false), cell("25", false)]
                },
            ])]
        );
    }

    #[test]
    fn table_surrounded_by_prose_splits_into_three_segments() {
        let s = "Before.\n\n| A |\n| --- |\n| 1 |\n\nAfter.";
        assert_eq!(
            seg(s),
            vec![
                Segment::Prose("Before.".into()),
                Segment::Table(vec![TableColumn {
                    cells: vec![cell("A", true), cell("1", false)]
                }]),
                Segment::Prose("After.".into()),
            ]
        );
    }

    #[test]
    fn table_cell_with_inline_code_keeps_text() {
        let s = "| Cmd |\n| --- |\n| `ls -la` |";
        assert_eq!(
            seg(s),
            vec![Segment::Table(vec![TableColumn {
                cells: vec![cell("Cmd", true), cell("ls -la", false)]
            }])]
        );
    }

    #[test]
    fn fenced_code_is_extracted() {
        let s = "Before.\n\n```rust\nfn main() {}\n```\n\nAfter.";
        assert_eq!(
            seg(s),
            vec![
                Segment::Prose("Before.".into()),
                Segment::Code {
                    lang: "rust".into(),
                    code: "fn main() {}".into()
                },
                Segment::Prose("After.".into()),
            ]
        );
    }

    #[test]
    fn heading_is_extracted() {
        assert_eq!(
            seg("# Title\n\nBody."),
            vec![
                Segment::Heading {
                    level: 1,
                    text: "Title".into()
                },
                Segment::Prose("Body.".into()),
            ]
        );
    }

    #[test]
    fn heading_with_inline_code_keeps_text() {
        assert_eq!(
            seg("## Use `foo()` now"),
            vec![Segment::Heading {
                level: 2,
                text: "Use foo() now".into()
            }]
        );
    }

    #[test]
    fn unclosed_fence_streams_as_code() {
        let s = "Look:\n\n```python\nprint('hi')\nx = ";
        assert_eq!(
            seg(s),
            vec![
                Segment::Prose("Look:".into()),
                Segment::Code {
                    lang: "python".into(),
                    code: "print('hi')\nx = ".into()
                },
            ]
        );
    }

    #[test]
    fn empty_input_yields_nothing() {
        assert_eq!(seg(""), vec![]);
        assert_eq!(seg("   \n"), vec![]);
    }

    #[test]
    fn code_without_language_tag() {
        let s = "```\nraw\n```";
        assert_eq!(
            seg(s),
            vec![Segment::Code {
                lang: "".into(),
                code: "raw".into()
            }]
        );
    }
}
