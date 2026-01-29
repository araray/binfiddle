use crate::{BinaryData, Result};

pub mod edit;
pub mod read;
pub mod search;
pub mod write;

pub use edit::{EditCommand, EditOperation};
pub use read::ReadCommand;
pub use search::{SearchCommand, SearchConfig, SearchMatch};
pub use write::WriteCommand;

pub trait Command {
    fn execute(&self, data: &mut BinaryData) -> Result<()>;
}
