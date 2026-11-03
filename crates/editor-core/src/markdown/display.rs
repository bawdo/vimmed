//! The display model of one line: which source bytes show, in what style,
//! behind what prefix glyph and block decoration, given where the cursor is.
//! Nothing here paints; styles name roles that the renderer maps to tokens.

use std::ops::Range;

use super::block::{Blocks, LineKind, ListKind};
use super::inline::{InlineEl, InlineKind};
use super::table::TableId;

/// What the cursor reveals on a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reveal {
    /// The cursor is elsewhere: everything renders.
    None,
    /// The cursor is on this line at this byte offset.
    Cursor(usize),
    /// The whole line shows raw (the cursor is in its code block or table).
    Block,
}

/// The glyph drawn before the line's text in place of its hidden markdown
/// prefix. `None` when the prefix shows raw or the line has none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prefix {
    None,
    Bullet(char),
    /// The ordered marker, `N.`. `line_display` sees one line, so this is the
    /// number as written; callers holding `Blocks` substitute
    /// `ordered_display`.
    Number(String),
    /// A task checkbox; true when checked.
    Task(bool),
    /// The gutter for this many quote bars.
    QuoteBars(u8),
}

/// The role a run of source text renders in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpanStyle {
    Body,
    Strong,
    Em,
    StrongEm,
    Strike,
    Code,
    Link,
    /// Revealed markdown syntax (monospace, `color.syntax.reveal`).
    Syntax,
    Heading(u8),
    CodeBlock {
        lang: Option<String>,
    },
}

/// A run of the line's source shown in one style. Display text is the
/// concatenation of every span's source text, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplaySpan {
    pub src: Range<usize>,
    pub style: SpanStyle,
}

/// Which row of a table a line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TablePart {
    Header,
    Separator,
    Row,
}

/// Decoration painted behind or around the whole line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockDeco {
    None,
    CodeBg {
        first: bool,
        last: bool,
        unclosed: bool,
    },
    QuoteBars(u8),
    Rule,
    Image {
        src: String,
        alt: String,
    },
    TableRow {
        table: TableId,
        part: TablePart,
    },
}

/// How one line displays. `src_len` is the source line's length, so the
/// mappings can place a cursor after trailing hidden syntax.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineDisplay {
    pub prefix: Prefix,
    pub spans: Vec<DisplaySpan>,
    pub deco: BlockDeco,
    pub src_len: usize,
}

/// The glyph drawn for an unordered item at every nesting level.
const BULLET: char = '◦';

