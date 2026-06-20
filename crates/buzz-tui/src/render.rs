//! Deterministic terminal rendering of message bodies. Kept separate from the
//! layout code in [`crate::ui`] so timeline rows stay compact while detail
//! panels show structured markdown, code, diffs, emoji, and media cards.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

const SELECTED_TIMELINE_BODY_LINES: usize = 6;

const CODE: Color = Color::LightCyan;
const QUOTE: Color = Color::Gray;
const HEADING: Color = Color::White;
const EMOJI: Color = Color::Magenta;
const LINK: Color = Color::Blue;
const ADD: Color = Color::Green;
const REMOVE: Color = Color::Red;
const HUNK: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;

/// Render a message body into styled lines, preserving structure rather than
/// flattening newlines.
pub fn render_message_body(content: &str) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut in_fence = false;
    let mut fence_is_diff = false;

    for raw in content.split('\n') {
        let trimmed = raw.trim_start();

        // Fenced code blocks: ``` optionally followed by a language.
        if trimmed.starts_with("```") {
            if in_fence {
                in_fence = false;
                fence_is_diff = false;
            } else {
                in_fence = true;
                let lang = trimmed.trim_start_matches('`').trim().to_lowercase();
                fence_is_diff = lang == "diff" || lang == "patch";
            }
            lines.push(Line::from(Span::styled(
                raw.to_string(),
                Style::new().fg(DIM),
            )));
            continue;
        }

        if in_fence {
            lines.push(code_line(raw, fence_is_diff));
            continue;
        }

        lines.push(markdown_line(raw));
    }

    lines
}

/// Render a one-line timeline preview that still advertises structure such as
/// code fences, diffs, media links, and multiline bodies.
pub fn render_message_preview(content: &str, max_chars: usize) -> Vec<Span<'static>> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return vec![Span::styled("(empty)", Style::new().fg(DIM))];
    }

    let mut spans = Vec::new();
    if is_diff_body(trimmed) {
        spans.push(Span::styled("[diff] ", Style::new().fg(HUNK)));
    } else if trimmed.contains("```") {
        spans.push(Span::styled("[code] ", Style::new().fg(CODE)));
    } else if is_standalone_url(trimmed) {
        spans.push(Span::styled(
            format!("[{}] ", media_kind(trimmed)),
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ));
    }

    let mut preview = trimmed
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(trimmed)
        .trim()
        .to_string();
    let has_more_lines = trimmed
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count()
        > 1;
    preview = compact_text(&preview, max_chars);
    spans.extend(inline_spans(&preview));
    if has_more_lines {
        spans.push(Span::styled(" +more", Style::new().fg(DIM)));
    }
    spans
}

/// Render the selected timeline item with inline continuation lines while
/// keeping the first row compatible with compact timeline previews.
pub fn render_selected_timeline_item(
    author: &str,
    content: &str,
    panel_width: u16,
) -> Vec<Line<'static>> {
    let available_width = panel_width.saturating_sub(4) as usize;
    let indent_width = author.chars().count() + 2;
    let body_width = available_width.saturating_sub(indent_width).max(10);

    let mut first_line = vec![Span::styled(
        format!(" {author} "),
        Style::new().fg(Color::Cyan),
    )];
    first_line.extend(render_message_preview(content, body_width));

    let mut lines = vec![Line::from(first_line)];
    let wrapped_body = wrap_timeline_body(content, body_width);
    if wrapped_body.len() <= 1 {
        return lines;
    }

    let continuation_count = wrapped_body.len().saturating_sub(1);
    let truncated = continuation_count > SELECTED_TIMELINE_BODY_LINES;
    let indent = " ".repeat(indent_width);
    lines.extend(
        wrapped_body
            .iter()
            .skip(1)
            .take(SELECTED_TIMELINE_BODY_LINES)
            .map(|line| indented_body_line(&indent, line)),
    );

    if truncated {
        lines.push(Line::from(vec![
            Span::raw(indent),
            Span::styled(
                "… more in Message panel (PgDn/Ctrl-D)",
                Style::new().fg(DIM),
            ),
        ]));
    }

    lines
}

