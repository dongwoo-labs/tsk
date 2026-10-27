//! Mono markdown for read-only bodies: task notes (task page, wide detail) and the
//! questions tab's record pages (question text, finding detail, impact, approval terms,
//! answers, comments, history).
//!
//! Markers are styled with bold / dim / underline only, never colour: the mono set in
//! [`super::render::MONO_MODIFIERS`]. Edit mode keeps the raw source.
//!
//! [`render`] is the reading surface. It parses each source line into styled rendered
//! characters first and only then wraps them with the one wrap engine
//! ([`super::edit::wrap_text`]), so rows wrap on the rendered width, never on markers
//! that are hidden, and a wrapped list item, quote, or table row keeps its hanging
//! indent or `│ ` bar on every row. Every row it returns fits the width it was given.
//!
//! Supported inline: `**strong**` (bold), `*em*` / `_em_` (underline), `` `code` ``
//! (dim, ticks kept), `~~strike~~` (dim: crossed-out text is outside the mono set),
//! `[text](url)` (text underlined, url dim in parentheses; only the url when both are
//! equal), `<https://…>` autolinks (the url, underlined), and backslash escapes (`\*`
//! is a literal `*`).
//!
//! Supported line starts: `#`…`######` headings (marker hidden, bold+underline body),
//! `-` / `*` / `+` lists (dim marker), `1.` / `1)` ordered lists (dim number),
//! `- [ ]` / `- [x]` task-list items (`☐` / `☑`; a checked item dims), `>` quotes (a dim
//! `│ ` bar per level), `---` / `***` / `___` rules (a dim `─` rule across the width),
//! `| a | b |` pipe tables (dim `│` separators, the header row bold, the `|---|` divider
//! dropped), and fenced ` ``` ` blocks (dim fence, dim unparsed body). Leading spaces
//! (nested lists) are kept and continuation rows hang under the item's text.
//!
//! [`paint_notes_line`] / [`paint_md_line`] are the per-row painter for a task page in
//! an edit session: there the notes rows must stay the raw wrap rows, because a click
//! on one maps straight into the draft. It dims line-start markers in place and styles
//! inline spans, but never re-wraps.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::edit::wrap_text;
use super::render::{style_dim, style_heading, style_plain};
use super::{present_line, split_line_breaks, terminal_text};

/// One rendered character and its style.
type Cell = (char, Style);

/// A line that opens or closes a fenced code block (` ``` ` after indent).
pub fn is_fence_line(text: &str) -> bool {
    text.trim_start().starts_with("```")
}

/// Render `source` as wrapped, styled rows no wider than `width` cells.
///
/// `base` is the body style (plain, or bold for a bold field). A non-zero width always
/// yields at least one row; an empty source is one blank row.
pub fn render(source: &str, width: usize, base: Style) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    if width == 0 {
        return out;
    }
    let lines: Vec<&str> = split_line_breaks(source).collect();
    let mut in_fence = false;
    for (index, line) in lines.iter().enumerate() {
        let fence = is_fence_line(line);
        if fence || in_fence {
            in_fence ^= fence;
            let body = cells(line.trim_end(), style_dim());
            wrap_block(&Block::text(Vec::new(), Vec::new(), body), width, &mut out);
            continue;
        }
        let previous = index.checked_sub(1).map(|at| lines[at]);
        if is_table_divider(line) && previous.is_some_and(is_table_row) {
            continue;
        }
        let header = is_table_row(line)
            && lines
                .get(index + 1)
                .is_some_and(|next| is_table_divider(next));
        wrap_block(&block_of(line.trim_end(), base, header), width, &mut out);
    }
    if out.is_empty() {
        out.push(Line::from(""));
    }
    out
}

/// The text of a rendered row exactly as painted (copy reads the same cells).
pub fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

/// One source line, parsed: the prefix of its first wrapped row, the prefix of every
/// continuation row, and the rendered body. A rule fills the row instead of a body.
struct Block {
    first: Vec<Cell>,
    cont: Vec<Cell>,
    body: Vec<Cell>,
    rule: bool,
}

impl Block {
    fn text(first: Vec<Cell>, cont: Vec<Cell>, body: Vec<Cell>) -> Self {
        Self {
            first,
            cont,
            body,
            rule: false,
        }
    }

