pub mod display;
pub mod parsing;

// Re-export for easier access
pub use display::display_bytes;
pub use parsing::{parse_bit_input, parse_input, parse_range};