/// The display of `line` (of kind `kind`, with inline elements `inline` from
/// `parse_inline`) for the given reveal.
///
/// - Inline: with `Reveal::Cursor(c)`, the element whose syntax range holds
///   `c` (a cursor at its end does not count) shows raw, climbing to its
///   outermost parent; when several do, the one starting first wins. Every
///   element inside the revealed one shows raw with it. Revealed markers take
///   `SpanStyle::Syntax`; content keeps its style; other elements stay
///   rendered.
/// - A heading, list, quote or rule prefix shows raw (as `Syntax`) whenever
///   the cursor is on the line; otherwise it is hidden behind a `Prefix`.
///   Elements that start inside the prefix are ignored, so a `*` bullet never
///   opens an italic.
/// - Code, table and image lines reveal as a whole: any cursor on them (or
///   `Reveal::Block`) shows the line raw.
///
/// Code lines take `CodeBg` with `first` set on the opening fence, `last` on
/// the closing fence and `unclosed` false: callers holding `Blocks` take the
/// decoration from `code_bg` instead. Code content lines carry
/// `CodeBlock { lang: None }` for the same reason (only the fence names it).
pub fn line_display(
    line: &str,
    kind: &LineKind,
    inline: &[InlineEl],
    reveal: Reveal,
) -> LineDisplay {
    let len = line.len();
    let on_line = reveal != Reveal::None;
    let mut d = LineDisplay {
        prefix: Prefix::None,
        spans: Vec::new(),
        deco: BlockDeco::None,
        src_len: len,
    };
    let whole = |style| DisplaySpan { src: 0..len, style };
    match kind {
        LineKind::FenceOpen { .. } | LineKind::FenceClose { .. } | LineKind::Code { .. } => {
            let is_code = matches!(kind, LineKind::Code { .. });
            d.deco = BlockDeco::CodeBg {
                first: matches!(kind, LineKind::FenceOpen { .. }),
                last: matches!(kind, LineKind::FenceClose { .. }),
                unclosed: false,
            };
            if is_code {
                d.spans.push(whole(SpanStyle::CodeBlock { lang: None }));
            } else if on_line {
                d.spans.push(whole(SpanStyle::Syntax));
            }
            d.spans.retain(|s| !s.src.is_empty());
            return d;
        }
        LineKind::Rule => {
            if on_line {
                d.spans.push(whole(SpanStyle::Syntax));
            } else {
                d.deco = BlockDeco::Rule;
            }
            return d;
        }
        LineKind::Image { alt, src } if !on_line => {
            d.deco = BlockDeco::Image {
                src: line.get(src.clone()).unwrap_or_default().to_owned(),
                alt: line.get(alt.clone()).unwrap_or_default().to_owned(),
            };
            return d;
        }
        LineKind::TableHeader { table }
        | LineKind::TableSep { table }
        | LineKind::TableRow { table }
            if !on_line =>
        {
            let part = match kind {
                LineKind::TableHeader { .. } => TablePart::Header,
                LineKind::TableSep { .. } => TablePart::Separator,
                _ => TablePart::Row,
            };
            d.deco = BlockDeco::TableRow {
                table: *table,
                part,
            };
            if part == TablePart::Separator {
                return d;
            }
        }
        _ => {}
    }

    let reveal_all = reveal == Reveal::Block
        || (on_line
            && matches!(
                kind,
                LineKind::Image { .. }
                    | LineKind::TableHeader { .. }
                    | LineKind::TableSep { .. }
                    | LineKind::TableRow { .. }
            ));

    let (prefix_end, base) = match kind {
        LineKind::Heading(level) => (heading_content(line), SpanStyle::Heading(*level)),
        LineKind::ListItem { content_offset, .. } | LineKind::Quote { content_offset, .. } => {
            (*content_offset, SpanStyle::Body)
        }
        _ => (0, SpanStyle::Body),
    };
    let prefix_end = prefix_end.min(len);
    if prefix_end > 0 && !on_line {
        match kind {
            LineKind::ListItem { kind, .. } => {
                d.prefix = match kind {
                    ListKind::Bullet(_) => Prefix::Bullet(BULLET),
                    ListKind::Ordered(n) => Prefix::Number(format!("{n}.")),
                    ListKind::Task(checked) => Prefix::Task(*checked),
                };
            }
            LineKind::Quote { depth, .. } => {
                d.prefix = Prefix::QuoteBars(*depth);
                d.deco = BlockDeco::QuoteBars(*depth);
            }
            _ => {}
        }
    }

    // Elements starting in the prefix are ignored, as are malformed or stale
    // ones (outside the line, or off a char boundary), so spans always slice.
    let valid = |e: &InlineEl| {
        e.syntax.start >= prefix_end
            && e.syntax.start <= e.content.start
            && e.content.start <= e.content.end
            && e.content.end <= e.syntax.end
            && e.syntax.end <= len
            && [e.syntax.start, e.content.start, e.content.end, e.syntax.end]
                .iter()
                .all(|&i| line.is_char_boundary(i))
    };
    let root = match reveal {
        Reveal::Cursor(c) if !reveal_all => revealed_root(inline, c, &valid),
        _ => None,
    };
    let revealed = |e: &InlineEl| {
        reveal_all
            || root
                .as_ref()
                .is_some_and(|r| r.start <= e.syntax.start && e.syntax.end <= r.end)
    };

    let mut events: Vec<(usize, Flag, i32)> = Vec::new();
    let mut mark = |r: Range<usize>, flag: Flag| {
        if !r.is_empty() {
            events.push((r.start, flag, 1));
            events.push((r.end, flag, -1));
        }
    };
    mark(
        0..prefix_end,
        if on_line { Flag::Syntax } else { Flag::Hidden },
    );
    for e in inline.iter().filter(|e| valid(e)) {
        let marker = if revealed(e) {
            Flag::Syntax
        } else {
            Flag::Hidden
        };
        mark(e.syntax.start..e.content.start, marker);
        mark(e.content.end..e.syntax.end, marker);
        let content = match e.kind {
            InlineKind::Bold => Some(Flag::Strong),
            InlineKind::Italic => Some(Flag::Em),
            InlineKind::Strike => Some(Flag::Strike),
            InlineKind::Code => Some(Flag::Code),
            InlineKind::Link { .. } => Some(Flag::Link),
            InlineKind::Image { .. } | InlineKind::Escape => None,
        };
        if let Some(flag) = content {
            mark(e.content.clone(), flag);
        }
    }
    events.sort_by_key(|&(pos, _, _)| pos);

    let mut counts = [0i32; Flag::COUNT];
    let mut next = 0;
    let mut pos = 0;
    while pos < len {
        while next < events.len() && events[next].0 <= pos {
            counts[events[next].1 as usize] += events[next].2;
            next += 1;
        }
        let end = events.get(next).map_or(len, |ev| ev.0.min(len));
        if let Some(style) = style_of(&counts, &base) {
            match d.spans.last_mut() {
                Some(last) if last.style == style && last.src.end == pos => last.src.end = end,
                _ => d.spans.push(DisplaySpan {
                    src: pos..end,
                    style,
                }),
            }
        }
        pos = end;
    }
    d
}

/// What covers a byte, counted while sweeping the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flag {
    Hidden,
    Syntax,
    Strong,
    Em,
    Strike,
    Code,
    Link,
}

impl Flag {
    const COUNT: usize = 7;
}

