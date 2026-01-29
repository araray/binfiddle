//! Utility modules for binfiddle.
//!
//! This module provides parsing and display utilities for binary data manipulation.

pub mod display;
pub mod parsing;

// Re-export commonly used functions at the module level
pub use display::display_bytes;
pub use parsing::{parse_bit_input, parse_input, parse_range};