    /// Put `first` / `cont` before the matching prefixes.
    fn led(mut self, first: &[Cell], cont: &[Cell]) -> Self {
        self.first.splice(0..0, first.iter().copied());
        self.cont.splice(0..0, cont.iter().copied());
        self
    }
}

fn cells(text: &str, style: Style) -> Vec<Cell> {
    text.chars().map(|c| (c, style)).collect()
}

fn cells_width(cells: &[Cell]) -> usize {
    let text: String = cells.iter().map(|(c, _)| *c).collect();
    Line::from(terminal_text(&text)).width()
}

fn block_of(line: &str, base: Style, table_header: bool) -> Block {
    let indent_len = line.len() - line.trim_start_matches(' ').len();
    let (indent, rest) = line.split_at(indent_len);
    let lead = cells(indent, style_plain());
    if rest.is_empty() {
        return Block::text(lead.clone(), lead, Vec::new());
    }
    if let Some(inner) = rest.strip_prefix('>') {
        let inner = inner.strip_prefix(' ').unwrap_or(inner);
        let mut barred = lead;
        barred.extend(cells("│ ", style_dim()));
        return block_of(inner, base, false).led(&barred, &barred);
    }
    if is_rule(rest) {
        return Block {
            first: lead.clone(),
            cont: lead,
            body: Vec::new(),
            rule: true,
        };
    }
    if let Some(body) = heading_body(rest) {
        return Block::text(lead.clone(), lead, inline(body, style_heading()));
    }
    if let Some((checked, body)) = task_item(rest) {
        let (glyph, style) = if checked {
            ("☑ ", base.add_modifier(Modifier::DIM))
        } else {
            ("☐ ", base)
        };
        let mut first = lead.clone();
        first.extend(cells(glyph, style_plain()));
        let mut cont = lead;
        cont.extend(cells("  ", style_plain()));
        return Block::text(first, cont, inline(body, style));
    }
    if let Some(marker) = list_marker(rest) {
        let mut first = lead.clone();
        first.extend(cells(&rest[..marker], style_dim()));
        let mut cont = lead;
        cont.extend(cells(&" ".repeat(marker), style_plain()));
        return Block::text(first, cont, inline(&rest[marker..], base));
    }
    if is_table_row(rest) {
        let style = if table_header {
            base.add_modifier(Modifier::BOLD)
        } else {
            base
        };
        return Block::text(lead.clone(), lead, table_cells(rest, style));
    }
    Block::text(lead.clone(), lead, inline(rest, base))
}

/// Wrap one block's body under its prefixes and push the painted rows.
fn wrap_block(block: &Block, width: usize, out: &mut Vec<Line<'static>>) {
    let mut first = block.first.clone();
    let mut cont = block.cont.clone();
    // A prefix that leaves no room for the body is dropped rather than overflowing.
    if cells_width(&first).max(cells_width(&cont)) >= width {
        first.clear();
        cont.clear();
    }
    if block.rule {
        let room = width - cells_width(&first);
        let mut row = first;
        row.extend(cells(&"─".repeat(room), style_dim()));
        out.push(spans_of(&row));
        return;
    }
    let room = width - cells_width(&first).max(cells_width(&cont));
    let text: String = block.body.iter().map(|(c, _)| *c).collect();
    for (index, row) in wrap_text(&text, room).iter().enumerate() {
        let mut painted = if index == 0 {
            first.clone()
        } else {
            cont.clone()
        };
        // `cell_of` holds one entry per raw character living on this row.
        let from = row.first_raw.min(block.body.len());
        let to = (row.first_raw + row.cell_of.len()).min(block.body.len());
        painted.extend(block.body[from..to].iter().copied());
        out.push(spans_of(&painted));
    }
}

/// Merge runs of one style into spans, escaping control characters as the wrap did.
fn spans_of(row: &[Cell]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut current: Option<Style> = None;
    for (c, style) in row {
        if current != Some(*style) {
            if let Some(style) = current {
                spans.push(Span::styled(std::mem::take(&mut buf), style));
            }
            current = Some(*style);
        }
        buf.push_str(&terminal_text(&c.to_string()));
    }
    if let Some(style) = current {
        spans.push(Span::styled(buf, style));
    }
    Line::from(spans)
}

/// `---`, `***`, `___`: three or more of one character, spaces allowed between.
fn is_rule(rest: &str) -> bool {
    let marks: Vec<char> = rest.chars().filter(|c| *c != ' ').collect();
    marks.len() >= 3 && matches!(marks[0], '-' | '*' | '_') && marks.iter().all(|c| *c == marks[0])
}