/// Wrap already-rendered message detail lines to the terminal width so scroll
/// bounds match the rows Ratatui will draw.
pub fn wrap_message_detail_lines(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    lines
        .into_iter()
        .flat_map(|line| wrap_styled_line(line, width))
        .collect()
}

fn wrap_styled_line(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    let line_style = line.style;
    let alignment = line.alignment;
    let mut rows = Vec::new();
    let mut current_spans: Vec<Span<'static>> = Vec::new();
    let mut current_width = 0;

    for span in line.spans {
        let span_style = span.style;
        let mut chunk = String::new();

        for ch in span.content.chars() {
            if current_width == width {
                if !chunk.is_empty() {
                    current_spans.push(Span::styled(std::mem::take(&mut chunk), span_style));
                }
                rows.push(Line {
                    style: line_style,
                    alignment,
                    spans: std::mem::take(&mut current_spans),
                });
                current_width = 0;
            }

            chunk.push(ch);
            current_width += 1;
        }

        if !chunk.is_empty() {
            current_spans.push(Span::styled(chunk, span_style));
        }
    }

    if !current_spans.is_empty() || rows.is_empty() {
        rows.push(Line {
            style: line_style,
            alignment,
            spans: current_spans,
        });
    }

    rows
}

fn code_line(raw: &str, is_diff: bool) -> Line<'static> {
    let style = if is_diff {
        diff_style(raw)
    } else {
        Style::new().fg(CODE)
    };
    Line::from(vec![
        Span::styled("│ ", Style::new().fg(DIM)),
        Span::styled(raw.to_string(), style),
    ])
}

fn diff_style(line: &str) -> Style {
    if line.starts_with("@@") {
        Style::new().fg(HUNK)
    } else if line.starts_with('+') {
        Style::new().fg(ADD)
    } else if line.starts_with('-') {
        Style::new().fg(REMOVE)
    } else {
        Style::new().fg(CODE)
    }
}

fn markdown_line(raw: &str) -> Line<'static> {
    let trimmed = raw.trim_start();

    if trimmed.starts_with("# ") || trimmed.starts_with("## ") || trimmed.starts_with("### ") {
        return Line::from(Span::styled(
            raw.to_string(),
            Style::new().fg(HEADING).add_modifier(Modifier::BOLD),
        ));
    }

    if let Some(rest) = trimmed.strip_prefix("> ") {
        return Line::from(vec![
            Span::styled("│ ", Style::new().fg(DIM)),
            Span::styled(
                rest.to_string(),
                Style::new().fg(QUOTE).add_modifier(Modifier::ITALIC),
            ),
        ]);
    }

    if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("+ ") {
        let body = &trimmed[2..];
        let mut spans = vec![Span::styled("• ", Style::new().fg(Color::Yellow))];
        spans.extend(inline_spans(body));
        return Line::from(spans);
    }

    // A standalone URL reads as an attachment/media card.
    if is_standalone_url(trimmed) {
        return media_card(trimmed);
    }

    Line::from(inline_spans(raw))
}

fn media_card(url: &str) -> Line<'static> {
    let kind = media_kind(url);
    Line::from(vec![
        Span::styled("📎 ", Style::new().fg(Color::Yellow)),
        Span::styled(
            format!("[{kind}] "),
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            url.to_string(),
            Style::new().fg(LINK).add_modifier(Modifier::UNDERLINED),
        ),
    ])
}

