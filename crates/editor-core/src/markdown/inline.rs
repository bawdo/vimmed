//! Inline markdown elements within one line: bold, italic, strikethrough,
//! code spans, links, images and backslash escapes. Offsets are byte offsets
//! into the line; every marker is ASCII, so every range lands on a char
//! boundary.

use std::ops::Range;

use super::block::LineKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InlineKind {
    Bold,
    Italic,
    Strike,
    Code,
    /// `url` is the text between `](` and `)`.
    Link {
        url: Range<usize>,
    },
    /// `alt` is the text between `![` and `]`, `src` between `](` and `)`.
    Image {
        alt: Range<usize>,
        src: Range<usize>,
    },
    /// `\X`: the syntax is the backslash and the character, the content the
    /// character alone.
    Escape,
}

/// One inline element. `syntax` spans the markers and the content; `content`
/// is the text between the opening and closing markers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineEl {
    pub kind: InlineKind,
    pub syntax: Range<usize>,
    pub content: Range<usize>,
    /// For an italic inside a bold: the index (into the same `Vec`) of the
    /// first bold that contains it.
    pub parent: Option<usize>,
}

/// The CommonMark escapable characters this parser honours.
const ESCAPABLE: &[u8] = b"\\`*_{}[]()<>#+-.!|~";

/// Whether lines of this kind carry inline elements. Code lines (fences
/// included) and rules do not.
fn parses_inline(kind: &LineKind) -> bool {
    !matches!(
        kind,
        LineKind::Rule
            | LineKind::FenceOpen { .. }
            | LineKind::FenceClose { .. }
            | LineKind::Code { .. }
    )
}

/// Where the inline text of a line of this kind starts: after a list marker
/// (and its indentation), after quote markers, or after a heading's hashes
/// and the whitespace that follows them. 0 for other kinds.
pub(crate) fn content_start(line: &str, kind: &LineKind) -> usize {
    match kind {
        LineKind::ListItem { content_offset, .. } | LineKind::Quote { content_offset, .. } => {
            *content_offset
        }
        LineKind::Heading(_) => {
            let hashes = line.bytes().take_while(|&b| b == b'#').count();
            let rest = &line[hashes..];
            hashes + rest.len() - rest.trim_start().len()
        }
        _ => 0,
    }
}

/// The inline elements of a line of kind `kind`, for `line_display`. Empty
/// for code and rule lines. Parsing starts after the line's prefix (see
/// `content_start`), so a `*` bullet never pairs with a `*` in the text;
/// ranges are still offsets into the whole line.
///
/// This diverges from moon_edit, which parses the raw line including its
/// prefix.
pub fn parse_line(line: &str, kind: &LineKind) -> Vec<InlineEl> {
    if !parses_inline(kind) {
        return Vec::new();
    }
    let start = content_start(line, kind);
    let Some(content) = line.get(start..) else {
        return Vec::new();
    };
    let shift = |r: Range<usize>| r.start + start..r.end + start;
    parse_inline(content)
        .into_iter()
        .map(|e| InlineEl {
            kind: match e.kind {
                InlineKind::Link { url } => InlineKind::Link { url: shift(url) },
                InlineKind::Image { alt, src } => InlineKind::Image {
                    alt: shift(alt),
                    src: shift(src),
                },
                other => other,
            },
            syntax: shift(e.syntax),
            content: shift(e.content),
            parent: e.parent,
        })
        .collect()
}

/// The inline elements of `line`, sorted by `syntax.start`.
///
/// Passes run in this order, each blind to openers the earlier passes claim:
/// escapes, code spans (single backtick; a triple backtick is skipped),
/// links, images, `**`, `__`, `~~`, `_`, `*`. An escaped character never
/// opens an element. A marker with no closer stops that pass for the rest of
/// the line.
pub fn parse_inline(line: &str) -> Vec<InlineEl> {
    let t = line.as_bytes();
    let mut els = Vec::new();
    escapes(t, &mut els);
    code_spans(t, &mut els);
    links(t, &mut els);
    images(t, &mut els);
    for marker in [b"**", b"__", b"~~"] {
        paired(t, marker, &mut els);
    }
    for &marker in b"_*" {
        single(t, marker, &mut els);
    }

    // Sort by start (stable), then rewrite parent indices from pass order
    // into sorted order.
    let mut order: Vec<usize> = (0..els.len()).collect();
    order.sort_by_key(|&i| els[i].syntax.start);
    let mut sorted_at = vec![0; els.len()];
    for (new, &old) in order.iter().enumerate() {
        sorted_at[old] = new;
    }
    order
        .iter()
        .map(|&i| {
            let mut e = els[i].clone();
            e.parent = e.parent.map(|p| sorted_at[p]);
            e
        })
        .collect()
}

