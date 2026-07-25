//! Syntax highlighting for code segments, rendered through Slint's StyledText.
//!
//! StyledText has no code-block construct, but it does support `<font color>`
//! spans. We highlight with syntect and emit one markdown paragraph whose
//! lines are joined with hard breaks, every span colored and every character
//! escaped so code renders literally. The colored markdown is a plain `String`
//! so it can cross threads; `StyledText::from_markdown` runs on the UI thread
//! (with a plain-text fallback on parse failure). Copy actions must always use
//! the raw code string, never the displayed text (escapes and NBSP
//! indentation would leak through).

use std::sync::OnceLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{Style, Theme, ThemeSet};
use syntect::parsing::SyntaxSet;

struct Assets {
    syntaxes: SyntaxSet,
    dark: Theme,
    light: Theme,
}

fn assets() -> &'static Assets {
    static ASSETS: OnceLock<Assets> = OnceLock::new();
    ASSETS.get_or_init(|| {
        let mut themes = ThemeSet::load_defaults();
        Assets {
            syntaxes: SyntaxSet::load_defaults_newlines(),
            dark: themes
                .themes
                .remove("base16-ocean.dark")
                .expect("bundled theme"),
            light: themes
                .themes
                .remove("InspiredGitHub")
                .expect("bundled theme"),
        }
    })
}

/// Highlight `code` into StyledText-compatible colored markdown.
pub fn code_markdown(code: &str, lang: &str, dark: bool) -> String {
    let assets = assets();
    let syntax = assets
        .syntaxes
        .find_syntax_by_token(lang)
        .unwrap_or_else(|| assets.syntaxes.find_syntax_plain_text());
    let theme = if dark { &assets.dark } else { &assets.light };
    let mut highlighter = HighlightLines::new(syntax, theme);

    let mut out = String::with_capacity(code.len() * 3);
    for (i, line) in code.lines().enumerate() {
        if i > 0 {
            // Hard line break within one paragraph.
            out.push_str("\\\n");
        }
        match highlighter.highlight_line(line, &assets.syntaxes) {
            Ok(spans) => {
                for (style, text) in spans {
                    push_span(&mut out, style, text);
                }
            }
            Err(_) => escape_into(&mut out, line),
        }
    }
    out
}

fn push_span(out: &mut String, style: Style, text: &str) {
    if text.is_empty() {
        return;
    }
    let c = style.foreground;
    out.push_str(&format!(
        "<font color=\"#{:02x}{:02x}{:02x}\">",
        c.r, c.g, c.b
    ));
    escape_into(out, text);
    out.push_str("</font>");
}

/// Escape so CommonMark + StyledText's HTML subset render `text` literally.
/// Leading/consecutive spaces become NBSP to survive markdown whitespace
/// collapsing (indentation matters in code).
fn escape_into(out: &mut String, text: &str) {
    let mut prev_space = true; // treat line start as after-space => NBSP for indentation
    for ch in text.chars() {
        match ch {
            ' ' => {
                if prev_space {
                    out.push('\u{00a0}');
                } else {
                    out.push(' ');
                }
                prev_space = true;
                continue;
            }
            '\t' => {
                out.push_str("\u{00a0}\u{00a0}\u{00a0}\u{00a0}");
                prev_space = true;
                continue;
            }
            c if c.is_ascii_punctuation() => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
        prev_space = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::StyledText;

    #[test]
    fn rust_code_parses_as_styled_markdown() {
        let md = code_markdown("fn main() { let x = 1; }", "rust", true);
        assert!(md.contains("<font color="));
        assert!(StyledText::from_markdown(&md).is_ok());
    }

    #[test]
    fn markdown_specials_stay_literal_and_parse() {
        let md = code_markdown("a = '**not bold**' # <tag> `tick`", "python", false);
        assert!(StyledText::from_markdown(&md).is_ok());
        // `**` must be escaped, not passed through as emphasis markers.
        assert!(md.contains("\\*\\*"));
    }

    #[test]
    fn unknown_language_falls_back_to_plain_syntax() {
        let md = code_markdown("whatever ^^ !!", "nosuchlang", true);
        assert!(StyledText::from_markdown(&md).is_ok());
    }

    #[test]
    fn escape_preserves_indentation() {
        let mut s = String::new();
        escape_into(&mut s, "    indented");
        assert!(s.starts_with("\u{00a0}\u{00a0}\u{00a0}\u{00a0}"));
    }

    #[test]
    fn multiline_uses_hard_breaks() {
        let md = code_markdown("a\nb", "", true);
        assert!(md.contains("\\\n"));
    }
}
