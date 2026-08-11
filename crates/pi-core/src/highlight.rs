//! Syntax highlighting for code segments.
//!
//! Highlights with syntect and returns one [`CodeLine`] (a `Vec<ColoredSpan>`)
//! per source line — plain data, cheap to compute off the UI thread and
//! trivial to turn into Slint's per-span model (a nested `for line in
//! code-lines: for span in line.spans: Text { color: span.color }`), unlike
//! routing colored code through `StyledText::from_markdown`, which needed
//! every span escaped and indentation faked with NBSP to survive markdown
//! parsing. Copy actions use the original code string directly; nothing here
//! is lossy or needs unescaping.

use std::sync::OnceLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{Style, Theme, ThemeSet};
use syntect::parsing::SyntaxSet;

#[derive(Debug, Clone, PartialEq)]
pub struct ColoredSpan {
    pub text: String,
    pub color: (u8, u8, u8),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodeLine {
    pub spans: Vec<ColoredSpan>,
}

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

/// The background color the active syntect theme was designed against.
/// Code cards must use this so span colors keep their intended contrast.
pub fn theme_background(dark: bool) -> (u8, u8, u8) {
    let assets = assets();
    let theme = if dark { &assets.dark } else { &assets.light };
    theme
        .settings
        .background
        .map(|c| (c.r, c.g, c.b))
        .unwrap_or(if dark { (43, 48, 59) } else { (255, 255, 255) })
}

/// Highlight `code` into one [`CodeLine`] per source line.
pub fn highlight_lines(code: &str, lang: &str, dark: bool) -> Vec<CodeLine> {
    let assets = assets();
    let syntax = assets
        .syntaxes
        .find_syntax_by_token(lang)
        .unwrap_or_else(|| assets.syntaxes.find_syntax_plain_text());
    let theme = if dark { &assets.dark } else { &assets.light };
    let mut highlighter = HighlightLines::new(syntax, theme);

    // Slint's `Text` renders tabs as tofu glyphs, not tab stops.
    let code = code.replace('\t', "    ");
    code.lines()
        .map(|line| {
            let spans = match highlighter.highlight_line(line, &assets.syntaxes) {
                Ok(spans) => spans
                    .into_iter()
                    .filter(|(_, text)| !text.is_empty())
                    .map(|(style, text)| colored_span(style, text))
                    .collect(),
                Err(_) => vec![ColoredSpan {
                    text: line.to_string(),
                    color: theme
                        .settings
                        .foreground
                        .map(|c| (c.r, c.g, c.b))
                        .unwrap_or(if dark { (192, 197, 206) } else { (36, 41, 46) }),
                }],
            };
            CodeLine { spans }
        })
        .collect()
}

fn colored_span(style: Style, text: &str) -> ColoredSpan {
    let c = style.foreground;
    ColoredSpan {
        text: text.to_string(),
        color: (c.r, c.g, c.b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_code_produces_colored_spans() {
        let lines = highlight_lines("fn main() { let x = 1; }", "rust", true);
        assert_eq!(lines.len(), 1);
        assert!(!lines[0].spans.is_empty());
        // Highlighted code isn't a single uniformly-colored run.
        let colors: std::collections::HashSet<_> = lines[0].spans.iter().map(|s| s.color).collect();
        assert!(colors.len() > 1);
    }

    #[test]
    fn span_text_concatenates_back_to_the_source_line() {
        let code = "a = '**not bold**' # <tag> `tick`";
        let lines = highlight_lines(code, "python", false);
        let rebuilt: String = lines[0].spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(rebuilt, code);
    }

    #[test]
    fn unknown_language_falls_back_to_plain_syntax() {
        let lines = highlight_lines("whatever ^^ !!", "nosuchlang", true);
        assert_eq!(lines.len(), 1);
        let rebuilt: String = lines[0].spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(rebuilt, "whatever ^^ !!");
    }

    #[test]
    fn theme_backgrounds_differ_by_scheme() {
        let dark = theme_background(true);
        let light = theme_background(false);
        assert_ne!(dark, light);
        // Dark theme background must actually be dark, light light.
        assert!((dark.0 as u16 + dark.1 as u16 + dark.2 as u16) < 300);
        assert!((light.0 as u16 + light.1 as u16 + light.2 as u16) > 600);
    }

    #[test]
    fn multiline_code_produces_one_code_line_per_source_line() {
        let lines = highlight_lines("a\nb", "", true);
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn tabs_are_expanded_to_spaces() {
        let lines = highlight_lines("\tindented", "", true);
        let rebuilt: String = lines[0].spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(rebuilt, "    indented");
    }
}