/// The first `needle` at or after `from`.
fn find(t: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    t.get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| i + from)
}

fn find_byte(t: &[u8], b: u8, from: usize) -> Option<usize> {
    find(t, &[b], from)
}

fn new_el(kind: InlineKind, syntax: Range<usize>, content: Range<usize>) -> InlineEl {
    InlineEl {
        kind,
        syntax,
        content,
        parent: None,
    }
}

/// Whether the byte at `at` is an escaped character.
fn escaped(at: usize, els: &[InlineEl]) -> bool {
    els.iter()
        .any(|e| e.kind == InlineKind::Escape && e.content.start == at)
}

fn in_code(at: usize, els: &[InlineEl]) -> bool {
    els.iter()
        .any(|e| e.kind == InlineKind::Code && e.syntax.contains(&at))
}

/// Whether `at` sits inside a code span or image, or inside a link's
/// `](url)` tail (link text may hold formatting).
fn claimed(at: usize, els: &[InlineEl]) -> bool {
    els.iter().any(|e| match e.kind {
        InlineKind::Code | InlineKind::Image { .. } => e.syntax.contains(&at),
        InlineKind::Link { .. } => at > e.content.end && at < e.syntax.end,
        _ => false,
    })
}

/// A backslash before an escapable character. The pair is consumed, so an
/// escaped backslash leaves the character after it live.
fn escapes(t: &[u8], els: &mut Vec<InlineEl>) {
    let mut i = 0;
    while i + 1 < t.len() {
        if t[i] == b'\\' && ESCAPABLE.contains(&t[i + 1]) {
            els.push(new_el(InlineKind::Escape, i..i + 2, i + 1..i + 2));
            i += 2;
        } else {
            i += 1;
        }
    }
}

/// Whether a backtick at `at` starts a run of three.
fn is_fence(t: &[u8], at: usize) -> bool {
    t.get(at + 1) == Some(&b'`') && t.get(at + 2) == Some(&b'`')
}

/// Single-backtick code spans. A triple backtick at either end is skipped.
fn code_spans(t: &[u8], els: &mut Vec<InlineEl>) {
    let mut from = 0;
    while let Some(open) = find_byte(t, b'`', from) {
        if escaped(open, els) {
            from = open + 1;
            continue;
        }
        if is_fence(t, open) {
            from = open + 3;
            continue;
        }
        let mut search = open + 1;
        let close = loop {
            match find_byte(t, b'`', search) {
                Some(c) if escaped(c, els) => search = c + 1,
                other => break other,
            }
        };
        let Some(close) = close else { break };
        if is_fence(t, close) {
            from = close + 3;
            continue;
        }
        if close > open + 1 {
            els.push(new_el(InlineKind::Code, open..close + 1, open + 1..close));
        }
        from = close + 1;
    }
}

/// `[text](url)`. A `[` straight after an unescaped `!` belongs to an image;
/// after an escaped `!` it opens a link.
fn links(t: &[u8], els: &mut Vec<InlineEl>) {
    let mut from = 0;
    while let Some(open) = find_byte(t, b'[', from) {
        if (open > 0 && t[open - 1] == b'!' && !escaped(open - 1, els)) || escaped(open, els) {
            from = open + 1;
            continue;
        }
        let Some(bracket) = find_byte(t, b']', open + 1) else {
            break;
        };
        if t.get(bracket + 1) != Some(&b'(') {
            from = bracket + 1;
            continue;
        }
        let Some(paren) = find_byte(t, b')', bracket + 2) else {
            break;
        };
        if !in_code(open, els) {
            els.push(new_el(
                InlineKind::Link {
                    url: bracket + 2..paren,
                },
                open..paren + 1,
                open + 1..bracket,
            ));
        }
        from = paren + 1;
    }
}

