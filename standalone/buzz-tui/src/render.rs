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
const SPOILER: Color = Color::Magenta;

const SPOILER_MASK: &str = "▒▒▒▒▒";

/// Render a message body into styled lines, preserving structure rather than
/// flattening newlines. Spoilers (`||…||` inline, lone-`||` delimited blocks)
/// render revealed but visibly marked — the detail panel is the TUI's
/// deliberate "reveal" surface; previews stay masked.
pub fn render_message_body(content: &str) -> Vec<Line<'static>> {
    let raw_lines: Vec<&str> = content.split('\n').collect();
    let block_spoilers = block_spoiler_flags(&raw_lines);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut in_fence = false;
    let mut fence_is_diff = false;

    for (index, raw) in raw_lines.iter().enumerate() {
        let trimmed = raw.trim_start();

        if !in_fence {
            if block_spoilers.delimiter[index] {
                let label = if block_spoilers.opener[index] {
                    format!("{SPOILER_MASK} spoiler")
                } else {
                    SPOILER_MASK.to_string()
                };
                lines.push(Line::from(Span::styled(label, Style::new().fg(DIM))));
                continue;
            }
            if block_spoilers.inside[index] {
                lines.push(Line::from(vec![
                    Span::styled("▒ ", Style::new().fg(DIM)),
                    Span::styled(
                        raw.to_string(),
                        Style::new().fg(SPOILER).add_modifier(Modifier::ITALIC),
                    ),
                ]));
                continue;
            }
        }

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

/// Which lines belong to complete lone-`||` block-spoiler groups, computed
/// outside code fences. Unpaired delimiters are left to render literally.
struct BlockSpoilerFlags {
    delimiter: Vec<bool>,
    opener: Vec<bool>,
    inside: Vec<bool>,
}

fn block_spoiler_flags(lines: &[&str]) -> BlockSpoilerFlags {
    let mut delimiter = vec![false; lines.len()];
    let mut opener = vec![false; lines.len()];
    let mut inside = vec![false; lines.len()];
    let mut delimiters = Vec::new();
    let mut in_fence = false;
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence && trimmed == "||" {
            delimiters.push(index);
        }
    }
    for pair in delimiters.chunks_exact(2) {
        delimiter[pair[0]] = true;
        delimiter[pair[1]] = true;
        opener[pair[0]] = true;
        for flag in inside.iter_mut().take(pair[1]).skip(pair[0] + 1) {
            *flag = true;
        }
    }
    BlockSpoilerFlags {
        delimiter,
        opener,
        inside,
    }
}

/// Replace spoiler content with a fixed-width mask for one-line previews and
/// timeline rows, hiding both the text and its length.
pub fn mask_spoilers(content: &str) -> String {
    let raw_lines: Vec<&str> = content.split('\n').collect();
    let block_spoilers = block_spoiler_flags(&raw_lines);
    let mut out = Vec::new();
    let mut in_fence = false;
    for (index, line) in raw_lines.iter().enumerate() {
        if line.trim().starts_with("```") {
            in_fence = !in_fence;
            out.push((*line).to_string());
            continue;
        }
        if in_fence {
            out.push((*line).to_string());
            continue;
        }
        if block_spoilers.delimiter[index] {
            // Only the opening delimiter emits the mask marker.
            if block_spoilers.opener[index] {
                out.push(SPOILER_MASK.to_string());
            }
            continue;
        }
        if block_spoilers.inside[index] {
            continue;
        }
        out.push(mask_inline_spoilers(line));
    }
    out.join("\n")
}

fn mask_inline_spoilers(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    loop {
        let Some(open) = rest.find("||") else {
            out.push_str(rest);
            break;
        };
        let after = &rest[open + 2..];
        let Some(close) = after.find("||") else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..open]);
        out.push_str(SPOILER_MASK);
        rest = &after[close + 2..];
    }
    out
}

