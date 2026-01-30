/// src/commands/mod.rs
use crate::{BinaryData, Result};

pub mod analyze;
pub mod diff;
pub mod edit;
pub mod read;
pub mod search;
pub mod write;

pub use analyze::{
    AnalysisType, AnalyzeCommand, AnalyzeConfig, OutputFormat as AnalyzeOutputFormat,
};
pub use diff::{parse_ignore_ranges, DiffCommand, DiffConfig, DiffEntry, DiffFormat};
pub use edit::{EditCommand, EditOperation};
pub use read::ReadCommand;
pub use search::{SearchCommand, SearchConfig, SearchMatch};
pub use write::WriteCommand;

pub trait Command {
    fn execute(&self, data: &mut BinaryData) -> Result<()>;
}