fn heading_body(rest: &str) -> Option<&str> {
    let hashes = rest.len() - rest.trim_start_matches('#').len();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let after = &rest[hashes..];
    if after.is_empty() {
        return Some("");
    }
    after
        .strip_prefix(' ')
        .map(|body| body.trim_start_matches(' '))
}

/// `- [ ] x` / `- [x] x` under any bullet: whether it is checked, and the body.
fn task_item(rest: &str) -> Option<(bool, &str)> {
    let bytes = rest.as_bytes();
    if bytes.len() < 5 || !matches!(bytes[0], b'-' | b'*' | b'+') || bytes[1] != b' ' {
        return None;
    }
    let checked = match rest.get(2..5)? {
        "[ ]" => false,
        "[x]" | "[X]" => true,
        _ => return None,
    };
    let after = &rest[5..];
    if after.is_empty() {
        return Some((checked, ""));
    }
    after.strip_prefix(' ').map(|body| (checked, body))
}

/// Byte length of a list marker and its space: `- `, `* `, `+ `, `12. `, `3) `.
fn list_marker(rest: &str) -> Option<usize> {
    let bytes = rest.as_bytes();
    if bytes.len() >= 2 && matches!(bytes[0], b'-' | b'*' | b'+') && bytes[1] == b' ' {
        return Some(2);
    }
    let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
    if (1..=9).contains(&digits)
        && bytes.len() > digits + 1
        && matches!(bytes[digits], b'.' | b')')
        && bytes[digits + 1] == b' '
    {
        return Some(digits + 2);
    }
    None
}

fn is_table_row(line: &str) -> bool {
    let rest = line.trim();
    rest.starts_with('|') && rest.len() > 1 && rest[1..].contains('|')
}

/// `|---|:--:|` and the like: only pipes, dashes, colons, and spaces, with a dash.
fn is_table_divider(line: &str) -> bool {
    let rest = line.trim();
    rest.starts_with('|')
        && rest.contains('-')
        && rest.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

/// Cells split on unescaped pipes, trimmed, joined by a dim ` │ `.
fn table_cells(rest: &str, base: Style) -> Vec<Cell> {
    let inner = rest.trim();
    let inner = inner.strip_prefix('|').unwrap_or(inner);
    let inner = match inner.strip_suffix('|') {
        Some(stripped) if !stripped.ends_with('\\') => stripped,
        _ => inner,
    };
    let mut parts: Vec<String> = vec![String::new()];
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        let part = parts.last_mut().expect("parts is never empty");
        if c == '\\' && chars.peek() == Some(&'|') {
            part.push_str("\\|");
            chars.next();
        } else if c == '|' {
            parts.push(String::new());
        } else {
            part.push(c);
        }
    }
    let mut out = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            out.extend(cells(" │ ", style_dim()));
        }
        out.extend(inline(part.trim(), base));
    }
    out
}

/// Inline markdown over one line: markers hidden, bodies styled.
fn inline(text: &str, base: Style) -> Vec<Cell> {
    let chars: Vec<char> = text.chars().collect();
    inline_chars(&chars, base)
}

fn inline_chars(chars: &[char], base: Style) -> Vec<Cell> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '\\' && next.is_some_and(|n| n.is_ascii_punctuation()) {
            out.push((chars[i + 1], base));
            i += 2;
        } else if c == '`' {
            match chars[i + 1..].iter().position(|n| *n == '`') {
                Some(offset) => {
                    let close = i + 1 + offset;
                    out.extend(chars[i..=close].iter().map(|c| (*c, style_dim())));
                    i = close + 1;
                }
                None => {
                    out.push((c, base));
                    i += 1;
                }
            }
        } else if let Some(close) = (matches!(c, '*' | '~') && next == Some(c))
            .then(|| find_pair(chars, i + 2, c))
            .flatten()
            .filter(|close| *close > i + 2)
        {
            let modifier = if c == '*' {
                Modifier::BOLD
            } else {
                Modifier::DIM
            };
            out.extend(inline_chars(
                &chars[i + 2..close],
                base.add_modifier(modifier),
            ));
            i = close + 2;
        } else if let Some(close) = em_close(chars, i) {
            out.extend(inline_chars(
                &chars[i + 1..close],
                base.add_modifier(Modifier::UNDERLINED),
            ));
            i = close + 1;
        } else if let Some((label, url, end)) = link_at(chars, i) {
            let underline = base.add_modifier(Modifier::UNDERLINED);
            if label.is_empty() || label == url {
                out.extend(url.iter().map(|c| (*c, underline)));
            } else {
                out.extend(inline_chars(label, underline));
                out.push((' ', style_dim()));
                out.push(('(', style_dim()));
                out.extend(url.iter().map(|c| (*c, style_dim())));
                out.push((')', style_dim()));
            }
            i = end;
        } else if let Some((url, end)) = autolink_at(chars, i) {
            let underline = base.add_modifier(Modifier::UNDERLINED);
            out.extend(url.iter().map(|c| (*c, underline)));
            i = end;
        } else {
            out.push((c, base));
            i += 1;
        }
    }
    out
}