/// `![alt](src)`.
fn images(t: &[u8], els: &mut Vec<InlineEl>) {
    let mut from = 0;
    while let Some(bang) = find(t, b"![", from) {
        if escaped(bang, els) {
            from = bang + 1;
            continue;
        }
        let Some(bracket) = find_byte(t, b']', bang + 2) else {
            break;
        };
        if t.get(bracket + 1) != Some(&b'(') {
            from = bracket + 1;
            continue;
        }
        let Some(paren) = find_byte(t, b')', bracket + 2) else {
            break;
        };
        if !in_code(bang, els) {
            els.push(new_el(
                InlineKind::Image {
                    alt: bang + 2..bracket,
                    src: bracket + 2..paren,
                },
                bang..paren + 1,
                bang + 2..bracket,
            ));
        }
        from = paren + 1;
    }
}

/// Two-character markers: `**` and `__` (bold), `~~` (strikethrough).
fn paired(t: &[u8], marker: &[u8; 2], els: &mut Vec<InlineEl>) {
    let kind = if marker == b"~~" {
        InlineKind::Strike
    } else {
        InlineKind::Bold
    };
    let mut from = 0;
    while let Some(open) = find(t, marker, from) {
        if escaped(open, els) {
            from = open + 1;
            continue;
        }
        if claimed(open, els) {
            from = open + 2;
            continue;
        }
        let Some(close) = find(t, marker, open + 2) else {
            break;
        };
        if close > open + 2 {
            els.push(new_el(kind.clone(), open..close + 2, open + 2..close));
        }
        from = close + 2;
    }
}

/// The first `marker` at or after `from` with no `marker` either side.
fn find_single(t: &[u8], marker: u8, from: usize) -> Option<usize> {
    (from..t.len()).find(|&i| {
        t[i] == marker && (i == 0 || t[i - 1] != marker) && t.get(i + 1) != Some(&marker)
    })
}