/// Render a one-line timeline preview that still advertises structure such as
/// code fences, diffs, media links, and multiline bodies.
pub fn render_message_preview(content: &str, max_chars: usize) -> Vec<Span<'static>> {
    let masked = mask_spoilers(content);
    let trimmed = masked.trim();
    if trimmed.is_empty() {
        return vec![Span::styled("(empty)", Style::new().fg(DIM))];
    }
    if let Some(link) = parse_markdown_link(trimmed) {
        return markdown_link_card(&link).spans;
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
    timestamp: &str,
    content: &str,
    panel_width: u16,
) -> Vec<Line<'static>> {
    let available_width = panel_width.saturating_sub(4) as usize;
    let indent_width = author.chars().count() + timestamp.chars().count() + 3;
    let body_width = available_width.saturating_sub(indent_width).max(10);

    let mut first_line = vec![
        Span::styled(format!(" {author} "), Style::new().fg(Color::Cyan)),
        Span::styled(format!("{timestamp} "), Style::new().fg(DIM)),
    ];
    first_line.extend(render_message_preview(content, body_width));

    let mut lines = vec![Line::from(first_line)];
    let masked = mask_spoilers(content);
    let wrapped_body = wrap_message_detail_lines(render_message_body(&masked), body_width);
    if wrapped_body.len() <= 1 {
        return lines;
    }

    let continuation_count = wrapped_body.len().saturating_sub(1);
    let truncated = continuation_count > SELECTED_TIMELINE_BODY_LINES;
    let indent = " ".repeat(indent_width);
    lines.extend(
        wrapped_body
            .into_iter()
            .skip(1)
            .take(SELECTED_TIMELINE_BODY_LINES)
            .map(|line| indented_rendered_line(&indent, line)),
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

/// A visible fragment of a terminal hyperlink after message-body wrapping.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct WrappedHyperlink {
    pub row: usize,
    pub column: usize,
    pub width: usize,
    pub text: String,
    pub url: String,
}

/// Render and wrap a message body while retaining the complete destination for
/// every visible link fragment.
///
/// Terminal URL auto-detection operates one visual row at a time. A long URL
/// therefore becomes several unrelated, incomplete targets after wrapping.
/// These fragments let the UI paint an explicit OSC 8 hyperlink over each row,
/// all pointing to the original complete URL.
pub fn render_message_body_with_hyperlinks(
    content: &str,
    width: usize,
) -> (Vec<Line<'static>>, Vec<WrappedHyperlink>) {
    let rendered = render_message_body(content);
    let mut wrapped_lines = Vec::new();
    let mut hyperlinks = Vec::new();

    for (raw, line) in content.split('\n').zip(rendered) {
        let targets = line_link_targets(raw, &line);
        let wrapped = wrap_styled_line(line, width.max(1));
        let row_offset = wrapped_lines.len();
        collect_wrapped_hyperlinks(&wrapped, &targets, row_offset, &mut hyperlinks);
        wrapped_lines.extend(wrapped);
    }

    (wrapped_lines, hyperlinks)
}

fn line_link_targets(raw: &str, line: &Line<'_>) -> Vec<Option<String>> {
    if let Some(link) = parse_markdown_link(raw.trim_start()) {
        return vec![safe_http_url(&link.url)];
    }

    line.spans
        .iter()
        .filter(|span| is_link_span(span))
        .map(|span| safe_http_url(span.content.as_ref()))
        .collect()
}

