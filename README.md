# Binfiddle

**Binary utilities for developers and hackers**

[![License](https://img.shields.io/badge/license-BSD--3--Clause-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org/)

*Version 0.10.0 | Cross-platform (Windows/Linux/macOS) | x86_64/Arm64 Support*

Binfiddle is a **developer-focused binary manipulation toolkit** designed for flexibility, modularity, and clarity. It enables inspection, patching, differential analysis, statistical analysis, and custom exploration of binary data across a variety of formats.

Whether you're reverse-engineering firmware, debugging binary protocols, analyzing malware samples, or building custom workflows for binary files, Binfiddle provides essential tools without the bloat.

## 🌐 Overview

- [Features](#features)
- [Installation](#installation)
- [Quick Start](#quick-start)
- [Command Reference](#command-reference)
- [Examples](#examples)
- [Architecture](#architecture)
- [Contributing](#contributing)
- [License](#license)

## Features

### Core Capabilities

> Personal note: This is a reimplementation of a very old, with poor command line, version in C (and - believe it or not - plain Bash) I created for my own needs. I have been trying to renovate my personal tooling and making them publicly available. Nowadays we have LLMs that help a lot on crafting nice documentation and assisting on converting my tools. For this reason, I greatly appreciate any feedback, bug reports, issues, feature requests, fixes and improvements.

| Feature | Description |
|---------|-------------|
| **Read** | Extract and display byte ranges in multiple formats |
| **Write** | Overwrite bytes at specified positions |
| **Edit** | Insert, remove, or replace byte sequences |
| **Search** | Find patterns using exact match, regex, or wildcards |
| **Analyze** | Statistical analysis: entropy, histograms, Index of Coincidence |
| **Diff** | Compare binary files with multiple output formats |
| **Patch** | Apply binary patches (works with diff --format patch output) |
| **Convert** | Text encoding conversion and line ending normalization |
| **Struct** | Parse binary data using YAML templates for structure definitions |

### Key Differentiators

| Feature | Binfiddle | Traditional Tools |
|---------|-----------|-------------------|
| **Pipeline Integration** | First-class stdin/stdout support | Often interactive-only |
| **Unified Operations** | Read/Write/Edit/Search/Analyze/Diff in single tool | Separate tools per operation |
| **Configurable Chunking** | 1-64 bit granularity | Fixed 8-bit (byte) chunks |
| **Multi-Format I/O** | hex/dec/oct/bin/ascii | Usually hex-only |
| **Script Friendly** | Deterministic, non-interactive | Often requires user interaction |
| **Built-in Analysis** | Entropy, histograms, IC analysis | Requires external tools |

### Design Principles

- **Unix Philosophy**: Composable, pipeline-friendly, text streams as interface
- **Safety by Default**: No silent data loss, explicit modification flags required
- **Determinism**: Identical inputs produce byte-identical outputs
- **Memory Safety**: Built in Rust with no buffer overflows or data races

## Installation

### From Source (Recommended)

```bash
# Install Rust toolchain (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Clone and build
git clone https://github.com/araray/binfiddle.git
cd binfiddle
cargo build --release

# Binary is at target/release/binfiddle
sudo cp target/release/binfiddle /usr/local/bin/
```

### From Releases

Download pre-built binaries from the [Releases](https://github.com/araray/binfiddle/releases) page.

Available targets:
- `x86_64-unknown-linux-gnu` (Linux x64)
- `x86_64-unknown-linux-musl` (Linux x64, static)
- `x86_64-pc-windows-gnu` (Windows x64)
- `x86_64-apple-darwin` (macOS Intel)
- `aarch64-apple-darwin` (macOS Apple Silicon)

### Build for Other Platforms

```bash
# Use the build script
./build_releases.sh --native    # Current platform only
./build_releases.sh --help      # See all options

# Or manually with cargo
rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu
```

## Quick Start

```bash
# Read first 16 bytes as hex
binfiddle -i file.bin read 0..16

# Read entire file as ASCII
binfiddle -i file.bin read .. --format ascii

# Write bytes at offset 0x100
binfiddle -i file.bin write 0x100 DEADBEEF -o modified.bin

# Search for a pattern
binfiddle -i file.bin search "7F 45 4C 46" --all

# Insert bytes at position
binfiddle -i file.bin edit insert 0x200 CAFEBABE -o modified.bin

# Analyze entropy (find encrypted/compressed sections)
binfiddle -i firmware.bin analyze entropy --block-size 4096

# Compare two binary files
binfiddle diff original.bin modified.bin --diff-format unified

# Pipeline usage
cat data.bin | binfiddle read 0..32 | grep "7f 45"
```

## Command Reference

### Global Options

| Option | Short | Description | Default |
|--------|-------|-------------|---------|
| `--input <FILE>` | `-i` | Input file (use `-` for stdin) | stdin |
| `--output <FILE>` | `-o` | Output file (use `-` for stdout) | — |
| `--in-file` | — | Modify input file in-place | false |
| `--format <FMT>` | `-f` | Output format: hex, dec, oct, bin, ascii | hex |
| `--input-format <FMT>` | — | Input value format | hex |
| `--chunk-size <BITS>` | `-c` | Bits per display chunk (1-64) | 8 |
| `--width <N>` | — | Chunks per output line (0=no wrap) | 16 |
| `--silent` | — | Suppress change diff output | false |

### Commands

#### `read <RANGE>` — Read bytes from binary data

```bash
binfiddle -i file.bin read 0..64              # Bytes 0-63
binfiddle -i file.bin read 0x100..0x200       # Hex offsets
binfiddle -i file.bin read 10..               # Byte 10 to end
binfiddle -i file.bin read ..100              # First 100 bytes
binfiddle -i file.bin read ..                 # Entire file
binfiddle -i file.bin read 42                 # Single byte at index 42
```

#### `write <POSITION> <VALUE>` — Write bytes to binary data

```bash
binfiddle -i file.bin write 0x100 DEADBEEF -o out.bin
binfiddle -i file.bin write 0 "127 69 76 70" --input-format dec -o out.bin
binfiddle -i file.bin --in-file write 16 FF   # In-place modification
```

#### `edit <OPERATION> <RANGE> [DATA]` — Structural modifications

**Insert** — Add bytes at position (data shifts right):
```bash
binfiddle -i file.bin edit insert 0x100 DEADBEEF -o out.bin
```

**Remove** — Delete byte range (data shifts left):
```bash
binfiddle -i file.bin edit remove 0x500..0x600 -o out.bin
```

**Replace** — Remove range and insert new data:
```bash
binfiddle -i file.bin edit replace 0..4 7F454C46 -o out.bin
```

#### `search <PATTERN>` — Find patterns in binary data

```bash
# Exact hex pattern
binfiddle -i file.bin search "DE AD BE EF" --all

# ASCII string
binfiddle -i file.bin search "PASSWORD" --input-format ascii --all

# Regex pattern
binfiddle -i file.bin search "[A-Z]{4}" --input-format regex --all

# Wildcard mask (? = any byte)
binfiddle -i file.bin search "DE ?? BE EF" --input-format mask --all

# Count matches only
binfiddle -i file.bin search "00 00" --all --count

# Show context around matches
binfiddle -i file.bin search "CAFE" --all --context 8

# Prevent overlapping matches
binfiddle -i file.bin search "AA AA" --all --no-overlap
```

#### `analyze <TYPE>` — Statistical analysis of binary data

Analyze binary data for entropy, byte distribution, and cryptanalysis metrics.

```bash
# Entropy analysis (find encrypted/compressed sections)
binfiddle -i firmware.bin analyze entropy --block-size 4096

# Byte frequency histogram
binfiddle -i file.bin analyze histogram

# Index of Coincidence (cryptanalysis)
binfiddle -i file.bin analyze ic --block-size 0

# Output as CSV for graphing
binfiddle -i file.bin analyze entropy --output-format csv > entropy.csv

# Output as JSON
binfiddle -i file.bin analyze histogram --output-format json
```

**Analysis Types:**

| Type | Description | Use Case |
|------|-------------|----------|
| `entropy` | Shannon entropy (0-8 bits/byte) | Find encrypted/compressed sections |
| `histogram` | Byte frequency distribution | Identify file types, encoding |
| `ic` | Index of Coincidence | Cryptanalysis, detect encryption |

**Entropy Interpretation:**

| Range | Meaning |
|-------|---------|
| 0.0 - 1.0 | Highly repetitive (null bytes, single value) |
| 1.0 - 4.0 | Text, code, structured data |
| 4.0 - 6.0 | Mixed content |
| 6.0 - 7.5 | Compressed data |
| 7.5 - 8.0 | Encrypted or random data |

#### `diff <FILE1> <FILE2>` — Compare two binary files

Compare binary files byte-by-byte and display differences in various formats.

```bash
# Simple format (one line per difference)
binfiddle diff original.bin modified.bin

# Unified format (like text diff, with context)
binfiddle diff original.bin modified.bin --diff-format unified --context 3

# Side-by-side comparison
binfiddle diff original.bin modified.bin --diff-format side-by-side

# Generate patch file (for use with binfiddle patch)
binfiddle diff original.bin modified.bin --diff-format patch > changes.patch

# Ignore specific ranges (e.g., timestamps)
binfiddle diff v1.bin v2.bin --ignore-offsets "0x0..0x10,0x100..0x110"

# With color output
binfiddle diff file1.bin file2.bin --color always

# Show summary statistics
binfiddle diff file1.bin file2.bin --summary
```

**Diff Output Formats:**

| Format | Description |
|--------|-------------|
| `simple` | One line per difference: `Offset: 0xXX != 0xYY` |
| `unified` | Unified diff with context lines and hex dump |
| `side-by-side` | Two-column parallel comparison |
| `patch` | Machine-readable format for `binfiddle patch` |

#### `convert` — Text encoding and line ending conversion

Convert text encodings and normalize line endings for embedded text data.

```bash
# Convert UTF-8 to UTF-16LE
binfiddle -i document.txt convert --to utf-16le -o document_utf16.txt

# Convert UTF-16LE to UTF-8
binfiddle -i windows_file.txt convert --from utf-16le --to utf-8 -o unix_file.txt

# Convert Windows line endings (CRLF) to Unix (LF)
binfiddle -i script.bat convert --newlines unix -o script.sh

# Add UTF-8 BOM
binfiddle -i file.txt convert --bom add -o file_with_bom.txt

# Remove BOM from a file
binfiddle -i file_with_bom.txt convert --bom remove -o file_no_bom.txt

# Full conversion: UTF-16LE → UTF-8, Unix newlines, no BOM
binfiddle -i windows_doc.txt convert \
    --from utf-16le --to utf-8 --newlines unix --bom remove \
    -o unix_doc.txt
```

**Convert Options:**

| Option | Values | Default | Description |
|--------|--------|---------|-------------|
| `--from` | utf-8, utf-16le, utf-16be, latin-1, windows-1252 | utf-8 | Source encoding |
| `--to` | utf-8, utf-16le, utf-16be, latin-1, windows-1252 | utf-8 | Target encoding |
| `--newlines` | unix, windows, mac, keep | keep | Line ending conversion |
| `--bom` | add, remove, keep | keep | BOM handling |
| `--on-error` | strict, replace, ignore | replace | Error handling mode |

#### `patch <TARGET> <PATCH_FILE>` — Apply binary patches

Apply patches generated by `binfiddle diff --format patch` or created manually.

```bash
# Apply a patch file to create a new output
binfiddle --output patched.bin patch original.bin changes.patch

# Preview changes without modifying (dry run)
binfiddle patch original.bin changes.patch --dry-run

# Modify file in-place with backup
binfiddle --in-file -i target.bin patch target.bin changes.patch --backup .bak

# Revert a patch (undo changes)
binfiddle --output reverted.bin patch patched.bin changes.patch --revert
```

**Full Diff-Patch Workflow:**

```bash
# 1. Create a patch from two files
binfiddle diff original.bin modified.bin --diff-format patch > changes.patch

# 2. Apply the patch to original to recreate modified
binfiddle --output reconstructed.bin patch original.bin changes.patch

# 3. Verify the result matches
diff modified.bin reconstructed.bin && echo "Perfect match!"
```

**Patch Options:**

| Option | Description |
|--------|-------------|
| `--backup <SUFFIX>` | Create backup before modifying (e.g., `.bak`) |
| `--dry-run` | Show what would be done without making changes |
| `--revert` | Apply patch in reverse (undo changes) |

**Patch File Format:**

```
# binfiddle patch file
# source: original.bin
# target: modified.bin
# format: OFFSET:OLD_HEX:NEW_HEX
# differences: N
#
0x00000000:de:ff
0x00000002:be:ca
```

#### `struct <TEMPLATE>` — Parse binary data using structure templates

Parse and interpret binary data according to YAML structure templates, useful for analyzing file headers, network protocols, and data structures.

```bash
# Parse an ELF file header
binfiddle -i /bin/ls struct elf_header.yaml

# List fields in a template without parsing data
binfiddle struct my_format.yaml --list-fields

# Get a specific field value
binfiddle -i firmware.bin struct header.yaml --get version

# Output as JSON
binfiddle -i data.bin struct format.yaml --output-format json

# Output as YAML
binfiddle -i data.bin struct format.yaml --output-format yaml
```

**Template Format (YAML):**

```yaml
name: MyHeader
description: Binary header structure
endian: little  # or 'big'
fields:
  - name: magic
    offset: 0x00
    size: 4
    type: hex_string
    assert: "7f454c46"  # Verify magic bytes
    description: "Magic number"
  - name: version
    offset: 0x04
    size: 2
    type: u16
    enum:
      "1": "v1.0"
      "2": "v2.0"
```

**Supported Field Types:**

| Type | Size | Description |
|------|------|-------------|
| `u8`, `u16`, `u32`, `u64` | 1/2/4/8 bytes | Unsigned integers |
| `i8`, `i16`, `i32`, `i64` | 1/2/4/8 bytes | Signed integers |
| `hex_string` | Variable | Raw bytes as hex |
| `string` | Variable | ASCII/UTF-8 string |
| `bytes` | Variable | Raw byte array |

**Struct Options:**

| Option | Description |
|--------|-------------|
| `--list-fields` | List template fields without parsing data |
| `--get <FIELD>` | Get specific field value(s) |
| `--output-format <FMT>` | Output format: human, json, yaml |

### Range Syntax

| Syntax | Meaning |
|--------|---------|
| `10` | Single byte at index 10 |
| `10..20` | Bytes 10-19 (10 bytes) |
| `10..` | Byte 10 to end of file |
| `..20` | Bytes 0-19 |
| `..` | Entire file |
| `0x100` | Hex index (256) |
| `0x100..0x200` | Hex range |

### Output Formats

| Format | Example Output |
|--------|----------------|
| `hex` | `de ad be ef` |
| `dec` | `222 173 190 239` |
| `oct` | `336 255 276 357` |
| `bin` | `11011110 10101101 10111110 11101111` |
| `ascii` | `....` (non-printable shown as `.`) |

## Examples

### Firmware Analysis

```bash
# Check ELF magic bytes
binfiddle -i firmware.bin read 0..4
# Output: 7f 45 4c 46

# Extract section as ASCII
binfiddle -i firmware.bin read 0x1000..0x1100 --format ascii

# Patch version string
binfiddle -i firmware.bin edit replace 0x200..0x210 "v2.0.0" \
    --input-format ascii -o patched.bin

# Find encrypted sections via entropy
binfiddle -i firmware.bin analyze entropy --block-size 4096
```

### Binary Diffing

```bash
# Compare two firmware versions
binfiddle diff v1.bin v2.bin --diff-format unified --context 5

# Generate a patch file
binfiddle diff original.bin modified.bin --diff-format patch > changes.patch

# Quick comparison with summary
binfiddle diff v1.bin v2.bin --summary

# Compare ignoring timestamps at offset 0x10
binfiddle diff v1.bin v2.bin --ignore-offsets "0x10..0x18"
```

### Security Analysis

```bash
# Check for high-entropy (encrypted/packed) sections
binfiddle -i suspicious.exe analyze entropy --block-size 4096 --output-format csv > entropy.csv

# Analyze byte distribution for anomalies
binfiddle -i malware.bin analyze histogram --output-format json

# Search for shellcode patterns
binfiddle -i dump.bin search "31 c0 50 68" --all --context 16
```

### Data Recovery

```bash
# Search for JPEG headers
binfiddle -i disk.img search "FF D8 FF" --all --offsets-only

# Extract data at found offset
binfiddle -i disk.img read 0x15000..0x20000 -o recovered.jpg
```

### Pipeline Integration

```bash
# Read from stdin
cat data.bin | binfiddle read 0..16

# Chain with other tools
binfiddle -i binary read 0..100 | xxd -r -p > raw.bin

# Use in scripts
MAGIC=$(binfiddle -i file.bin read 0..4)
if [ "$MAGIC" = "7f 45 4c 46" ]; then
    echo "ELF file detected"
fi
```

### Scripted Patching

```bash
#!/bin/bash
# Patch multiple offsets
OFFSETS=(0x100 0x200 0x300)
for offset in "${OFFSETS[@]}"; do
    binfiddle -i target.bin write "$offset" 90 -o temp.bin
    mv temp.bin target.bin
done
```

## Architecture

### Module Structure

```
src/
├── main.rs          # CLI entry point and argument parsing
├── lib.rs           # Library exports
├── error.rs         # Error types (BinfiddleError)
├── commands/
│   ├── mod.rs       # Command trait and exports
│   ├── read.rs      # Read command
│   ├── write.rs     # Write command
│   ├── edit.rs      # Edit command (insert/remove/replace)
│   ├── search.rs    # Search command (exact/regex/mask/parallel)
│   ├── analyze.rs   # Analyze command (entropy/histogram/IC)
│   └── diff.rs      # Diff command (simple/unified/side-by-side/patch)
└── utils/
    ├── mod.rs       # Utility exports
    ├── parsing.rs   # Range and format parsing
    └── display.rs   # Output formatting
```

### Core Data Model

```rust
pub struct BinaryData {
    data: Vec<u8>,       // Underlying byte storage
    chunk_size: usize,   // Display chunk size in bits (1-64)
    width: usize,        // Chunks per output line
}

pub enum BinarySource {
    File(PathBuf),       // Read from file
    Stdin,               // Read from stdin
    RawData(Vec<u8>),    // In-memory data
}
```

### Error Handling

All operations return `Result<T, BinfiddleError>` with specific error types:
- `Io` — File not found, permission denied, etc.
- `Parse` — Invalid hex, decimal, or format strings
- `InvalidRange` — Out of bounds or invalid range specification
- `InvalidChunkSize` — Chunk size is 0 or exceeds data
- `InvalidInput` — Unknown format or invalid input

## Contributing

Contributions are welcome!

### Building and Testing

```bash
# Run tests
cargo test

# Run with verbose output
cargo test -- --nocapture

# Build release
cargo build --release

# Build for all platforms
./build_releases.sh
```

### Code Style

- Follow Rust standard formatting (`cargo fmt`)
- Run clippy for lints (`cargo clippy`)
- Add tests for new functionality
- Document public APIs with doc comments

## Roadmap

TBD

### Completed Features

- ✅ **Read/Write/Edit** — Core binary manipulation
- ✅ **Search** — Exact, regex, and wildcard pattern matching with parallel processing
- ✅ **Analyze** — Entropy, histogram, and Index of Coincidence analysis
- ✅ **Diff** — Binary comparison with multiple output formats

### Planned Features

- **Patch** — Apply binary patches from diff output
- **Convert** — Encoding and line ending conversion
- **Struct** — Structure-aware parsing with templates
- **Memory** — Live process memory inspection

## License

This project is licensed under the BSD-3-Clause License. See [LICENSE](LICENSE) for details.

## Acknowledgments

- Built with [Rust](https://www.rust-lang.org/)
- CLI parsing by [clap](https://github.com/clap-rs/clap)
- Pattern matching by [memchr](https://github.com/BurntSushi/memchr) and [regex](https://github.com/rust-lang/regex)
- Parallel processing by [rayon](https://github.com/rayon-rs/rayon)

---

**Pro Tip**: Combine with [radare2](https://www.radare.org) for full analysis workflows:

```bash
# Extract .text section using radare2 section info
RANGE=$(rabin2 -S binary | awk '/\.text/{print $2".."$3}')
binfiddle -i binary read "$RANGE" -o text.bin

# Analyze entropy of extracted section
binfiddle -i text.bin analyze entropy --block-size 256
```
