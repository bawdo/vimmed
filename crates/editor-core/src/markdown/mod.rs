//! Markdown block structure: what kind each line is (heading, list item,
//! fenced code, table row, ...), the inline elements within a line, and how
//! a line displays given where the cursor is.

pub mod block;
pub mod display;
pub mod inline;
pub mod table;

pub use block::{BlockId, Blocks, CodeBlockInfo, LineKind, ListKind, ordered_display};
pub use display::{
    BlockDeco, DisplaySpan, LineDisplay, Prefix, Reveal, SpanBase, SpanStyle, TablePart, code_bg,
    display_to_src, line_display, src_to_display,
};
pub use inline::{InlineEl, InlineKind, parse_inline, parse_line};
pub use table::{Align, TableId, TableInfo};
