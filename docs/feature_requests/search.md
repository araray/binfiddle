# Feature Proposal: `search` Subcommand for Pattern Matching in Binary Data

**Author:** Araray Velho

**Date:** 2025-03-28

**Status:** Accepted / Ready for Development

---

## 1. Overview

This document outlines the proposed **search functionality** for `binfiddle`, a binary manipulation toolkit. The goal is to provide flexible, chunk-aware pattern matching that supports multiple input/output formats (hex, dec, bin, oct, ascii, regex, mask) and works seamlessly with both files and `stdin` streams.

---

## 2. Motivation

Currently, `binfiddle` lacks built-in search capabilities, forcing users to rely on external tools like `grep`, `xxd`, or custom scripts. A native search feature would:

- ✅ Eliminate the need for intermediate tooling
- ✅ Integrate with `--chunk-size` and `--format`
- ✅ Enable binary pattern detection (e.g., firmware markers, packet headers)
- ✅ Respect piping and stream-based workflows

---

## 3. Feature Specification

### 3.1 Core Matching Modes

| Feature               | Description                                                  | Example Usage                                      |
|-----------------------|--------------------------------------------------------------|---------------------------------------------------|
| **Exact Byte Search** | Match raw bytes in hex, ascii, bin, etc.                     | `binfiddle search "A1 B2" --format hex`           |
| **Regex Matching**    | Search for regex byte patterns (e.g., `\x00{4}`)             | `binfiddle search "\x00.{4}" --input-format regex` |
| **Decimal/Octal**     | Search for numeric patterns (e.g., `1234` → bytes)           | `binfiddle search "1234" --input-format dec`      |
| **Wildcard Bitmask**  | Match partial values (e.g., `F?` to match `F0`–`FF`)         | `binfiddle search "F?" --input-format mask`       |

### 3.2 Chunk & Format Awareness

- Respects `--chunk-size`, preventing split-pattern matching unless explicitly allowed
- Output formatting via `--format` (hex, bin, dec, ascii, etc.)

### 3.3 Input Targets

| Source     | Supported | Notes                                                       |
|------------|-----------|-------------------------------------------------------------|
| File       | ✅        | Default via `--input`                                        |
| Stdin      | ✅        | Accepts piped binary streams                                 |
| Memory     | ❌ (TBD)  | Could extend `BinarySource::MemoryAddress` in the future     |

---

## 4. CLI Design

### 4.1 Usage

```bash
binfiddle search <PATTERN> \
  [--input-format <hex|dec|bin|oct|ascii|regex|mask>] \
  [--format <hex|dec|bin|oct|ascii>] \
  [--chunk-size <BITS>] \
  [--all] \          # Return all matches (default: first only)
  [--color] \        # Highlight matches
  [--input <FILE>]   # Optional (defaults to stdin)
~~~

### 4.2 Examples

```bash
# Search for hex pattern in file
binfiddle search "DE AD BE EF" --input file.bin --format hex

# Stream binary data and search for binary pattern
cat dump.bin | binfiddle search "11001100" --input-format bin --format bin

# Regex-based structured search
binfiddle search "\x00{4}." --input-format regex --format hex
```

------

## 5. Implementation Strategy

### 5.1 Parser & CLI

- Extend `main.rs` with a `Search` subcommand under `Commands`
- Accept and parse the appropriate CLI arguments
- Add a dedicated `search.rs` module under `commands/`

### 5.2 Pattern Handling

- Implement input parsing in `utils/parsing.rs`:
    - Convert hex/dec/bin/octal to raw bytes
    - Compile regex or mask-based expressions
- Warn on malformed or chunk-misaligned patterns

### 5.3 Search Engine

- For raw byte matching: use `memchr` or manual iteration
- For regex: use `regex::bytes::Regex`
- For wildcards/masks: implement a custom matching engine
- Respect chunk boundaries if `--chunk-size` is set

### 5.4 Output

- Use `display_bytes()` for formatting matches
- Consider optional offset reporting or highlight via `--color`

------

## 6. Future Extensions

- `--context <N>`: Show N bytes before/after each match
- `--pattern-name <tag>`: Named patterns for common headers or signatures
- Parallel search using `rayon` for performance on large files
- Binary patch templates via match hooks

------

## 7. Acceptance Criteria

-  Files and `stdin` are supported
-  Matches honor `--chunk-size` and formatting
-  Supports multiple input formats: hex, dec, bin, ascii, regex
-  Provides `--all` and `--color` options
-  Handles malformed inputs gracefully
-  Unit tests for parsing and match logic

------

## 8. Notes

This feature aligns closely with `binfiddle`'s mission as a developer-friendly, stream-oriented binary manipulation toolkit.