/// The style of a byte under `counts`, or `None` when it is hidden. Revealed
/// syntax wins over hidden syntax; a byte takes the strongest inline role
/// covering it (code, link, then emphasis, then strikethrough).
fn style_of(counts: &[i32; Flag::COUNT], base: &SpanStyle) -> Option<SpanStyle> {
    let on = |f: Flag| counts[f as usize] > 0;
    Some(if on(Flag::Syntax) {
        SpanStyle::Syntax
    } else if on(Flag::Hidden) {
        return None;
    } else if on(Flag::Code) {
        SpanStyle::Code
    } else if on(Flag::Link) {
        SpanStyle::Link
    } else if on(Flag::Strong) && on(Flag::Em) {
        SpanStyle::StrongEm
    } else if on(Flag::Strong) {
        SpanStyle::Strong
    } else if on(Flag::Em) {
        SpanStyle::Em
    } else if on(Flag::Strike) {
        SpanStyle::Strike
    } else {
        base.clone()
    })
}

/// The syntax range to reveal for a cursor at `c`: among the elements whose
/// syntax holds `c`, climb each to its outermost parent and keep the one that
/// starts first.
fn revealed_root(
    inline: &[InlineEl],
    c: usize,
    valid: &dyn Fn(&InlineEl) -> bool,
) -> Option<Range<usize>> {
    let mut best: Option<Range<usize>> = None;
    for e in inline.iter().filter(|e| valid(e) && e.syntax.contains(&c)) {
        let mut top = e;
        // Bounded climb, so a malformed parent cycle cannot hang.
        for _ in 0..inline.len() {
            match top.parent.and_then(|p| inline.get(p)) {
                Some(p) if valid(p) => top = p,
                _ => break,
            }
        }
        if best.as_ref().is_none_or(|b| top.syntax.start < b.start) {
            best = Some(top.syntax.clone());
        }
    }
    best
}

/// Where a heading's text starts: after the hashes and the whitespace after
/// them.
fn heading_content(line: &str) -> usize {
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    let rest = &line[hashes..];
    hashes + rest.len() - rest.trim_start().len()
}

/// The code block decoration of `line`, derived from the line kinds around
/// it: `first` on the block's opening fence, `last` when the next line is not
/// part of the same block, and `unclosed` when the block's final line is not
/// a closing fence (a block can end at the next opening fence, not only at
/// the end of the document). `BlockDeco::None` for lines outside code.
pub fn code_bg(blocks: &Blocks, line: usize) -> BlockDeco {
    let count = blocks.line_count();
    if line >= count {
        return BlockDeco::None;
    }
    let kind = blocks.kind(line);
    let id = match kind {
        LineKind::FenceOpen { block, .. }
        | LineKind::FenceClose { block }
        | LineKind::Code { block } => *block,
        _ => return BlockDeco::None,
    };
    let continues = |i: usize| {
        i < count
            && matches!(
                blocks.kind(i),
                LineKind::Code { block } | LineKind::FenceClose { block } if *block == id
            )
    };
    let mut end = line;
    while continues(end + 1) {
        end += 1;
    }
    BlockDeco::CodeBg {
        first: matches!(kind, LineKind::FenceOpen { .. }),
        last: end == line,
        unclosed: !matches!(blocks.kind(end), LineKind::FenceClose { .. }),
    }
}

/// The source byte offset under display byte `display_byte`. A display
/// offset on the boundary between two spans maps to the start of the later
/// one (past any hidden syntax), and the end of the display maps to the end
/// of the line. Offsets past the end clamp to it.
///
/// Char boundaries in the display text map to char boundaries in the source
/// (spans split only at ASCII markers). Never panics, whatever the offset.
pub fn display_to_src(d: &LineDisplay, display_byte: usize) -> usize {
    let mut at = 0;
    for s in &d.spans {
        let n = s.src.len();
        if display_byte < at + n {
            return s.src.start + (display_byte - at);
        }
        at += n;
    }
    d.src_len
}

