//! The token table: every token's identifier, dotted name, kind and its
//! light and dark default. Names are fixed; later tasks may add rows but must
//! never rename one.
//!
//! Direction B ("quiet bar"): a calm paper-white page, one blue accent, an
//! amber for "changes not saved", grey chrome. Light values start from the
//! spike constants; dark values are the same design over a charcoal ground.
//! `font.body` and `font.mono` hold resolver sentinels, not real families.

use super::{Theme, TokenKind};

token_table! {
    // Colours: (variant, name, kind, light, dark)
    ColorBg, "color.bg", Color, "#FAF9F6", "#1C1C1F";
    ColorText, "color.text", Color, "#2B2B2B", "#E4E2DD";
    ColorTextMuted, "color.text.muted", Color, "#9A968E", "#8E8B84";
    ColorCaret, "color.caret", Color, "#3B6EA5", "#7FAEE0";
    ColorSelection, "color.selection", Color, "#3B6EA540", "#7FAEE04D";
    ColorSearchMatch, "color.search.match", Color, "#F4BF4F66", "#D9A53B55";
    ColorSearchCurrent, "color.search.current", Color, "#F4BF4FCC", "#D9A53BAA";
    ColorCodeBg, "color.code.bg", Color, "#F0EEE9", "#26262A";
    ColorCodeLabel, "color.code.label", Color, "#6A6C73", "#9C9A94";
    ColorCodeUnclosed, "color.code.unclosed", Color, "#B35C00", "#E8A04E";
    ColorRule, "color.rule", Color, "#DDDAD3", "#3A3A3E";
    ColorQuoteBar, "color.quote.bar", Color, "#D6D8DD", "#4A4A50";
    ColorLink, "color.link", Color, "#2F5FA8", "#7FAEE0";
    ColorSyntaxReveal, "color.syntax.reveal", Color, "#6A6C73", "#9C9A94";
    ColorTableBorder, "color.table.border", Color, "#DDDAD3", "#3A3A3E";
    ColorImagePlaceholder, "color.image.placeholder", Color, "#E8E6E0", "#2E2E32";
    ColorStatusBg, "color.status.bg", Color, "#F3F1EC", "#232326";
    ColorStatusText, "color.status.text", Color, "#5D5F66", "#A5A39D";
    ColorStatusDirty, "color.status.dirty", Color, "#B35C00", "#E8A04E";
    ColorStatusWarning, "color.status.warning", Color, "#A86A00", "#F0B65A";
    ColorStatusError, "color.status.error", Color, "#B3261E", "#F2877F";
    ColorOverlayBg, "color.overlay.bg", Color, "#2B2B2B", "#3A3A3F";
    ColorOverlayText, "color.overlay.text", Color, "#FAF9F6", "#E4E2DD";
    ColorPreeditUnderline, "color.preedit.underline", Color, "#3B6EA5", "#7FAEE0";

    // Syntax highlighting colours.
    ColorSyntaxKeyword, "color.syntax.keyword", Color, "#8250A8", "#C39BE8";
    ColorSyntaxString, "color.syntax.string", Color, "#3C7A3E", "#8CC98E";
    ColorSyntaxComment, "color.syntax.comment", Color, "#8A877F", "#7D7A74";
    ColorSyntaxFunction, "color.syntax.function", Color, "#2F5FA8", "#7FAEE0";
    ColorSyntaxType, "color.syntax.type", Color, "#8A5A00", "#E3B565";
    ColorSyntaxNumber, "color.syntax.number", Color, "#B35C00", "#E8A04E";
    ColorSyntaxConstant, "color.syntax.constant", Color, "#A8365A", "#E88BA6";
    ColorSyntaxOperator, "color.syntax.operator", Color, "#5D5F66", "#A5A39D";
    ColorSyntaxPunctuation, "color.syntax.punctuation", Color, "#6A6C73", "#9C9A94";
    ColorSyntaxVariable, "color.syntax.variable", Color, "#2B2B2B", "#E4E2DD";
    ColorSyntaxProperty, "color.syntax.property", Color, "#1F7A7A", "#7BCACA";
    ColorSyntaxAttribute, "color.syntax.attribute", Color, "#7A6A1F", "#CDBE6E";
    ColorSyntaxTag, "color.syntax.tag", Color, "#A8365A", "#E88BA6";

    // Fonts. The family defaults are resolver sentinels.
    FontBody, "font.body", Family, "system-ui", "system-ui";
    FontMono, "font.mono", Family, "system-mono", "system-mono";
    FontSize, "font.size", Px, "16", "16";
    FontSizeStatus, "font.size.status", Px, "11.5", "11.5";
    FontSizeH1, "font.size.h1", Px, "30", "30";
    FontSizeH2, "font.size.h2", Px, "21", "21";
    FontSizeH3, "font.size.h3", Px, "18", "18";
    FontSizeH4, "font.size.h4", Px, "16", "16";
    FontSizeH5, "font.size.h5", Px, "15", "15";
    FontSizeH6, "font.size.h6", Px, "14", "14";
    FontWeightBody, "font.weight.body", Weight, "400", "400";
    FontWeightStrong, "font.weight.strong", Weight, "700", "700";
    FontWeightHeading, "font.weight.heading", Weight, "650", "650";

    // Line heights.
    LineHeightBody, "line.height.body", Ratio, "1.5", "1.5";
    LineHeightCode, "line.height.code", Ratio, "1.4", "1.4";

    // Spacing and sizes.
    SpaceColumn, "space.column", Px, "660", "660";
    SpacePageTop, "space.page.top", Px, "40", "40";
    SpacePageSide, "space.page.side", Px, "24", "24";
    SpaceParagraph, "space.paragraph", Px, "14", "14";
    SpaceListIndent, "space.list.indent", Px, "24", "24";
    SpaceQuoteGap, "space.quote.gap", Px, "8", "8";
    SizeQuoteBar, "size.quote.bar", Px, "2", "2";
    SizeCaret, "size.caret", Px, "2", "2";
    RadiusCode, "radius.code", Px, "4", "4";
    SpaceCodePad, "space.code.pad", Px, "14", "14";
    SizeRule, "size.rule", Px, "1", "1";
    SizeStatusHeight, "size.status.height", Px, "28", "28";
    SpaceStatusPad, "space.status.pad", Px, "14", "14";
    SizeImageMaxHeight, "size.image.max_height", Px, "420", "420";
    SizeUnclosedBar, "size.unclosed.bar", Px, "3", "3";

    // Block rendering (Task 27).
    // The code block's language label, top right.
    FontSizeCodeLabel, "font.size.code.label", Px, "12", "12";
    // Table cells: horizontal and vertical padding, and the grid line width.
    SpaceTablePad, "space.table.pad", Px, "10", "10";
    SpaceTablePadY, "space.table.pad.y", Px, "4", "4";
    SizeTableBorder, "size.table.border", Px, "1", "1";
    // The least gap between a prefix glyph (a list marker, or a hanging
    // prefix) and the text after it.
    SpacePrefixGap, "space.prefix.gap", Px, "6", "6";
    // The left-margin gutter that block prefixes (`#`, `>`, list markers) hang
    // in, as iA Writer does. 0 turns hanging off.
    SizeHangingPrefix, "size.hanging_prefix", Px, "0", "0";
}