/// One-character italic markers, `_` and `*`. The parent is the first bold
/// (in pass order) whose content holds the whole italic.
fn single(t: &[u8], marker: u8, els: &mut Vec<InlineEl>) {
    let mut from = 0;
    while let Some(open) = find_single(t, marker, from) {
        if escaped(open, els) || claimed(open, els) {
            from = open + 1;
            continue;
        }
        let Some(close) = find_single(t, marker, open + 1) else {
            break;
        };
        if close > open + 1 {
            let parent = els.iter().position(|e| {
                e.kind == InlineKind::Bold && e.content.start <= open && e.content.end > close
            });
            let mut e = new_el(InlineKind::Italic, open..close + 1, open + 1..close);
            e.parent = parent;
            els.push(e);
        }
        from = close + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Vec<InlineEl> {
        parse_inline(s)
    }

    fn kinds(s: &str) -> Vec<InlineKind> {
        parse(s).into_iter().map(|e| e.kind).collect()
    }

    fn one(s: &str) -> InlineEl {
        let mut els = parse(s);
        assert_eq!(els.len(), 1, "{s:?}: {els:?}");
        els.remove(0)
    }

    fn el(kind: InlineKind, syntax: Range<usize>, content: Range<usize>) -> InlineEl {
        InlineEl {
            kind,
            syntax,
            content,
            parent: None,
        }
    }

    fn url<'a>(s: &'a str, e: &InlineEl) -> &'a str {
        match &e.kind {
            InlineKind::Link { url } => &s[url.clone()],
            other => panic!("not a link: {other:?}"),
        }
    }

    fn image<'a>(s: &'a str, e: &InlineEl) -> (&'a str, &'a str) {
        match &e.kind {
            InlineKind::Image { alt, src } => (&s[alt.clone()], &s[src.clone()]),
            other => panic!("not an image: {other:?}"),
        }
    }

    // Bold.

    #[test]
    fn parses_single_bold_element() {
        assert_eq!(one("Hello **world**"), el(InlineKind::Bold, 6..15, 8..13));
    }

    #[test]
    fn parses_multiple_bold_elements() {
        assert_eq!(
            kinds("**a** and **b**"),
            [InlineKind::Bold, InlineKind::Bold]
        );
    }

    #[test]
    fn ignores_unclosed_bold() {
        assert!(parse("Hello **world").is_empty());
    }

    #[test]
    fn ignores_empty_bold() {
        assert!(parse("Hello **** world").is_empty());
    }

    #[test]
    fn parses_double_underscore_bold() {
        assert_eq!(one("Hello __world__"), el(InlineKind::Bold, 6..15, 8..13));
    }

    #[test]
    fn parses_multiple_double_underscore_bolds() {
        assert_eq!(
            kinds("__a__ and __b__"),
            [InlineKind::Bold, InlineKind::Bold]
        );
    }

    #[test]
    fn ignores_unclosed_double_underscore_bold() {
        assert!(parse("Hello __world").is_empty());
    }

    #[test]
    fn ignores_empty_double_underscore_bold() {
        assert!(parse("Hello ____ world").is_empty());
    }

    #[test]
    fn triple_asterisks_give_bold_with_a_literal_asterisk() {
        let s = "***x***";
        let e = one(s);
        assert_eq!(e, el(InlineKind::Bold, 0..6, 2..4));
        assert_eq!(&s[e.content], "*x");
    }

    // Italic.

    #[test]
    fn parses_single_italic_element() {
        assert_eq!(one("Hello _world_"), el(InlineKind::Italic, 6..13, 7..12));
    }

    #[test]
    fn parses_multiple_italic_elements() {
        assert_eq!(
            kinds("_a_ and _b_"),
            [InlineKind::Italic, InlineKind::Italic]
        );
    }

    #[test]
    fn ignores_double_underscores_for_italic() {
        assert_eq!(kinds("Hello __world__"), [InlineKind::Bold]);
    }

    #[test]
    fn ignores_unclosed_italic() {
        assert!(parse("Hello _world").is_empty());
    }

    #[test]
    fn parses_single_asterisk_italic() {
        assert_eq!(one("Hello *world*"), el(InlineKind::Italic, 6..13, 7..12));
    }

    #[test]
    fn parses_multiple_single_asterisk_italics() {
        assert_eq!(
            kinds("*a* and *b*"),
            [InlineKind::Italic, InlineKind::Italic]
        );
    }

    #[test]
    fn ignores_unclosed_single_asterisk_italic() {
        assert!(parse("Hello *world").is_empty());
    }

    #[test]
    fn ignores_double_asterisks_as_italic() {
        assert_eq!(kinds("Hello **world**"), [InlineKind::Bold]);
    }

    #[test]
    fn snake_case_italicises_the_middle_word() {
        // Deliberate (moon_edit behaviour): no intraword rule.
        let s = "snake_case_x";
        let e = one(s);
        assert_eq!(e.kind, InlineKind::Italic);
        assert_eq!(&s[e.content], "case");
    }

    #[test]
    fn unclosed_marker_stops_that_marker_for_the_rest_of_the_line() {
        assert_eq!(kinds("_a_ _b"), [InlineKind::Italic]);
        assert_eq!(kinds("**a** **b"), [InlineKind::Bold]);
        // An unclosed backtick stops code spans, not the other passes.
        assert_eq!(kinds("`a` `b **c**"), [InlineKind::Code, InlineKind::Bold]);
    }

    // Strikethrough.

    #[test]
    fn parses_single_strikethrough_element() {
        assert_eq!(one("Hello ~~world~~"), el(InlineKind::Strike, 6..15, 8..13));
    }

    #[test]
    fn parses_multiple_strikethrough_elements() {
        assert_eq!(
            kinds("~~a~~ and ~~b~~"),
            [InlineKind::Strike, InlineKind::Strike]
        );
    }

    #[test]
    fn ignores_unclosed_strikethrough() {
        assert!(parse("Hello ~~world").is_empty());
    }

    #[test]
    fn ignores_empty_strikethrough() {
        assert!(parse("Hello ~~~~ world").is_empty());
    }

    // Code spans.

    #[test]
    fn parses_single_code_span() {
        assert_eq!(one("Use `code` here"), el(InlineKind::Code, 4..10, 5..9));
    }

    #[test]
    fn parses_multiple_code_spans() {
        assert_eq!(kinds("`a` and `b`"), [InlineKind::Code, InlineKind::Code]);
    }

    #[test]
    fn ignores_code_fence_markers() {
        assert!(!kinds("```code```").contains(&InlineKind::Code));
    }

    #[test]
    fn code_spans_prevent_other_parsing_inside() {
        assert_eq!(kinds("`**not bold**`"), [InlineKind::Code]);
    }

    // Links.

    #[test]
    fn parses_single_link() {
        let s = "Click [here](http://example.com)";
        let e = one(s);
        assert_eq!((e.syntax.clone(), e.content.clone()), (6..32, 7..11));
        assert_eq!(url(s, &e), "http://example.com");
    }

    #[test]
    fn parses_link_with_simple_url() {
        let s = "[text](url)";
        assert_eq!(url(s, &one(s)), "url");
    }

    #[test]
    fn parses_multiple_links() {
        let s = "[a](1) and [b](2)";
        let els = parse(s);
        assert_eq!(els.len(), 2);
        assert_eq!(url(s, &els[0]), "1");
        assert_eq!(url(s, &els[1]), "2");
    }

    #[test]
    fn ignores_bracket_without_paren() {
        assert!(parse("This [is] not a link").is_empty());
    }

    #[test]
    fn ignores_unclosed_paren() {
        assert!(parse("[text](url").is_empty());
    }

    #[test]
    fn escaped_bang_makes_a_link() {
        let s = r"\![alt](path.png)";
        let els = parse(s);
        assert_eq!(els.len(), 2, "{els:?}");
        assert_eq!(els[0], el(InlineKind::Escape, 0..2, 1..2));
        assert_eq!(els[1].syntax, 2..17);
        assert_eq!(url(s, &els[1]), "path.png");
    }

    // Images.

    #[test]
    fn parses_image_with_alt_text_and_path() {
        let s = "![photo](images/photo.png)";
        let e = one(s);
        assert_eq!((e.syntax.clone(), e.content.clone()), (0..26, 2..7));
        assert_eq!(image(s, &e), ("photo", "images/photo.png"));
    }

    #[test]
    fn parses_image_with_percent_encoded_path() {
        let s = "![my photo](images/my%20photo.png)";
        assert_eq!(image(s, &one(s)).1, "images/my%20photo.png");
    }

    #[test]
    fn parses_image_with_empty_alt_text() {
        let s = "![](images/photo.png)";
        assert_eq!(image(s, &one(s)), ("", "images/photo.png"));
    }

    #[test]
    fn does_not_parse_image_inside_code_span() {
        assert_eq!(kinds("`![alt](path.png)`"), [InlineKind::Code]);
    }

    #[test]
    fn does_not_parse_image_with_escaped_bang() {
        let els = parse(r"\![alt](path.png)");
        assert!(
            !els.iter()
                .any(|e| matches!(e.kind, InlineKind::Image { .. }))
        );
    }

    #[test]
    fn parses_image_alongside_text() {
        let s = "See this ![photo](img.png) here";
        let e = one(s);
        assert!(matches!(e.kind, InlineKind::Image { .. }));
        assert_eq!(e.syntax, 9..26);
    }

    #[test]
    fn parses_image_with_subdirectory_path() {
        let s = "![diagram](assets/diagrams/flow.svg)";
        assert_eq!(image(s, &one(s)).1, "assets/diagrams/flow.svg");
    }

    #[test]
    fn image_with_underscores_in_filename_does_not_produce_italic() {
        let s = "![couriersplease_logo](images/couriersplease_logo.jpeg)";
        assert_eq!(
            image(s, &one(s)),
            ("couriersplease_logo", "images/couriersplease_logo.jpeg")
        );
    }

    #[test]
    fn image_with_bold_markers_in_path_does_not_produce_bold() {
        let s = "![alt](images/**star**/photo.png)";
        assert!(matches!(one(s).kind, InlineKind::Image { .. }));
    }

    #[test]
    fn link_and_image_on_same_line_are_parsed_separately() {
        let els = parse("[link](note.md) ![img](photo.png)");
        assert_eq!(els.len(), 2);
        assert!(matches!(els[0].kind, InlineKind::Link { .. }));
        assert!(matches!(els[1].kind, InlineKind::Image { .. }));
    }

    // Nesting.

    #[test]
    fn italic_nested_in_bold_sets_parent() {
        let els = parse("**bold _italic_ text**");
        assert_eq!(els.len(), 2);
        assert_eq!(els[0].kind, InlineKind::Bold);
        assert_eq!(els[1].kind, InlineKind::Italic);
        assert_eq!(els[1].parent, Some(0));
    }

    #[test]
    fn multiple_italics_in_bold() {
        let els = parse("**_a_ and _b_**");
        assert_eq!(els.len(), 3);
        assert_eq!(els[0].kind, InlineKind::Bold);
        assert_eq!(els[1].parent, Some(0));
        assert_eq!(els[2].parent, Some(0));
    }

    #[test]
    fn standalone_italic_has_no_parent() {
        let els = parse("**bold** and _italic_");
        assert_eq!(els[1].kind, InlineKind::Italic);
        assert_eq!(els[1].parent, None);
    }

    #[test]
    fn italic_parent_index_survives_sorting() {
        // The bold comes after an earlier code span in sorted order.
        let els = parse("`c` **x *y* z**");
        assert_eq!(els.len(), 3);
        assert_eq!(els[1].kind, InlineKind::Bold);
        assert_eq!(els[2].parent, Some(1));
    }

    // Mixed.

    #[test]
    fn parses_bold_italic_and_code_together() {
        assert_eq!(
            kinds("**bold** _italic_ `code`"),
            [InlineKind::Bold, InlineKind::Italic, InlineKind::Code]
        );
    }

    #[test]
    fn parses_link_with_bold_text() {
        let els = parse("[**bold link**](url)");
        assert_eq!(els.len(), 2);
        assert!(matches!(els[0].kind, InlineKind::Link { .. }));
        assert_eq!(els[1].kind, InlineKind::Bold);
    }

    #[test]
    fn elements_are_sorted_by_position() {
        let els = parse("_a_ **b** `c`");
        assert_eq!(els.len(), 3);
        assert!(
            els.windows(2)
                .all(|w| w[0].syntax.start < w[1].syntax.start)
        );
    }

    // Lines without inline formatting.

    #[test]
    fn code_and_rule_lines_carry_no_inline_elements() {
        use crate::markdown::BlockId;
        let l = "**not parsed**";
        for kind in [
            LineKind::Code { block: BlockId(0) },
            LineKind::FenceOpen {
                block: BlockId(0),
                lang: None,
            },
            LineKind::FenceClose { block: BlockId(0) },
            LineKind::Rule,
        ] {
            assert!(parse_line(l, &kind).is_empty(), "{kind:?}");
        }
        assert_eq!(parse_line(l, &LineKind::Paragraph), parse(l));
    }

    #[test]
    fn parse_line_starts_after_a_star_bullet() {
        // Divergence from moon_edit, which parses the raw line: the bullet
        // `*` would open an italic and shift every later pairing.
        let l = "* a *b* c *d*";
        let kind = LineKind::ListItem {
            indent: 0,
            kind: crate::markdown::ListKind::Bullet('*'),
            content_offset: 2,
        };
        let els = parse_line(l, &kind);
        assert_eq!(
            els,
            [
                el(InlineKind::Italic, 4..7, 5..6),
                el(InlineKind::Italic, 10..13, 11..12),
            ]
        );
        assert_eq!(&l[els[0].content.clone()], "b");
        assert_eq!(&l[els[1].content.clone()], "d");
    }

    #[test]
    fn parse_line_skips_quote_and_heading_prefixes() {
        let quote = LineKind::Quote {
            depth: 1,
            content_offset: 2,
        };
        assert_eq!(
            parse_line("> *x*", &quote),
            [el(InlineKind::Italic, 2..5, 3..4)]
        );
        assert_eq!(
            parse_line("# *x*", &LineKind::Heading(1)),
            [el(InlineKind::Italic, 2..5, 3..4)]
        );
        // Ranges and parents stay whole-line offsets and indices.
        let els = parse_line("## **a *b***", &LineKind::Heading(2));
        assert_eq!(els[0].kind, InlineKind::Bold);
        assert_eq!(els[0].syntax.start, 3);
        let l = "- **a _b_**";
        let item = LineKind::ListItem {
            indent: 0,
            kind: crate::markdown::ListKind::Bullet('-'),
            content_offset: 2,
        };
        let els = parse_line(l, &item);
        assert_eq!(els[1].parent, Some(0));
        assert_eq!(&l[els[1].content.clone()], "b");
    }

    #[test]
    fn parse_line_tolerates_a_bad_content_offset() {
        let kind = LineKind::Quote {
            depth: 1,
            content_offset: 99,
        };
        assert!(parse_line("> x", &kind).is_empty());
        // Mid-char offset: nothing to parse rather than a panic.
        let kind = LineKind::Quote {
            depth: 1,
            content_offset: 1,
        };
        assert!(parse_line("é *x*", &kind).is_empty());
    }

    // Edge cases.

    #[test]
    fn empty_string_returns_empty_list() {
        assert!(parse("").is_empty());
    }

    #[test]
    fn plain_text_returns_empty_list() {
        assert!(parse("Hello world").is_empty());
    }

    #[test]
    fn adjacent_elements() {
        assert_eq!(parse("**a****b**").len(), 2);
    }

    #[test]
    fn multibyte_text_keeps_ranges_on_char_boundaries() {
        let s = "é **👩‍👩‍👧** _ü_";
        for e in parse(s) {
            for r in [&e.syntax, &e.content] {
                assert!(s.is_char_boundary(r.start) && s.is_char_boundary(r.end));
            }
        }
        assert_eq!(kinds(s), [InlineKind::Bold, InlineKind::Italic]);
    }

    // Escapes.

    #[test]
    fn parses_escaped_asterisk() {
        let els = parse(r"not \*bold\*");
        assert_eq!(els.len(), 2);
        assert_eq!(els[0], el(InlineKind::Escape, 4..6, 5..6));
    }

    #[test]
    fn escaped_asterisks_prevent_bold_parsing() {
        assert_eq!(
            kinds(r"\*\*not bold\*\*"),
            [const { InlineKind::Escape }; 4]
        );
    }

    #[test]
    fn parses_escaped_backtick() {
        assert_eq!(kinds(r"not \`code\`"), [const { InlineKind::Escape }; 2]);
    }

    #[test]
    fn parses_escaped_underscore() {
        assert_eq!(kinds(r"not \_italic\_"), [const { InlineKind::Escape }; 2]);
    }

    #[test]
    fn parses_escaped_backslash_as_single_backslash() {
        let s = r"a \\ b";
        let e = one(s);
        assert_eq!(e, el(InlineKind::Escape, 2..4, 3..4));
        assert_eq!(&s[e.content], "\\");
    }

    #[test]
    fn escaped_backslash_does_not_escape_the_next_marker() {
        assert_eq!(kinds(r"\\*a*"), [InlineKind::Escape, InlineKind::Italic]);
    }

    #[test]
    fn backslash_before_non_escapable_char_is_literal() {
        assert!(parse(r"\a").is_empty());
    }

    #[test]
    fn parses_escaped_hash() {
        assert_eq!(kinds(r"\# not header"), [InlineKind::Escape]);
    }

    #[test]
    fn parses_all_escapable_characters() {
        for c in r"\`*_{}[]()<>#+-.!|~".chars() {
            let s = format!("\\{c}");
            assert_eq!(kinds(&s), [InlineKind::Escape], "failed for {s}");
        }
    }

    #[test]
    fn escaped_closer_still_closes() {
        // Only openers are blocked by an escape (moon_edit behaviour).
        let els = parse(r"**a\**");
        assert_eq!(els[0], el(InlineKind::Bold, 0..6, 2..4));
    }
}
