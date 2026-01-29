# src/commands/write.rs
use super::Command;
use crate::{error::Result, BinaryData};

pub struct WriteCommand {
    position: usize,
    value: Vec<u8>,
}

impl WriteCommand {
    pub fn new(position: usize, value: Vec<u8>) -> Self {
        Self { position, value }
    }
}

impl Command for WriteCommand {
    fn execute(&self, data: &mut BinaryData) -> Result<()> {
        data.write_range(self.position, &self.value)
    }
}