fn media_kind(url: &str) -> &'static str {
    let lower = url.to_lowercase();
    let path = lower.split(['?', '#']).next().unwrap_or(&lower);
    if path.ends_with(".png")
        || path.ends_with(".jpg")
        || path.ends_with(".jpeg")
        || path.ends_with(".gif")
        || path.ends_with(".webp")
    {
        "image"
    } else if path.ends_with(".mp4") || path.ends_with(".mov") || path.ends_with(".webm") {
        "video"
    } else if path.ends_with(".mp3") || path.ends_with(".ogg") || path.ends_with(".wav") {
        "audio"
    } else if path.ends_with(".pdf") {
        "pdf"
    } else {
        "link"
    }
}

fn indented_body_line(indent: &str, line: &str) -> Line<'static> {
    let mut rendered = render_message_body(line)
        .into_iter()
        .next()
        .unwrap_or_else(|| Line::from(String::new()));
    let mut spans = vec![Span::raw(indent.to_string())];
    spans.append(&mut rendered.spans);
    Line {
        style: rendered.style,
        alignment: rendered.alignment,
        spans,
    }
}

fn wrap_timeline_body(content: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    content
        .trim()
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        })
        .flat_map(|line| wrap_line(line, width))
        .collect()
}

fn wrap_line(line: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();

    for word in line.split_whitespace() {
        let word_len = word.chars().count();
        if current.is_empty() {
            if word_len <= width {
                current.push_str(word);
            } else {
                push_long_word_chunks(word, width, &mut lines, &mut current);
            }
            continue;
        }

        let current_len = current.chars().count();
        if current_len + 1 + word_len <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            if word_len <= width {
                current.push_str(word);
            } else {
                push_long_word_chunks(word, width, &mut lines, &mut current);
            }
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn push_long_word_chunks(word: &str, width: usize, lines: &mut Vec<String>, current: &mut String) {
    let mut chunk = String::new();
    for ch in word.chars() {
        if chunk.chars().count() == width {
            lines.push(std::mem::take(&mut chunk));
        }
        chunk.push(ch);
    }
    *current = chunk;
}

/// Split a line into spans, highlighting `:emoji:` shortcodes and URLs.
fn inline_spans(text: &str) -> Vec<Span<'static>> {
    if text.is_empty() {
        return vec![Span::raw(String::new())];
    }
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (index, word) in text.split(' ').enumerate() {
        if index > 0 {
            spans.push(Span::raw(" "));
        }
        if word.is_empty() {
            continue;
        }
        if word.starts_with("http://") || word.starts_with("https://") {
            spans.push(Span::styled(
                word.to_string(),
                Style::new().fg(LINK).add_modifier(Modifier::UNDERLINED),
            ));
        } else if is_emoji_shortcode(word) {
            spans.push(Span::styled(word.to_string(), Style::new().fg(EMOJI)));
        } else {
            spans.push(Span::raw(word.to_string()));
        }
    }
    spans
}

fn is_standalone_url(text: &str) -> bool {
    (text.starts_with("http://") || text.starts_with("https://")) && !text.contains(' ')
}

fn is_emoji_shortcode(word: &str) -> bool {
    let inner = word.strip_prefix(':').and_then(|w| w.strip_suffix(':'));
    match inner {
        Some(name) => {
            !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        }
        None => false,
    }
}

fn is_diff_body(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.starts_with("```diff")
        || lower.starts_with("```patch")
        || text.lines().any(|line| line.starts_with("@@"))
}

fn compact_text(value: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if out.chars().count() >= max_chars {
            out.push('…');
            return out;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_line_count() {
        let body = "line one\nline two\nline three";
        assert_eq!(render_message_body(body).len(), 3);
    }

    #[test]
    fn renders_code_fence_without_dropping_lines() {
        let body = "intro\n```rust\nlet x = 1;\n```\noutro";
        // 1 intro + open fence + 1 code + close fence + 1 outro = 5
        assert_eq!(render_message_body(body).len(), 5);
    }

    #[test]
    fn detects_media_kinds() {
        assert_eq!(media_kind("https://x.com/a.png"), "image");
        assert_eq!(media_kind("https://x.com/a.mp4?t=1"), "video");
        assert_eq!(media_kind("https://x.com/file"), "link");
    }

    #[test]
    fn recognizes_emoji_shortcodes() {
        assert!(is_emoji_shortcode(":smile:"));
        assert!(!is_emoji_shortcode("ratio:"));
        assert!(!is_emoji_shortcode("plain"));
    }

    #[test]
    fn preview_marks_multiline_code() {
        let spans = render_message_preview("```rust\nlet x = 1;\n```", 80);
        assert_eq!(
            spans.first().map(|span| span.content.as_ref()),
            Some("[code] ")
        );
        assert!(spans.iter().any(|span| span.content.as_ref() == " +more"));
    }

    #[test]
    fn preview_marks_diff() {
        let spans = render_message_preview("```diff\n+added\n```", 80);
        assert_eq!(
            spans.first().map(|span| span.content.as_ref()),
            Some("[diff] ")
        );
    }

    #[test]
    fn selected_timeline_item_keeps_single_line_compact() {
        let lines = render_selected_timeline_item("abc12345", "short body", 80);
        assert_eq!(lines.len(), 1);
        assert_eq!(line_text(&lines[0]), " abc12345 short body");
    }

    #[test]
    fn selected_timeline_item_wraps_long_body() {
        let lines = render_selected_timeline_item(
            "abc12345",
            "one two three four five six seven eight nine ten eleven",
            28,
        );

        assert!(lines.len() > 1);
        assert_eq!(line_text(&lines[0]), " abc12345 one two three …");
        assert!(line_text(&lines[1]).starts_with("          "));
        assert!(line_text(&lines[1]).contains("four"));
    }

    #[test]
    fn selected_timeline_item_truncates_after_six_body_lines() {
        let body =
            "one two three four five six seven eight nine ten eleven twelve thirteen fourteen";
        let lines = render_selected_timeline_item("abc12345", body, 18);

        assert_eq!(lines.len(), 8);
        assert_eq!(
            line_text(lines.last().expect("footer line")),
            "          … more in Message panel (PgDn/Ctrl-D)"
        );
    }

    #[test]
    fn message_detail_wraps_single_long_paragraph() {
        let lines =
            wrap_message_detail_lines(render_message_body("one two three four five six seven"), 10);

        assert!(lines.len() > 1);
        assert_eq!(line_text(&lines[0]), "one two th");
        assert_eq!(line_text(&lines[1]), "ree four f");
    }

    #[test]
    fn message_detail_wraps_long_unbroken_word() {
        let lines = wrap_message_detail_lines(render_message_body("abcdefghijklmnop"), 5);

        assert_eq!(lines.len(), 4);
        assert_eq!(line_text(&lines[0]), "abcde");
        assert_eq!(line_text(&lines[3]), "p");
    }

    #[test]
    fn message_detail_preserves_explicit_newlines() {
        let lines = wrap_message_detail_lines(render_message_body("alpha\nbeta"), 80);

        assert_eq!(lines.len(), 2);
        assert_eq!(line_text(&lines[0]), "alpha");
        assert_eq!(line_text(&lines[1]), "beta");
    }

    #[test]
    fn wrapped_message_detail_can_scroll_when_visual_rows_overflow() {
        let mut lines = vec![Line::from("event abc"), Line::from("")];
        lines.extend(render_message_body(
            "one two three four five six seven eight nine",
        ));
        let lines = wrap_message_detail_lines(lines, 8);
        let visible_rows = 4usize;
        let max_scroll = lines.len().saturating_sub(visible_rows.max(1));

        assert!(max_scroll > 0);
    }

    #[test]
    fn non_selected_preview_remains_one_line() {
        let spans = render_message_preview("one\ntwo\nthree", 80);
        assert_eq!(spans_text(&spans), "one +more");
    }

    fn line_text(line: &Line<'_>) -> String {
        spans_text(&line.spans)
    }

    fn spans_text(spans: &[Span<'_>]) -> String {
        spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }
}
