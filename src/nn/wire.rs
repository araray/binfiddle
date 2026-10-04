//! Canonical decimal-string integers for the NN wire contracts.
//!
//! Unsigned counts, offsets, dimensions, lengths, and sequence numbers are
//! canonical decimal strings matching `0|[1-9][0-9]*`; signed strides match
//! `0|-?[1-9][0-9]*` with a declared i64 bound. Leading plus signs, whitespace,
//! leading zeroes, and `-0` are rejected. Range checking is performed here, not
//! delegated to the parser's lenient behavior.

use super::error::NnError;

/// Parse a canonical unsigned decimal string (`0|[1-9][0-9]*`) into a `u64`.
pub fn parse_decimal_u64(value: &str) -> Result<u64, NnError> {
    if !is_canonical_unsigned(value) {
        return Err(NnError::WireSyntax {
            detail: format!(
                "invalid canonical unsigned decimal integer: {}",
                crate::nn::error::brief(value)
            ),
        });
    }
    value.parse::<u64>().map_err(|_| NnError::WireSyntax {
        detail: format!(
            "unsigned decimal integer out of u64 range: {}",
            crate::nn::error::brief(value)
        ),
    })
}

/// Parse a canonical signed decimal string (`0|-?[1-9][0-9]*`) into an `i64`.
pub fn parse_decimal_i64(value: &str) -> Result<i64, NnError> {
    let digits = value.strip_prefix('-').unwrap_or(value);
    if value == "-0" || !is_canonical_unsigned(digits) {
        return Err(NnError::WireSyntax {
            detail: format!(
                "invalid canonical signed decimal integer: {}",
                crate::nn::error::brief(value)
            ),
        });
    }
    value.parse::<i64>().map_err(|_| NnError::WireSyntax {
        detail: format!(
            "signed decimal integer out of i64 range: {}",
            crate::nn::error::brief(value)
        ),
    })
}

/// Format a `u64` as its canonical decimal string.
pub fn format_decimal_u64(value: u64) -> String {
    value.to_string()
}

/// Format an `i64` as its canonical decimal string.
pub fn format_decimal_i64(value: i64) -> String {
    value.to_string()
}

fn is_canonical_unsigned(digits: &str) -> bool {
    let bytes = digits.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    if bytes.len() > 1 && bytes[0] == b'0' {
        return false;
    }
    bytes.iter().all(u8::is_ascii_digit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code_of(result: Result<u64, NnError>) -> &'static str {
        result.expect_err("expected error").code().as_str()
    }

    #[test]
    fn accepts_canonical_unsigned() {
        assert_eq!(parse_decimal_u64("0").unwrap(), 0);
        assert_eq!(parse_decimal_u64("1").unwrap(), 1);
        assert_eq!(parse_decimal_u64("18").unwrap(), 18);
        assert_eq!(parse_decimal_u64("18446744073709551615").unwrap(), u64::MAX);
    }

    #[test]
    fn rejects_non_canonical_unsigned() {
        for bad in [
            "", "01", "007", "+1", " 1", "1 ", "1_0", "-0", "-1", "1.0", "١٢", "0x10",
        ] {
            assert_eq!(
                code_of(parse_decimal_u64(bad)),
                "WIRE_SYNTAX",
                "input {bad:?}"
            );
        }
    }

    #[test]
    fn rejects_unsigned_overflow() {
        assert_eq!(
            code_of(parse_decimal_u64("18446744073709551616")),
            "WIRE_SYNTAX"
        );
        assert_eq!(
            code_of(parse_decimal_u64("99999999999999999999999999")),
            "WIRE_SYNTAX"
        );
    }

    #[test]
    fn accepts_canonical_signed() {
        assert_eq!(parse_decimal_i64("0").unwrap(), 0);
        assert_eq!(parse_decimal_i64("-1").unwrap(), -1);
        assert_eq!(parse_decimal_i64("42").unwrap(), 42);
        assert_eq!(parse_decimal_i64("-9223372036854775808").unwrap(), i64::MIN);
        assert_eq!(parse_decimal_i64("9223372036854775807").unwrap(), i64::MAX);
    }

    #[test]
    fn rejects_non_canonical_signed() {
        for bad in ["-0", "+1", "--1", "1-", "", "01", "-01", " -1", "-1 "] {
            assert_eq!(
                parse_decimal_i64(bad)
                    .expect_err("expected error")
                    .code()
                    .as_str(),
                "WIRE_SYNTAX",
                "input {bad:?}"
            );
        }
    }

    #[test]
    fn rejects_signed_overflow() {
        assert!(parse_decimal_i64("-9223372036854775809").is_err());
        assert!(parse_decimal_i64("9223372036854775808").is_err());
    }

    #[test]
    fn formats_are_canonical() {
        assert_eq!(format_decimal_u64(0), "0");
        assert_eq!(format_decimal_u64(u64::MAX), "18446744073709551615");
        assert_eq!(format_decimal_i64(i64::MIN), "-9223372036854775808");
        assert_eq!(format_decimal_i64(0), "0");
    }
}
