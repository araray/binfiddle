//! Bit-level addressing and reading for structural templates.
//!
//! Provides helpers to read arbitrary bit ranges from a byte slice using
//! either MSB-first (big-endian) or LSB-first (little-endian) bit ordering.

use crate::error::{BinfiddleError, Result};

use super::struct_cmd::Endianness;

/// A byte + bit address inside a binary stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BitAddress {
    /// Byte offset from the start of the data.
    pub byte: usize,
    /// Bit index inside the byte, 0-7.
    pub bit: u8,
}

impl BitAddress {
    /// Creates a new bit address. The bit index is normalized to 0-7.
    pub fn new(byte: usize, bit: u8) -> Self {
        let total_bits = byte * 8 + bit as usize;
        Self {
            byte: total_bits / 8,
            bit: (total_bits % 8) as u8,
        }
    }

    /// Returns the absolute bit index (byte * 8 + bit).
    pub fn flat(&self) -> usize {
        self.byte * 8 + self.bit as usize
    }

    /// Adds a number of bits and returns the resulting address.
    pub fn add_bits(&self, bits: usize) -> Self {
        Self::new(self.byte, self.bit + bits as u8)
    }
}

/// Reads `bit_count` bits starting at absolute bit index `bit_start`.
///
/// Bit ordering is determined by `endian`:
/// - `Endianness::Big` treats the stream as MSB-first within each byte.
///   The first bit read becomes the most significant bit of the result.
/// - `Endianness::Little` treats the stream as LSB-first within each byte.
///   The first bit read becomes the least significant bit of the result.
///
/// # Panics
/// Panics if `bit_count` is greater than 64.
pub fn read_bits(data: &[u8], bit_start: usize, bit_count: usize, endian: Endianness) -> u64 {
    assert!(
        bit_count <= 64,
        "read_bits supports at most 64 bits, got {}",
        bit_count
    );
    if bit_count == 0 {
        return 0;
    }

    let mut result: u64 = 0;

    match endian {
        Endianness::Big => {
            // MSB-first bit stream: bit 0 of the stream is the MSB of byte 0.
            for i in 0..bit_count {
                let bit_index = bit_start + i;
                let byte_idx = bit_index / 8;
                let bit_in_byte = 7 - (bit_index % 8);
                if byte_idx >= data.len() {
                    break;
                }
                let bit = ((data[byte_idx] >> bit_in_byte) & 1) as u64;
                result = (result << 1) | bit;
            }
        }
        Endianness::Little => {
            // LSB-first bit stream: bit 0 of the stream is the LSB of byte 0.
            for i in 0..bit_count {
                let bit_index = bit_start + i;
                let byte_idx = bit_index / 8;
                let bit_in_byte = bit_index % 8;
                if byte_idx >= data.len() {
                    break;
                }
                let bit = ((data[byte_idx] >> bit_in_byte) & 1) as u64;
                result |= bit << i;
            }
        }
    }

    result
}

/// Sign-extends an unsigned value of `bit_width` bits to a signed `i64`.
pub fn sign_extend(value: u64, bit_width: usize) -> Result<i64> {
    if bit_width == 0 {
        return Err(BinfiddleError::Parse(
            "Cannot sign-extend a zero-bit value".to_string(),
        ));
    }
    if bit_width >= 64 {
        return Ok(value as i64);
    }
    let sign_bit = 1u64 << (bit_width - 1);
    Ok(if value & sign_bit != 0 {
        // Negative: set all bits above bit_width to 1.
        let mask = !0u64 << bit_width;
        (value | mask) as i64
    } else {
        value as i64
    })
}

/// Computes the number of bytes spanned by a bit range.
pub fn byte_span(bit_offset: u8, bit_size: u8) -> usize {
    let start = bit_offset as usize;
    let end = start + bit_size as usize;
    end.div_ceil(8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bit_address_new_normalizes() {
        let addr = BitAddress::new(0, 10);
        assert_eq!(addr.byte, 1);
        assert_eq!(addr.bit, 2);
        assert_eq!(addr.flat(), 10);
    }

    #[test]
    fn test_bit_address_add_bits() {
        let addr = BitAddress::new(1, 3).add_bits(10);
        assert_eq!(addr.byte, 2);
        assert_eq!(addr.bit, 5);
    }

    #[test]
    fn test_read_bits_big_endian_single_msb() {
        // 0b1000_0000 => reading bit 0 (MSB) gives 1
        let data = [0x80];
        assert_eq!(read_bits(&data, 0, 1, Endianness::Big), 1);
        // reading bit 1 gives 0
        assert_eq!(read_bits(&data, 1, 1, Endianness::Big), 0);
    }

    #[test]
    fn test_read_bits_little_endian_single_lsb() {
        // 0b0000_0001 => reading bit 0 (LSB) gives 1
        let data = [0x01];
        assert_eq!(read_bits(&data, 0, 1, Endianness::Little), 1);
        // reading bit 1 gives 0
        assert_eq!(read_bits(&data, 1, 1, Endianness::Little), 0);
    }

    #[test]
    fn test_read_bits_big_endian_nibble() {
        // 0b1010_0000 => bits 0..4 MSB-first = 1010 = 10
        let data = [0xA0];
        assert_eq!(read_bits(&data, 0, 4, Endianness::Big), 0b1010);
    }

    #[test]
    fn test_read_bits_little_endian_nibble() {
        // 0x0A = 0b0000_1010
        // LSB-first: bit0=0, bit1=1, bit2=0, bit3=1 -> result = 0b1010 = 10
        let data = [0x0A];
        assert_eq!(read_bits(&data, 0, 4, Endianness::Little), 10);
    }

    #[test]
    fn test_read_bits_big_endian_across_bytes() {
        // Bytes: 0x12 0x34
        // Big-endian bit stream: 0001 0010 0011 0100
        // Read 12 bits starting at bit 4:
        // skip first 4 bits -> 0010 0011 0100 = 0x234
        let data = [0x12, 0x34];
        assert_eq!(read_bits(&data, 4, 12, Endianness::Big), 0x234);
    }

    #[test]
    fn test_read_bits_little_endian_across_bytes() {
        // Bytes: 0x12 0x34
        // Read 12 bits starting at bit 4 (LSB-first):
        // bit4..bit15 -> 1,0,0,0,0,0,1,0,1,1,0,0
        // result = 1 + 64 + 256 + 512 = 833
        let data = [0x12, 0x34];
        assert_eq!(read_bits(&data, 4, 12, Endianness::Little), 833);
    }

    #[test]
    fn test_sign_extend_positive() {
        assert_eq!(sign_extend(0b0101, 4).unwrap(), 5);
    }

    #[test]
    fn test_sign_extend_negative() {
        // 4-bit -3 = 1101
        assert_eq!(sign_extend(0b1101, 4).unwrap(), -3);
        // 3-bit -1 = 111
        assert_eq!(sign_extend(0b111, 3).unwrap(), -1);
    }

    #[test]
    fn test_byte_span() {
        assert_eq!(byte_span(0, 1), 1);
        assert_eq!(byte_span(4, 4), 1);
        assert_eq!(byte_span(4, 5), 2);
        assert_eq!(byte_span(0, 8), 1);
        assert_eq!(byte_span(0, 9), 2);
    }
}