fn collect_wrapped_hyperlinks(
    lines: &[Line<'_>],
    targets: &[Option<String>],
    row_offset: usize,
    output: &mut Vec<WrappedHyperlink>,
) {
    let mut target_index = 0usize;
    let mut active_target = None;

    for (row, line) in lines.iter().enumerate() {
        let mut column = 0usize;
        for span in &line.spans {
            let span_width = span.width();
            if is_link_span(span) {
                let index = match active_target {
                    Some(index) => index,
                    None => {
                        let index = target_index;
                        target_index = target_index.saturating_add(1);
                        active_target = Some(index);
                        index
                    }
                };
                if let Some(Some(url)) = targets.get(index) {
                    output.push(WrappedHyperlink {
                        row: row_offset + row,
                        column,
                        width: span_width,
                        text: span.content.to_string(),
                        url: url.clone(),
                    });
                }
            } else if span_width > 0 {
                active_target = None;
            }
            column += span_width;
        }
    }
}

fn is_link_span(span: &Span<'_>) -> bool {
    span.style.fg == Some(LINK) && span.style.add_modifier.contains(Modifier::UNDERLINED)
}

fn safe_http_url(value: &str) -> Option<String> {
    let parsed = url::Url::parse(value).ok()?;
    if matches!(parsed.scheme(), "http" | "https") {
        Some(parsed.to_string())
    } else {
        None
    }
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

    // Render full-line Markdown links as compact cards. Showing the raw
    // `![label](very-long-url)` text makes terminals split the destination
    // into unrelated clickable fragments when it crosses the panel edge.
    if let Some(link) = parse_markdown_link(trimmed) {
        return markdown_link_card(&link);
    }

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

struct MarkdownLink {
    label: String,
    url: String,
    image: bool,
}

fn parse_markdown_link(text: &str) -> Option<MarkdownLink> {
    let (image, body) = match text.strip_prefix("![") {
        Some(body) => (true, body),
        None => (false, text.strip_prefix('[')?),
    };
    let (label, destination) = body.split_once("](")?;
    let url = destination.strip_suffix(')')?;
    if label.is_empty()
        || !(url.starts_with("https://") || url.starts_with("http://"))
        || url.chars().any(char::is_whitespace)
    {
        return None;
    }
    Some(MarkdownLink {
        label: label.to_string(),
        url: url.to_string(),
        image,
    })
}

fn markdown_link_card(link: &MarkdownLink) -> Line<'static> {
    let kind = if link.image {
        media_kind(&link.url)
    } else {
        "link"
    };
    Line::from(vec![
        Span::styled("📎 ", Style::new().fg(Color::Yellow)),
        Span::styled(
            format!("[{kind}] "),
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            link.label.clone(),
            Style::new().fg(LINK).add_modifier(Modifier::UNDERLINED),
        ),
    ])
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

fn indented_rendered_line(indent: &str, mut rendered: Line<'static>) -> Line<'static> {
    let mut spans = vec![Span::raw(indent.to_string())];
    spans.append(&mut rendered.spans);
    Line {
        style: rendered.style,
        alignment: rendered.alignment,
        spans,
    }
}

/// Split a line into spans, marking revealed `||…||` spoiler segments and
/// delegating the rest to plain inline highlighting.
fn inline_spans(text: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut rest = text;
    loop {
        let Some(open) = rest.find("||") else {
            spans.extend(plain_inline_spans(rest));
            break;
        };
        let after = &rest[open + 2..];
        let Some(close) = after.find("||") else {
            spans.extend(plain_inline_spans(rest));
            break;
        };
        if open > 0 {
            spans.extend(plain_inline_spans(&rest[..open]));
        }
        spans.push(Span::styled(
            format!("▒{}▒", &after[..close]),
            Style::new().fg(SPOILER).add_modifier(Modifier::ITALIC),
        ));
        rest = &after[close + 2..];
        if rest.is_empty() {
            break;
        }
    }
    if spans.is_empty() {
        spans.push(Span::raw(String::new()));
    }
    spans
}

/// Split a line into spans, highlighting `:emoji:` shortcodes and URLs.
fn plain_inline_spans(text: &str) -> Vec<Span<'static>> {
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
        let lines = render_selected_timeline_item("abc12345", "2:34 PM", "short body", 80);
        assert_eq!(lines.len(), 1);
        assert_eq!(line_text(&lines[0]), " abc12345 2:34 PM short body");
    }

    #[test]
    fn selected_timeline_item_wraps_long_body() {
        let lines = render_selected_timeline_item(
            "abc12345",
            "2:34 PM",
            "one two three four five six seven eight nine ten eleven",
            36,
        );

        assert!(lines.len() > 1);
        assert_eq!(line_text(&lines[0]), " abc12345 2:34 PM one two three …");
        assert!(line_text(&lines[1]).starts_with("                  "));
        assert!(line_text(&lines[1]).contains("four"));
    }

    #[test]
    fn selected_timeline_item_truncates_after_six_body_lines() {
        let body =
            "one two three four five six seven eight nine ten eleven twelve thirteen fourteen";
        let lines = render_selected_timeline_item("abc12345", "2:34 PM", body, 26);

        assert_eq!(lines.len(), 8);
        assert_eq!(
            line_text(lines.last().expect("footer line")),
            "                  … more in Message panel (PgDn/Ctrl-D)"
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
    fn markdown_image_link_renders_as_compact_card_without_splitting_url() {
        let url = format!("https://buzz.cashu.space/media/{}.png", "95".repeat(32));
        let lines = wrap_message_detail_lines(render_message_body(&format!("![image]({url})")), 24);

        assert_eq!(lines.len(), 1);
        assert_eq!(line_text(&lines[0]), "📎 [image] image");
    }

    #[test]
    fn timeline_preview_compacts_markdown_image_before_truncating() {
        let url = format!("https://buzz.cashu.space/media/{}.png", "95".repeat(32));
        let spans = render_message_preview(&format!("![image]({url})"), 18);

        assert_eq!(spans_text(&spans), "📎 [image] image");
        assert!(!spans_text(&spans).contains(&url));
    }

    #[test]
    fn selected_timeline_image_stays_on_one_compact_row() {
        let url = format!("https://buzz.cashu.space/media/{}.png", "95".repeat(32));
        let lines =
            render_selected_timeline_item("alice", "2:34 PM", &format!("![image]({url})"), 42);

        assert_eq!(lines.len(), 1);
        assert_eq!(line_text(&lines[0]), " alice 2:34 PM 📎 [image] image");
        assert!(!line_text(&lines[0]).contains(&url));
    }

    #[test]
    fn selected_timeline_multiline_body_compacts_image_continuation() {
        let url = format!("https://buzz.cashu.space/media/{}.png", "95".repeat(32));
        let lines = render_selected_timeline_item(
            "alice",
            "2:34 PM",
            &format!("caption\n![image]({url})"),
            42,
        );

        assert_eq!(lines.len(), 2);
        assert_eq!(line_text(&lines[1]), "               📎 [image] image");
        assert!(!line_text(&lines[1]).contains(&url));
    }

    #[test]
    fn markdown_image_card_retains_its_complete_hyperlink_target() {
        let url = format!("https://buzz.cashu.space/media/{}.png", "95".repeat(32));
        let (lines, links) = render_message_body_with_hyperlinks(&format!("![image]({url})"), 24);

        assert_eq!(line_text(&lines[0]), "📎 [image] image");
        assert_eq!(
            links,
            vec![WrappedHyperlink {
                row: 0,
                column: 11,
                width: 5,
                text: "image".to_string(),
                url,
            }]
        );
    }

    #[test]
    fn every_wrapped_url_fragment_targets_the_complete_url() {
        let url = format!("https://example.com/{}.png", "ab".repeat(24));
        let (lines, links) = render_message_body_with_hyperlinks(&url, 20);

        assert!(lines.len() > 1);
        assert_eq!(links.len(), lines.len());
        assert!(links.iter().all(|link| link.url == url));
        assert_eq!(
            links
                .iter()
                .map(|link| link.text.as_str())
                .collect::<String>(),
            url
        );
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

    #[test]
    fn preview_masks_inline_spoilers() {
        let spans = render_message_preview("the killer is ||the butler|| apparently", 80);
        let text = spans_text(&spans);
        assert!(text.contains("▒▒▒▒▒"));
        assert!(!text.contains("butler"));
    }

    #[test]
    fn preview_masks_block_spoilers() {
        let spans = render_message_preview("||\nsecret line\n||", 80);
        let text = spans_text(&spans);
        assert!(text.contains("▒▒▒▒▒"));
        assert!(!text.contains("secret"));
    }

    #[test]
    fn mask_spoilers_leaves_unpaired_delimiters_alone() {
        assert_eq!(mask_spoilers("a || b"), "a || b");
        assert_eq!(mask_spoilers("||\nno closing"), "||\nno closing");
    }

    #[test]
    fn mask_spoilers_ignores_code_fences() {
        let body = "```\na || b || c\n```";
        assert_eq!(mask_spoilers(body), body);
    }

    #[test]
    fn message_body_reveals_inline_spoiler_with_marker() {
        let lines = render_message_body("reveal ||the butler|| now");
        assert_eq!(line_text(&lines[0]), "reveal ▒the butler▒ now");
    }

    #[test]
    fn message_body_reveals_block_spoiler_lines() {
        let lines = render_message_body("||\nsecret\n||");
        assert_eq!(lines.len(), 3);
        assert_eq!(line_text(&lines[0]), "▒▒▒▒▒ spoiler");
        assert_eq!(line_text(&lines[1]), "▒ secret");
        assert_eq!(line_text(&lines[2]), "▒▒▒▒▒");
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
