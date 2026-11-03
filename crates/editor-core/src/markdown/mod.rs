//! Markdown block structure: what kind each line is (heading, list item,
//! fenced code, table row, ...). Rendering, reveal and editing read these.

pub mod block;
pub mod table;

pub use block::{BlockId, Blocks, CodeBlockInfo, LineKind, ListKind, ordered_display};
pub use table::{Align, TableId, TableInfo};
