//! Text utilities: byte-offset ↔ line/column conversion.

pub mod line_index;

pub use line_index::{LineCol, LineIndex, PositionEncoding};