/// The display byte offset of source offset `src_col`, or `None` when it
/// lies strictly inside hidden syntax (or past the end of the line). The
/// edges of a hidden run map to the display boundary where it was removed.
pub fn src_to_display(d: &LineDisplay, src_col: usize) -> Option<usize> {
    if src_col > d.src_len {
        return None;
    }
    let mut at = 0;
    let mut gap_start = 0;
    for s in &d.spans {
        if src_col < s.src.start {
            return (src_col == gap_start).then_some(at);
        }
        if src_col <= s.src.end {
            return Some(at + (src_col - s.src.start));
        }
        at += s.src.len();
        gap_start = s.src.end;
    }
    (src_col == gap_start || src_col == d.src_len).then_some(at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::markdown::block::BlockId;
    use crate::markdown::inline::parse_inline;

    /// The display text: every span's source text, in order.
    fn text_of(d: &LineDisplay, l: &str) -> String {
        d.spans.iter().map(|s| &l[s.src.clone()]).collect()
    }

    fn show(l: &str, kind: &LineKind, reveal: Reveal) -> LineDisplay {
        line_display(l, kind, &parse_inline(l), reveal)
    }

    fn para(l: &str, reveal: Reveal) -> LineDisplay {
        show(l, &LineKind::Paragraph, reveal)
    }

    /// (text, style) per span.
    fn styled(d: &LineDisplay, l: &str) -> Vec<(String, SpanStyle)> {
        d.spans
            .iter()
            .map(|s| (l[s.src.clone()].to_owned(), s.style.clone()))
            .collect()
    }

    fn s(text: &str, style: SpanStyle) -> (String, SpanStyle) {
        (text.to_owned(), style)
    }

    /// The kind Task 9 gives `line` on its own.
    fn kind_of(line: &str) -> LineKind {
        let b = Buffer::from_text(line);
        Blocks::new(&b).kind(0).clone()
    }

    fn show_line(l: &str, reveal: Reveal) -> LineDisplay {
        show(l, &kind_of(l), reveal)
    }

    // The brief's reveal cases.

    #[test]
    fn cursor_inside_bold_reveals_only_that_bold() {
        let l = "a **b** *c*";
        let el = parse_inline(l);
        let d = line_display(l, &LineKind::Paragraph, &el, Reveal::Cursor(4));
        assert_eq!(text_of(&d, l), "a **b** c");
    }

    #[test]
    fn cursor_at_syntax_end_does_not_reveal() {
        let l = "a **b** *c*";
        let el = parse_inline(l);
        let d = line_display(l, &LineKind::Paragraph, &el, Reveal::Cursor(7));
        assert_eq!(text_of(&d, l), "a b c");
    }

    #[test]
    fn heading_prefix_shows_only_on_cursor_line() {
        let l = "# Title";
        let el = parse_inline(l);
        let d = line_display(l, &LineKind::Heading(1), &el, Reveal::None);
        assert_eq!(text_of(&d, l), "Title");
        let d = line_display(l, &LineKind::Heading(1), &el, Reveal::Cursor(0));
        assert_eq!(text_of(&d, l), "# Title");
    }

    #[test]
    fn src_display_round_trip_skips_hidden_syntax() {
        let l = "a **b**";
        let d = para(l, Reveal::None);
        assert_eq!(display_to_src(&d, 2), 4);
        assert_eq!(src_to_display(&d, 4), Some(2));
    }

    // Inline styles and reveal.

    #[test]
    fn rendered_elements_style_their_content() {
        let l = "a **b** _c_ ~~d~~ `e` [f](u)";
        assert_eq!(
            styled(&para(l, Reveal::None), l),
            [
                s("a ", SpanStyle::Body),
                s("b", SpanStyle::Strong),
                s(" ", SpanStyle::Body),
                s("c", SpanStyle::Em),
                s(" ", SpanStyle::Body),
                s("d", SpanStyle::Strike),
                s(" ", SpanStyle::Body),
                s("e", SpanStyle::Code),
                s(" ", SpanStyle::Body),
                s("f", SpanStyle::Link),
            ]
        );
    }

    #[test]
    fn revealed_markers_are_syntax_and_content_keeps_its_style() {
        let l = "a **b** *c*";
        assert_eq!(
            styled(&para(l, Reveal::Cursor(4)), l),
            [
                s("a ", SpanStyle::Body),
                s("**", SpanStyle::Syntax),
                s("b", SpanStyle::Strong),
                s("**", SpanStyle::Syntax),
                s(" ", SpanStyle::Body),
                s("c", SpanStyle::Em),
            ]
        );
    }

    #[test]
    fn cursor_at_syntax_start_reveals() {
        let l = "a **b**";
        assert_eq!(text_of(&para(l, Reveal::Cursor(2)), l), "a **b**");
        assert_eq!(text_of(&para(l, Reveal::Cursor(1)), l), "a b");
    }

    #[test]
    fn italic_inside_bold_is_strong_em_and_reveals_the_bold() {
        let l = "**a _b_ c** d";
        assert_eq!(
            styled(&para(l, Reveal::None), l),
            [
                s("a ", SpanStyle::Strong),
                s("b", SpanStyle::StrongEm),
                s(" c", SpanStyle::Strong),
                s(" d", SpanStyle::Body),
            ]
        );
        // The cursor on the italic climbs to its parent bold.
        assert_eq!(text_of(&para(l, Reveal::Cursor(5)), l), "**a _b_ c** d");
    }

    #[test]
    fn smallest_start_wins_among_containing_elements() {
        // The cursor is inside both the link and the bold in its text.
        let l = "x [**a**](u) *y*";
        assert_eq!(text_of(&para(l, Reveal::Cursor(5)), l), "x [**a**](u) y");
    }

    #[test]
    fn other_elements_stay_rendered_when_one_reveals() {
        let l = "**a** `b` _c_";
        assert_eq!(text_of(&para(l, Reveal::Cursor(6)), l), "a `b` c");
    }

    #[test]
    fn escape_reveals_on_backslash_or_escaped_char() {
        let l = r"a \*b";
        assert_eq!(text_of(&para(l, Reveal::None), l), "a *b");
        assert_eq!(text_of(&para(l, Reveal::Cursor(2)), l), r"a \*b");
        assert_eq!(text_of(&para(l, Reveal::Cursor(3)), l), r"a \*b");
        assert_eq!(text_of(&para(l, Reveal::Cursor(4)), l), "a *b");
        assert_eq!(
            styled(&para(l, Reveal::Cursor(2)), l),
            [
                s("a ", SpanStyle::Body),
                s("\\", SpanStyle::Syntax),
                s("*b", SpanStyle::Body),
            ]
        );
    }

    #[test]
    fn link_hides_its_url_until_revealed() {
        let l = "see [here](http://x.y) now";
        assert_eq!(text_of(&para(l, Reveal::None), l), "see here now");
        assert_eq!(text_of(&para(l, Reveal::Cursor(12)), l), l);
    }

    #[test]
    fn inline_image_shows_its_alt_text() {
        let l = "see ![pic](a.png) now";
        assert_eq!(text_of(&para(l, Reveal::None), l), "see pic now");
    }

    #[test]
    fn block_reveal_shows_every_marker() {
        let l = "**a** _b_";
        assert_eq!(text_of(&para(l, Reveal::Block), l), l);
    }

    // Headings.

    #[test]
    fn heading_text_takes_the_heading_style() {
        let l = "## Sub **bold**";
        let d = show(l, &LineKind::Heading(2), Reveal::None);
        assert_eq!(d.prefix, Prefix::None);
        assert_eq!(
            styled(&d, l),
            [
                s("Sub ", SpanStyle::Heading(2)),
                s("bold", SpanStyle::Strong)
            ]
        );
        let d = show(l, &LineKind::Heading(2), Reveal::Cursor(14));
        assert_eq!(
            styled(&d, l)[..2],
            [
                s("## ", SpanStyle::Syntax),
                s("Sub ", SpanStyle::Heading(2))
            ]
        );
        // Cursor at 14 is inside the bold, so its markers show too.
        assert_eq!(text_of(&d, l), l);
    }

    // Lists (moon_edit editor_line_render_test, list rendering).

    #[test]
    fn renders_bullet_for_unordered_list_when_cursor_not_on_line() {
        let l = "- item";
        let d = show_line(l, Reveal::None);
        assert_eq!(d.prefix, Prefix::Bullet('◦'));
        assert_eq!(text_of(&d, l), "item");
    }

    #[test]
    fn reveals_raw_prefix_when_cursor_on_line() {
        let l = "- item";
        let d = show_line(l, Reveal::Cursor(3));
        assert_eq!(d.prefix, Prefix::None);
        assert_eq!(
            styled(&d, l),
            [s("- ", SpanStyle::Syntax), s("item", SpanStyle::Body)]
        );
    }

    #[test]
    fn renders_hollow_bullet_for_nested_list() {
        let l = "  - nested";
        let d = show_line(l, Reveal::None);
        assert_eq!(d.prefix, Prefix::Bullet('◦'));
        assert_eq!(text_of(&d, l), "nested");
        // Revealed, the indentation shows raw too.
        assert_eq!(text_of(&show_line(l, Reveal::Cursor(5)), l), l);
    }

    #[test]
    fn renders_checkbox_for_task_list() {
        let l = "- [ ] todo";
        let d = show_line(l, Reveal::None);
        assert_eq!(d.prefix, Prefix::Task(false));
        assert_eq!(text_of(&d, l), "todo");
        assert_eq!(
            show_line("- [x] done", Reveal::None).prefix,
            Prefix::Task(true)
        );
    }

    #[test]
    fn renders_ordered_list_with_number() {
        let l = "1. first";
        let d = show_line(l, Reveal::None);
        assert_eq!(d.prefix, Prefix::Number("1.".to_owned()));
        assert_eq!(text_of(&d, l), "first");
    }

    #[test]
    fn list_marker_is_not_parsed_as_inline() {
        // The `*` bullet would otherwise open an italic with the next `*`.
        let l = "* a *b*";
        let d = show_line(l, Reveal::None);
        assert_eq!(d.prefix, Prefix::Bullet('◦'));
        assert_eq!(text_of(&d, l), "a *b*");
    }

    // Quotes (moon_edit editor_line_render_test, blockquote rendering).

    #[test]
    fn renders_bars_and_content_for_blockquote_when_cursor_not_on_line() {
        let l = "> quoted text";
        let d = show_line(l, Reveal::None);
        assert_eq!(d.prefix, Prefix::QuoteBars(1));
        assert_eq!(d.deco, BlockDeco::QuoteBars(1));
        assert_eq!(text_of(&d, l), "quoted text");
    }

    #[test]
    fn renders_content_for_nested_blockquotes() {
        for (l, depth, text) in [
            (">> nested", 2, "nested"),
            ("> > spaced", 2, "spaced"),
            (">>> deep", 3, "deep"),
        ] {
            let d = show_line(l, Reveal::None);
            assert_eq!(d.prefix, Prefix::QuoteBars(depth), "{l}");
            assert_eq!(text_of(&d, l), text, "{l}");
        }
    }

    #[test]
    fn reveals_raw_quote_markers_when_cursor_on_line() {
        let l = "> quoted";
        let d = show_line(l, Reveal::Cursor(4));
        assert_eq!(d.prefix, Prefix::None);
        assert_eq!(d.deco, BlockDeco::None);
        assert_eq!(text_of(&d, l), "> quoted");
        assert_eq!(
            text_of(&show_line(">> nested", Reveal::Cursor(0)), ">> nested"),
            ">> nested"
        );
    }

    #[test]
    fn renders_bars_for_empty_blockquote() {
        let d = show_line(">", Reveal::None);
        assert_eq!(d.deco, BlockDeco::QuoteBars(1));
        assert!(d.spans.is_empty());
    }

    #[test]
    fn renders_bold_and_links_inside_blockquotes() {
        let l = "> some **bold** text";
        assert_eq!(text_of(&show_line(l, Reveal::None), l), "some bold text");
        let l = ">> a [link](x.md) here";
        assert_eq!(text_of(&show_line(l, Reveal::None), l), "a link here");
    }

    // Rules.

    #[test]
    fn renders_rule_when_cursor_not_on_line() {
        for l in ["---", "***", "___", "* * *"] {
            let d = show_line(l, Reveal::None);
            assert_eq!(d.deco, BlockDeco::Rule, "{l}");
            assert!(d.spans.is_empty(), "{l}");
        }
    }

    #[test]
    fn shows_raw_rule_when_cursor_on_line() {
        let l = "---";
        let d = show_line(l, Reveal::Cursor(1));
        assert_eq!(d.deco, BlockDeco::None);
        assert_eq!(styled(&d, l), [s("---", SpanStyle::Syntax)]);
    }

    // Code blocks.

    fn code_lines(text: &str) -> (Buffer, Blocks) {
        let b = Buffer::from_text(text);
        let bl = Blocks::new(&b);
        (b, bl)
    }

    #[test]
    fn code_content_renders_in_the_code_block_style() {
        let l = "console.log(\"**hi**\");";
        let d = line_display(l, &LineKind::Code { block: BlockId(0) }, &[], Reveal::None);
        assert_eq!(styled(&d, l), [s(l, SpanStyle::CodeBlock { lang: None })]);
        assert_eq!(
            d.deco,
            BlockDeco::CodeBg {
                first: false,
                last: false,
                unclosed: false
            }
        );
    }

    #[test]
    fn code_line_ignores_inline_elements_passed_in() {
        let l = "**x**";
        let d = line_display(
            l,
            &LineKind::Code { block: BlockId(0) },
            &parse_inline(l),
            Reveal::None,
        );
        assert_eq!(text_of(&d, l), l);
    }

    #[test]
    fn hides_fences_when_cursor_outside_code_block() {
        let open = LineKind::FenceOpen {
            block: BlockId(0),
            lang: Some("python".to_owned()),
        };
        let d = line_display("```python", &open, &[], Reveal::None);
        assert!(d.spans.is_empty());
        assert_eq!(
            d.deco,
            BlockDeco::CodeBg {
                first: true,
                last: false,
                unclosed: false
            }
        );
        let close = LineKind::FenceClose { block: BlockId(0) };
        let d = line_display("```", &close, &[], Reveal::None);
        assert!(d.spans.is_empty());
        assert_eq!(
            d.deco,
            BlockDeco::CodeBg {
                first: false,
                last: true,
                unclosed: false
            }
        );
    }

    #[test]
    fn shows_fences_when_cursor_inside_code_block() {
        let open = LineKind::FenceOpen {
            block: BlockId(0),
            lang: Some("python".to_owned()),
        };
        for reveal in [Reveal::Block, Reveal::Cursor(2)] {
            let d = line_display("```python", &open, &[], reveal);
            assert_eq!(text_of(&d, "```python"), "```python");
            assert_eq!(d.spans[0].style, SpanStyle::Syntax);
        }
        let d = line_display(
            "print(1)",
            &LineKind::Code { block: BlockId(0) },
            &[],
            Reveal::Block,
        );
        assert_eq!(text_of(&d, "print(1)"), "print(1)");
    }

    #[test]
    fn code_bg_for_a_closed_block() {
        let (_, bl) = code_lines("x\n```rs\na\nb\n```\ny");
        assert_eq!(code_bg(&bl, 0), BlockDeco::None);
        let bg = |first, last| BlockDeco::CodeBg {
            first,
            last,
            unclosed: false,
        };
        assert_eq!(code_bg(&bl, 1), bg(true, false));
        assert_eq!(code_bg(&bl, 2), bg(false, false));
        assert_eq!(code_bg(&bl, 3), bg(false, false));
        assert_eq!(code_bg(&bl, 4), bg(false, true));
        assert_eq!(code_bg(&bl, 5), BlockDeco::None);
        assert_eq!(code_bg(&bl, 99), BlockDeco::None);
    }

    #[test]
    fn code_bg_for_an_unclosed_block_running_to_the_end() {
        let (_, bl) = code_lines("```\na\nb");
        let bg = |first, last| BlockDeco::CodeBg {
            first,
            last,
            unclosed: true,
        };
        assert_eq!(code_bg(&bl, 0), bg(true, false));
        assert_eq!(code_bg(&bl, 1), bg(false, false));
        assert_eq!(code_bg(&bl, 2), bg(false, true));
    }

    #[test]
    fn code_bg_for_an_unclosed_block_ending_at_the_next_opener() {
        let (_, bl) = code_lines("```\na\n```py\nb\n```");
        assert!(matches!(bl.kind(2), LineKind::FenceOpen { .. }));
        let bg = |first, last, unclosed| BlockDeco::CodeBg {
            first,
            last,
            unclosed,
        };
        assert_eq!(code_bg(&bl, 0), bg(true, false, true));
        assert_eq!(code_bg(&bl, 1), bg(false, true, true));
        assert_eq!(code_bg(&bl, 2), bg(true, false, false));
        assert_eq!(code_bg(&bl, 3), bg(false, false, false));
        assert_eq!(code_bg(&bl, 4), bg(false, true, false));
    }

    #[test]
    fn code_bg_for_a_lone_unclosed_fence() {
        let (_, bl) = code_lines("```");
        assert_eq!(
            code_bg(&bl, 0),
            BlockDeco::CodeBg {
                first: true,
                last: true,
                unclosed: true
            }
        );
    }

    // Tables (moon_edit editor_line_test, table rendering).

    #[test]
    fn table_rows_render_with_table_deco_and_hide_the_separator() {
        let (b, bl) = code_lines("| A | **B** |\n|---|---|\n| 1 | 2 |");
        let table = match bl.kind(0) {
            LineKind::TableHeader { table } => *table,
            other => panic!("{other:?}"),
        };
        let d = line_display(
            b.line(0),
            bl.kind(0),
            &parse_inline(b.line(0)),
            Reveal::None,
        );
        assert_eq!(
            d.deco,
            BlockDeco::TableRow {
                table,
                part: TablePart::Header
            }
        );
        assert_eq!(text_of(&d, b.line(0)), "| A | B |");
        let d = line_display(
            b.line(1),
            bl.kind(1),
            &parse_inline(b.line(1)),
            Reveal::None,
        );
        assert!(d.spans.is_empty());
        assert_eq!(
            d.deco,
            BlockDeco::TableRow {
                table,
                part: TablePart::Separator
            }
        );
        let d = line_display(
            b.line(2),
            bl.kind(2),
            &parse_inline(b.line(2)),
            Reveal::None,
        );
        assert_eq!(
            d.deco,
            BlockDeco::TableRow {
                table,
                part: TablePart::Row
            }
        );
    }

    #[test]
    fn renders_raw_markdown_when_cursor_is_in_table() {
        let (b, bl) = code_lines("| A | **B** |\n|---|---|\n| 1 | 2 |");
        for i in 0..3 {
            let l = b.line(i);
            let d = line_display(l, bl.kind(i), &parse_inline(l), Reveal::Block);
            assert_eq!(text_of(&d, l), l);
            assert_eq!(d.deco, BlockDeco::None);
        }
    }

    // Image lines (moon_edit editor_line_test, image rendering).

    #[test]
    fn shows_image_when_cursor_not_on_image_line() {
        let l = "![a photo](img/p.png)";
        let d = show_line(l, Reveal::None);
        assert_eq!(
            d.deco,
            BlockDeco::Image {
                src: "img/p.png".to_owned(),
                alt: "a photo".to_owned()
            }
        );
        assert!(d.spans.is_empty());
    }

    #[test]
    fn shows_raw_markdown_when_cursor_is_on_image_line() {
        let l = "![a photo](img/p.png)";
        for c in [0, 5, l.len()] {
            let d = show_line(l, Reveal::Cursor(c));
            assert_eq!(d.deco, BlockDeco::None);
            assert_eq!(text_of(&d, l), l);
        }
    }

    // Mappings (moon_edit cursor_utils_test, escape sequences).

    #[test]
    fn hides_backslash_when_escape_not_revealed() {
        let l = r"\*text";
        let d = para(l, Reveal::None);
        assert_eq!(src_to_display(&d, 2), Some(1));
        assert_eq!(display_to_src(&d, 1), 2);
        assert_eq!(text_of(&d, l).len(), 5);
    }

    #[test]
    fn shows_backslash_when_escape_is_revealed() {
        let l = r"\*text";
        let d = para(l, Reveal::Cursor(0));
        assert_eq!(src_to_display(&d, 2), Some(2));
        assert_eq!(display_to_src(&d, 1), 1);
        assert_eq!(text_of(&d, l).len(), 6);
    }

    #[test]
    fn handles_multiple_escape_sequences() {
        let l = r"\*and\*";
        let d = para(l, Reveal::None);
        assert_eq!(src_to_display(&d, 0), Some(0));
        assert_eq!(src_to_display(&d, 1), Some(0));
        assert_eq!(src_to_display(&d, 2), Some(1));
        assert_eq!(src_to_display(&d, 5), Some(4));
        assert_eq!(src_to_display(&d, 7), Some(5));
        assert_eq!(text_of(&d, l).len(), 5);
    }

    #[test]
    fn handles_escape_in_middle_of_text() {
        let l = r"abc\*def";
        let d = para(l, Reveal::None);
        assert_eq!(src_to_display(&d, 0), Some(0));
        assert_eq!(src_to_display(&d, 3), Some(3));
        assert_eq!(src_to_display(&d, 4), Some(3));
        assert_eq!(src_to_display(&d, 5), Some(4));
        assert_eq!(src_to_display(&d, 8), Some(7));
        assert_eq!(text_of(&d, l).len(), 7);
    }

    #[test]
    fn cursor_at_escape_boundary_positions() {
        let l = r"\*x";
        let hidden = para(l, Reveal::None);
        let shown = para(l, Reveal::Cursor(1));
        for (src, h, s) in [(0, 0, 0), (1, 0, 1), (2, 1, 2), (3, 2, 3)] {
            assert_eq!(src_to_display(&hidden, src), Some(h), "hidden {src}");
            assert_eq!(src_to_display(&shown, src), Some(s), "shown {src}");
        }
    }

    #[test]
    fn mapping_inside_hidden_syntax_is_none() {
        let l = "a **b** c";
        let d = para(l, Reveal::None);
        assert_eq!(src_to_display(&d, 2), Some(2));
        assert_eq!(src_to_display(&d, 3), None);
        assert_eq!(src_to_display(&d, 4), Some(2));
        assert_eq!(src_to_display(&d, 5), Some(3));
        assert_eq!(src_to_display(&d, 6), None);
        assert_eq!(src_to_display(&d, 7), Some(3));
        assert_eq!(src_to_display(&d, 9), Some(5));
        assert_eq!(src_to_display(&d, 10), None);
    }

    #[test]
    fn display_end_maps_past_trailing_hidden_syntax() {
        let l = "a **b**";
        let d = para(l, Reveal::None);
        assert_eq!(display_to_src(&d, 3), 7);
        assert_eq!(display_to_src(&d, 99), 7);
        assert_eq!(display_to_src(&d, 0), 0);
        // A leading hidden prefix: display 0 is the first shown byte.
        let d = show("# T", &LineKind::Heading(1), Reveal::None);
        assert_eq!(display_to_src(&d, 0), 2);
        assert_eq!(src_to_display(&d, 0), Some(0));
        assert_eq!(src_to_display(&d, 1), None);
    }

    #[test]
    fn empty_line_maps_to_zero() {
        let d = para("", Reveal::None);
        assert!(d.spans.is_empty());
        assert_eq!(display_to_src(&d, 0), 0);
        assert_eq!(src_to_display(&d, 0), Some(0));
    }

    #[test]
    fn graphemes_inside_a_styled_span_map_on_char_boundaries() {
        // A decomposed accent and a ZWJ family inside bold.
        let l = "x **e\u{301}👩\u{200d}👩\u{200d}👧** y";
        for reveal in [Reveal::None, Reveal::Cursor(4)] {
            let d = para(l, reveal);
            let shown = text_of(&d, l);
            if reveal == Reveal::None {
                assert_eq!(shown, "x e\u{301}👩\u{200d}👩\u{200d}👧 y");
            }
            for db in 0..=shown.len() + 2 {
                let src = display_to_src(&d, db);
                if shown.is_char_boundary(db) {
                    assert!(l.is_char_boundary(src), "{reveal:?} display {db} -> {src}");
                    assert_eq!(src_to_display(&d, src), Some(db.min(shown.len())));
                }
            }
            for sc in 0..=l.len() + 1 {
                if let Some(db) = src_to_display(&d, sc)
                    && l.is_char_boundary(sc)
                {
                    assert!(shown.is_char_boundary(db), "{reveal:?} src {sc} -> {db}");
                }
            }
        }
        // Grapheme boundaries in the source map to grapheme boundaries.
        let d = para(l, Reveal::None);
        let shown = text_of(&d, l);
        let mut c = 0;
        while c < l.len() {
            if let Some(db) = src_to_display(&d, c) {
                assert_eq!(crate::grapheme::floor(&shown, db), db, "src {c}");
            }
            c = crate::grapheme::next(l, c);
        }
    }

    #[test]
    fn stale_elements_are_ignored_rather_than_panicking() {
        let l = "é x";
        let bad = [
            InlineEl {
                kind: InlineKind::Bold,
                syntax: 1..3,
                content: 1..2,
                parent: None,
            },
            InlineEl {
                kind: InlineKind::Italic,
                syntax: 2..40,
                content: 3..39,
                parent: Some(7),
            },
        ];
        for reveal in [Reveal::None, Reveal::Cursor(2), Reveal::Block] {
            let d = line_display(l, &LineKind::Paragraph, &bad, reveal);
            assert_eq!(text_of(&d, l), l);
        }
    }

    #[test]
    fn every_list_kind_gets_its_prefix() {
        for (l, p) in [
            ("- a", Prefix::Bullet('◦')),
            ("* a", Prefix::Bullet('◦')),
            ("7. a", Prefix::Number("7.".to_owned())),
            ("- [X] a", Prefix::Task(true)),
        ] {
            assert!(matches!(kind_of(l), LineKind::ListItem { .. }));
            assert_eq!(show_line(l, Reveal::None).prefix, p, "{l}");
        }
    }
}
