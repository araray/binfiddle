use super::Command;
use crate::utils::display;
use crate::{error::Result, BinaryData};

pub struct ReadCommand {
    range: String,
    format: String,
}

impl ReadCommand {
    pub fn new(range: String, format: String) -> Self {
        Self { range, format }
    }
}

impl Command for ReadCommand {
    fn execute(&self, data: &mut BinaryData) -> Result<()> {
        let (start, end) = crate::utils::parsing::parse_range(&self.range, data.len())?;
        let end = end.unwrap_or(data.len());

        let chunk = data.read_range(start, Some(end))?;
        let output = display::display_bytes(
            chunk.get_bytes(),
            &self.format,
            data.get_chunk_size(),
            data.get_width(),
        )?;

        println!("{}", output);
        Ok(())
    }
}