/// First `cc` pair at or after `from`, skipping escapes.
fn find_pair(chars: &[char], from: usize, c: char) -> Option<usize> {
    let mut i = from;
    while i + 1 < chars.len() {
        if chars[i] == '\\' {
            i += 2;
            continue;
        }
        if chars[i] == c && chars[i + 1] == c {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// The close of a `*x*` / `_x_` opened at `i`, if it opens there. It opens only before
/// a non-space and closes only after one; `_` also needs non-word characters outside,
/// so `snake_case` and `test_foo_bar` stay literal.
fn em_close(chars: &[char], open: usize) -> Option<usize> {
    let c = chars[open];
    if !matches!(c, '*' | '_') {
        return None;
    }
    let next = chars.get(open + 1)?;
    if next.is_whitespace() || *next == c {
        return None;
    }
    if c == '_' && open > 0 && chars[open - 1].is_alphanumeric() {
        return None;
    }
    let mut i = open + 1;
    while i < chars.len() {
        if chars[i] == '\\' {
            i += 2;
            continue;
        }
        if chars[i] == c && !chars[i - 1].is_whitespace() {
            let word_right = chars.get(i + 1).is_some_and(|r| r.is_alphanumeric());
            if c == '*' || !word_right {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// `[label](url)` or `![label](url)` at `i`: label, url, and the index past it. The url
/// has no spaces; the label has no brackets.
fn link_at(chars: &[char], i: usize) -> Option<(&[char], &[char], usize)> {
    let open = match chars[i] {
        '[' => i,
        '!' if chars.get(i + 1) == Some(&'[') => i + 1,
        _ => return None,
    };
    let close = open
        + 1
        + chars[open + 1..]
            .iter()
            .position(|c| matches!(c, ']' | '['))?;
    if chars[close] != ']' || chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let url_from = close + 2;
    let end = url_from
        + chars[url_from..]
            .iter()
            .position(|c| *c == ')' || c.is_whitespace())?;
    if chars[end] != ')' || end == url_from {
        return None;
    }
    Some((&chars[open + 1..close], &chars[url_from..end], end + 1))
}

/// `<https://…>`, `<http://…>`, `<mailto:…>` at `i`: the url and the index past it.
fn autolink_at(chars: &[char], i: usize) -> Option<(&[char], usize)> {
    if chars[i] != '<' {
        return None;
    }
    let end = i
        + 1
        + chars[i + 1..]
            .iter()
            .position(|c| matches!(c, '>' | '<') || c.is_whitespace())?;
    if chars[end] != '>' {
        return None;
    }
    let url = &chars[i + 1..end];
    let text: String = url.iter().collect();
    ["https://", "http://", "mailto:"]
        .iter()
        .any(|scheme| text.len() > scheme.len() && text.starts_with(scheme))
        .then_some((url, end + 1))
}

/// Paint one already-wrapped notes row, tracking fenced-code state (edit sessions).
///
/// Fence lines and body are dim. Body does not run inline md.
/// Other lines use [`paint_md_line`].
pub fn paint_notes_line(
    text: &str,
    width: usize,
    base: Style,
    in_fence: &mut bool,
) -> Line<'static> {
    if width == 0 {
        return Line::from("");
    }
    if is_fence_line(text) {
        *in_fence = !*in_fence;
        return bound_styled_line(
            vec![Span::styled(text.trim_end().to_string(), style_dim())],
            width,
        );
    }
    if *in_fence {
        return bound_styled_line(
            vec![Span::styled(text.trim_end().to_string(), style_dim())],
            width,
        );
    }
    paint_md_line(text, width, base)
}

/// Same styles as view, with dim on every span (peek).
pub fn dim_line(line: Line<'static>) -> Line<'static> {
    Line::from(
        line.spans
            .into_iter()
            .map(|span| Span::styled(span.content, span.style.add_modifier(Modifier::DIM)))
            .collect::<Vec<_>>(),
    )
}

/// Paint one already-wrapped notes row with mono markdown styling, line-start markers
/// kept in place (dim) so the row stays the raw wrap row.
pub fn paint_md_line(text: &str, width: usize, base: Style) -> Line<'static> {
    if width == 0 {
        return Line::from("");
    }
    let trimmed = text.trim_end();
    if trimmed.is_empty() {
        return Line::from(Span::styled(" ".repeat(width.min(1)), base));
    }

    let (prefix_spans, body, heading) = line_prefix(trimmed);
    let mut spans = prefix_spans;
    let body_base = if heading { style_heading() } else { base };
    spans.extend(spans_of(&inline(body, body_base)).spans);
    bound_styled_line(spans, width)
}

/// Dim list / heading markers; return the remainder and whether it is a heading.
fn line_prefix(text: &str) -> (Vec<Span<'static>>, &str, bool) {
    let lead = text.len() - text.trim_start().len();
    let (indent, rest) = text.split_at(lead);
    let mut spans = Vec::new();
    if !indent.is_empty() {
        spans.push(Span::styled(indent.to_string(), style_plain()));
    }
    let bytes = rest.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() && bytes[i] == b'#' && i < 6 {
        i += 1;
    }
    if i > 0 && i < bytes.len() && bytes[i] == b' ' {
        spans.push(Span::styled(rest[..i + 1].to_string(), style_dim()));
        return (spans, &rest[i + 1..], true);
    }
    if bytes.len() >= 2 && matches!(bytes[0], b'-' | b'*') && bytes[1] == b' ' {
        spans.push(Span::styled(rest[..2].to_string(), style_dim()));
        return (spans, &rest[2..], false);
    }
    (spans, rest, false)
}

fn bound_styled_line(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let flat: String = spans.iter().map(|s| s.content.as_ref()).collect();
    let shown = present_line(&flat, width);
    if shown == flat {
        return Line::from(spans);
    }
    Line::from(Span::styled(shown, style_plain()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    fn has_mod(line: &Line<'_>, m: Modifier) -> bool {
        line.spans.iter().any(|s| s.style.add_modifier.contains(m))
    }

    fn texts(lines: &[Line<'_>]) -> Vec<String> {
        lines.iter().map(line_text).collect()
    }

    /// The style of the first span whose text equals `content`.
    fn span_style(lines: &[Line<'_>], content: &str) -> Style {
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .find(|span| span.content.as_ref() == content)
            .unwrap_or_else(|| panic!("no span {content:?} in {:?}", texts(lines)))
            .style
    }

    fn dim(style: Style) -> bool {
        style.add_modifier.contains(Modifier::DIM)
    }

    #[test]
    fn strong_and_em_and_code() {
        let line = paint_md_line("a **b** *c* `d`", 40, style_plain());
        assert!(has_mod(&line, Modifier::BOLD));
        assert!(has_mod(&line, Modifier::UNDERLINED));
        let code = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "`d`")
            .expect("code span");
        assert!(code.style.add_modifier.contains(Modifier::DIM));
        assert!(!code.style.add_modifier.contains(Modifier::REVERSED));
        let flat: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(flat, "a b c `d`");
        let strong = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "b")
            .expect("strong span");
        assert!(strong.style.add_modifier.contains(Modifier::BOLD));
        assert!(!strong.style.add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn heading_and_list_markers_dim_in_the_edit_session_painter() {
        let h = paint_md_line("# Title", 40, style_plain());
        assert_eq!(h.spans[0].content.as_ref(), "# ");
        assert!(h.spans[0].style.add_modifier.contains(Modifier::DIM));
        let title = h
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "Title")
            .expect("heading body");
        assert!(title.style.add_modifier.contains(Modifier::BOLD));
        assert!(title.style.add_modifier.contains(Modifier::UNDERLINED));
        let indented = paint_md_line("  # Title", 40, style_plain());
        assert!(indented.spans.iter().any(|s| s.content.as_ref() == "# "));
        let list = paint_md_line("- item", 40, style_plain());
        assert_eq!(list.spans[0].content.as_ref(), "- ");
        assert!(list.spans[0].style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn the_edit_session_painter_keeps_task_list_markers() {
        let line = paint_md_line("- [ ] not a step", 40, style_plain());
        let flat: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(flat.contains("[ ] not a step"));
    }

    #[test]
    fn fenced_block_dims_the_fence_and_leaves_body_unparsed() {
        let mut in_fence = false;
        let open = paint_notes_line("```rust", 40, style_plain(), &mut in_fence);
        assert!(in_fence);
        assert!(has_mod(&open, Modifier::DIM));
        let body = paint_notes_line("**not bold**", 40, style_plain(), &mut in_fence);
        let flat: String = body.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(flat, "**not bold**");
        assert!(!has_mod(&body, Modifier::BOLD));
        assert!(has_mod(&body, Modifier::DIM));
        let close = paint_notes_line("```", 40, style_plain(), &mut in_fence);
        assert!(!in_fence);
        assert!(has_mod(&close, Modifier::DIM));
        let after = paint_notes_line("**bold**", 40, style_plain(), &mut in_fence);
        assert!(has_mod(&after, Modifier::BOLD));
    }

    #[test]
    fn dim_line_keeps_heading_and_code_and_adds_dim() {
        let mut in_fence = false;
        let heading = dim_line(paint_notes_line(
            "# Title",
            40,
            style_plain(),
            &mut in_fence,
        ));
        let title = heading
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "Title")
            .expect("heading");
        assert!(title.style.add_modifier.contains(Modifier::BOLD));
        assert!(title.style.add_modifier.contains(Modifier::UNDERLINED));
        assert!(title.style.add_modifier.contains(Modifier::DIM));
        let code = dim_line(paint_md_line("`x`", 40, style_plain()));
        let span = code
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "`x`")
            .expect("code");
        assert!(!span.style.add_modifier.contains(Modifier::REVERSED));
        assert!(span.style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn unmatched_stars_and_word_underscores_stay_literal() {
        for source in ["*.rs", "test_foo", "snake_case_name", "a * b * c", "2 ** 3"] {
            let lines = render(source, 40, style_plain());
            assert_eq!(texts(&lines), vec![source.to_string()], "{source}");
        }
        let glob = paint_md_line("*.rs", 40, style_plain());
        let flat: String = glob.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(flat, "*.rs");
    }

    #[test]
    fn em_keeps_matching_delimiter_across_inner_underscores() {
        for sample in ["*file_name*", "_file_name_", "*foo*", "_foo_"] {
            let line = paint_md_line(sample, 40, style_plain());
            let flat: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            let expected = sample.trim_matches(|c| c == '*' || c == '_');
            assert_eq!(flat, expected, "{sample}");
            let body = line
                .spans
                .iter()
                .find(|s| s.content.as_ref() == expected)
                .unwrap_or_else(|| panic!("em body for {sample}"));
            assert!(
                body.style.add_modifier.contains(Modifier::UNDERLINED),
                "{sample}"
            );
        }
    }

    #[test]
    fn underscore_without_a_valid_closer_stays_literal() {
        let line = paint_md_line("_file_name and **bold** `code`", 60, style_plain());
        let flat: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(flat, "_file_name and bold `code`");
        assert!(!has_mod(&line, Modifier::UNDERLINED));
        let bold = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "bold")
            .expect("strong span");
        assert!(bold.style.add_modifier.contains(Modifier::BOLD));
        let code = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "`code`")
            .expect("code span");
        assert!(code.style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn heading_keeps_bold_when_the_body_is_emphasized() {
        let h = paint_md_line("# *Release*", 40, style_plain());
        let body = h
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "Release")
            .expect("heading em");
        assert!(body.style.add_modifier.contains(Modifier::BOLD));
        assert!(body.style.add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn render_hides_heading_markers_and_keeps_the_heading_style() {
        let lines = render("## Plan\n  # Indented", 40, style_plain());
        assert_eq!(texts(&lines), vec!["Plan", "  Indented"]);
        let plan = span_style(&lines, "Plan");
        assert!(plan.add_modifier.contains(Modifier::BOLD));
        assert!(plan.add_modifier.contains(Modifier::UNDERLINED));
        // `#tag` without a space is not a heading.
        assert_eq!(texts(&render("#tag", 40, style_plain())), vec!["#tag"]);
    }

    #[test]
    fn render_keeps_ordered_list_numbers_dim() {
        let lines = render("1. first\n12) twelfth", 40, style_plain());
        assert_eq!(texts(&lines), vec!["1. first", "12) twelfth"]);
        assert!(dim(span_style(&lines, "1. ")));
        assert!(dim(span_style(&lines, "12) ")));
        assert!(!dim(span_style(&lines, "first")));
        // A version number or a year is not a list.
        assert_eq!(
            texts(&render("2026. was a year", 40, style_plain())),
            vec!["2026. was a year"]
        );
    }

    #[test]
    fn render_draws_a_dim_bar_for_each_quote_level() {
        let lines = render("> quoted **text**\n> > nested", 40, style_plain());
        assert_eq!(texts(&lines), vec!["│ quoted text", "│ │ nested"]);
        assert!(dim(span_style(&lines, "│ ")));
        assert!(span_style(&lines, "text")
            .add_modifier
            .contains(Modifier::BOLD));
        // A quoted list item keeps both the bar and the marker.
        assert_eq!(
            texts(&render("> - item", 40, style_plain())),
            vec!["│ - item"]
        );
    }

    #[test]
    fn a_wrapped_quote_keeps_its_bar_on_every_row() {
        let lines = render("> alpha beta gamma delta", 12, style_plain());
        assert!(lines.len() > 1, "{:?}", texts(&lines));
        for text in texts(&lines) {
            assert!(text.starts_with("│ "), "{text:?}");
        }
    }

    #[test]
    fn render_turns_rules_into_a_dim_rule_across_the_width() {
        for source in ["---", "***", "___", "- - -"] {
            let lines = render(source, 10, style_plain());
            assert_eq!(texts(&lines), vec!["─".repeat(10)], "{source}");
            assert!(dim(lines[0].spans[0].style));
        }
        // Two marks are not a rule.
        assert_eq!(texts(&render("--", 10, style_plain())), vec!["--"]);
    }

    #[test]
    fn render_shows_link_text_underlined_and_the_url_dim() {
        let lines = render("see [the docs](https://x.dev/a) now", 60, style_plain());
        assert_eq!(texts(&lines), vec!["see the docs (https://x.dev/a) now"]);
        assert!(span_style(&lines, "the docs")
            .add_modifier
            .contains(Modifier::UNDERLINED));
        assert!(dim(span_style(&lines, " (https://x.dev/a)")));
        // Text equal to its url shows once.
        let same = render("[https://x.dev](https://x.dev)", 60, style_plain());
        assert_eq!(texts(&same), vec!["https://x.dev"]);
        assert!(span_style(&same, "https://x.dev")
            .add_modifier
            .contains(Modifier::UNDERLINED));
        // Autolinks show the url alone.
        let auto = render("at <https://x.dev/b> ok", 60, style_plain());
        assert_eq!(texts(&auto), vec!["at https://x.dev/b ok"]);
        // Brackets that are not a link stay literal.
        for source in ["[ ] todo", "a[0] (b)", "<div>", "[x](has space)"] {
            assert_eq!(
                texts(&render(source, 60, style_plain())),
                vec![source.to_string()]
            );
        }
    }

    #[test]
    fn render_dims_strikethrough_and_hides_its_markers() {
        let lines = render("was ~~old~~ new", 40, style_plain());
        assert_eq!(texts(&lines), vec!["was old new"]);
        assert!(dim(span_style(&lines, "old")));
        assert_eq!(texts(&render("~ ~~", 40, style_plain())), vec!["~ ~~"]);
    }

    #[test]
    fn render_draws_task_list_boxes() {
        let lines = render("- [ ] open\n- [x] done\n  * [X] nested", 40, style_plain());
        assert_eq!(texts(&lines), vec!["☐ open", "☑ done", "  ☑ nested"]);
        assert!(!has_mod(&lines[0], Modifier::DIM));
        assert!(dim(span_style(&lines, "done")));
    }

    #[test]
    fn render_shows_escaped_characters_literally() {
        let lines = render(
            r"\*not em\* and \_x\_ and \`y\` and \# no",
            60,
            style_plain(),
        );
        assert_eq!(texts(&lines), vec!["*not em* and _x_ and `y` and # no"]);
        assert!(!has_mod(&lines[0], Modifier::UNDERLINED));
        assert!(!has_mod(&lines[0], Modifier::DIM));
        // A backslash before a letter is kept (Windows paths).
        assert_eq!(
            texts(&render(r"C:\dir\file", 60, style_plain())),
            vec![r"C:\dir\file"]
        );
    }

    #[test]
    fn nested_lists_keep_their_indent_and_hang_wrapped_rows() {
        let lines = render("- top\n  - child words wrap here", 16, style_plain());
        let shown = texts(&lines);
        assert_eq!(shown[0], "- top");
        assert_eq!(shown[1], "  - child words ");
        assert_eq!(shown[2], "    wrap here");
        let ordered = texts(&render("10. alpha beta gamma", 12, style_plain()));
        assert_eq!(ordered, vec!["10. alpha ", "    beta ", "    gamma"]);
    }

    #[test]
    fn render_draws_pipe_tables_with_dim_separators_and_no_divider() {
        let source = "| name | state |\n|------|:-----:|\n| a | **ok** |";
        let lines = render(source, 40, style_plain());
        assert_eq!(texts(&lines), vec!["name │ state", "a │ ok"]);
        assert!(dim(span_style(&lines, " │ ")));
        assert!(span_style(&lines, "name")
            .add_modifier
            .contains(Modifier::BOLD));
        assert!(!span_style(&lines, "a")
            .add_modifier
            .contains(Modifier::BOLD));
        // An escaped pipe stays inside its cell.
        assert_eq!(
            texts(&render(r"| a \| b | c |", 40, style_plain())),
            vec!["a | b │ c"]
        );
    }

    #[test]
    fn render_keeps_fenced_code_unparsed_and_dim() {
        let lines = render("```\n**raw** [a](b)\n```\n**bold**", 40, style_plain());
        assert_eq!(texts(&lines), vec!["```", "**raw** [a](b)", "```", "bold"]);
        assert!(dim(span_style(&lines, "**raw** [a](b)")));
        assert!(span_style(&lines, "bold")
            .add_modifier
            .contains(Modifier::BOLD));
    }

    #[test]
    fn render_wraps_on_rendered_width_so_markers_never_split_a_span() {
        // Raw, `**bold words**` is 14 cells and would wrap inside its markers at 12.
        let lines = render("**bold words** tail", 12, style_plain());
        assert_eq!(texts(&lines), vec!["bold words ", "tail"]);
        assert!(span_style(&lines, "bold words")
            .add_modifier
            .contains(Modifier::BOLD));
        // A bold span that does wrap stays bold on both rows.
        let split = render("**one two three**", 8, style_plain());
        assert_eq!(texts(&split), vec!["one two ", "three"]);
        for line in &split {
            for span in &line.spans {
                assert!(span.style.add_modifier.contains(Modifier::BOLD));
            }
        }
    }

    #[test]
    fn every_rendered_row_fits_its_width() {
        let source = "# Heading that is long\n> quote that wraps a lot of words\n\
                      - [ ] a task with many words to wrap\n1. ordered item with words\n\
                      | a | b | c |\n|---|---|---|\n| long cell text | x | y |\n---\n\
                      see [link text](https://example.com/a/very/long/path) and 日本語の文章です";
        for width in 2..30 {
            for line in render(source, width, style_plain()) {
                assert!(line.width() <= width, "{width}: {:?}", line_text(&line));
            }
        }
    }

    #[test]
    fn render_keeps_blank_lines_and_returns_a_row_for_empty_text() {
        assert_eq!(texts(&render("", 10, style_plain())), vec![""]);
        assert_eq!(
            texts(&render("a\n\nb", 10, style_plain())),
            vec!["a", "", "b"]
        );
        assert!(render("a", 0, style_plain()).is_empty());
    }

    #[test]
    fn render_styles_only_with_mono_modifiers() {
        let source = "# h\n> q ~~s~~ [l](u) *e* **b** `c`\n- [x] d\n| t | u |\n|-|-|\n---";
        for line in render(source, 40, style_plain()) {
            for span in &line.spans {
                assert!(span.style.fg.is_none() && span.style.bg.is_none());
                assert!(span
                    .style
                    .add_modifier
                    .difference(super::super::render::MONO_MODIFIERS)
                    .is_empty());
            }
        }
    }
}
