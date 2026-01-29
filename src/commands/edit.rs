/// src/commands/edit.rs
use super::Command;
use crate::{error::Result, BinaryData};

pub enum EditOperation {
    Insert {
        position: usize,
        data: Vec<u8>,
    },
    Remove {
        start: usize,
        end: usize,
    },
    Replace {
        start: usize,
        end: usize,
        data: Vec<u8>,
    },
}

pub struct EditCommand {
    operation: EditOperation,
}

impl EditCommand {
    pub fn new(operation: EditOperation) -> Self {
        Self { operation }
    }
}

impl Command for EditCommand {
    fn execute(&self, data: &mut BinaryData) -> Result<()> {
        match &self.operation {
            EditOperation::Insert {
                position,
                data: new_data,
            } => data.insert_data(*position, new_data),
            EditOperation::Remove { start, end } => data.remove_range(*start, *end),
            EditOperation::Replace {
                start,
                end,
                data: new_data,
            } => {
                data.remove_range(*start, *end)?;
                data.insert_data(*start, new_data)
            }
        }
    }
}
