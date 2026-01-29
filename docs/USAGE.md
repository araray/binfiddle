# Binfiddle Usage Guide

This guide provides detailed usage instructions, examples, and common workflows for binfiddle.

## Table of Contents

- [Getting Started](#getting-started)
- [Basic Operations](#basic-operations)
  - [Reading Data](#reading-data)
  - [Writing Data](#writing-data)
  - [Editing Data](#editing-data)
  - [Searching Data](#searching-data)
- [Input and Output](#input-and-output)
  - [File I/O](#file-io)
  - [Pipeline Usage](#pipeline-usage)
  - [In-Place Modification](#in-place-modification)
- [Format Options](#format-options)
  - [Output Formats](#output-formats)
  - [Input Formats](#input-formats)
  - [Chunk Sizes](#chunk-sizes)
- [Range Specifications](#range-specifications)
- [Common Workflows](#common-workflows)
- [Troubleshooting](#troubleshooting)

---

## Getting Started

### Basic Invocation

```bash
binfiddle [OPTIONS] <COMMAND> [COMMAND_OPTIONS]
```

### Getting Help

```bash
binfiddle --help              # General help
binfiddle read --help         # Command-specific help
binfiddle search --help       # Search command help
binfiddle --version           # Version information
```

### First Steps

```bash
# Create a test file
echo -n "Hello, World!" > test.txt

# Read the file as hex
binfiddle -i test.txt read ..
# Output: 48 65 6c 6c 6f 2c 20 57 6f 72 6c 64 21

# Read as ASCII
binfiddle -i test.txt read .. --format ascii
# Output: H e l l o ,   W o r l d !
```

---

## Basic Operations

### Reading Data

The `read` command extracts and displays bytes from binary data.

#### Syntax

```bash
binfiddle -i <FILE> read <RANGE> [OPTIONS]
```

#### Examples

```bash
# Read first 16 bytes
binfiddle -i file.bin read 0..16

# Read bytes 256-511 (using hex offsets)
binfiddle -i file.bin read 0x100..0x200

# Read from byte 100 to end of file
binfiddle -i file.bin read 100..

# Read first 50 bytes
binfiddle -i file.bin read ..50

# Read entire file
binfiddle -i file.bin read ..

# Read single byte at offset 42
binfiddle -i file.bin read 42

# Read with different output format
binfiddle -i file.bin read 0..8 --format dec
binfiddle -i file.bin read 0..8 --format bin
binfiddle -i file.bin read 0..8 --format ascii

# Read with custom line width
binfiddle -i file.bin read 0..64 --width 8

# Read as 4-bit (nibble) chunks
binfiddle -i file.bin read 0..4 --chunk-size 4
```

### Writing Data

The `write` command overwrites bytes at a specified position without changing file size.

#### Syntax

```bash
binfiddle -i <FILE> write <POSITION> <VALUE> [OPTIONS]
```

**Important**: Write requires an output specification (`-o` or `--in-file`).

#### Examples

```bash
# Write hex bytes at offset 0x100, save to new file
binfiddle -i file.bin write 0x100 DEADBEEF -o modified.bin

# Write at offset 0, modify in place
binfiddle -i file.bin --in-file write 0 7F454C46

# Write decimal values
binfiddle -i file.bin write 0 "127 69 76 70" --input-format dec -o out.bin

# Write ASCII string
binfiddle -i file.bin write 0x200 "HELLO" --input-format ascii -o out.bin

# Write binary values
binfiddle -i file.bin write 0 "11111111 00000000" --input-format bin -o out.bin

# Suppress diff output
binfiddle -i file.bin write 0 FF --silent -o out.bin
```

#### Write Diff Output

By default, write shows the previous and new values:

```
Previous: deadbeef
New:      cafebabe
```

Use `--silent` to suppress this output.

### Editing Data

The `edit` command performs structural modifications that can change file size.

#### Operations

| Operation | Effect |
|-----------|--------|
| `insert` | Add bytes at position (shifts data right) |
| `remove` | Delete byte range (shifts data left) |
| `replace` | Remove range and insert new data |

#### Insert

```bash
# Insert at position 0x100
binfiddle -i file.bin edit insert 0x100 DEADBEEF -o modified.bin

# Insert at beginning (prepend)
binfiddle -i file.bin edit insert 0 HEADER -o modified.bin

# Insert at end (append) - use file size as position
binfiddle -i file.bin edit insert $(stat -c%s file.bin) FOOTER -o modified.bin
```

#### Remove

```bash
# Remove bytes 0x500-0x5FF (256 bytes)
binfiddle -i file.bin edit remove 0x500..0x600 -o modified.bin

# Remove first 16 bytes
binfiddle -i file.bin edit remove 0..16 -o modified.bin

# Remove last 4 bytes
LEN=$(stat -c%s file.bin)
binfiddle -i file.bin edit remove $((LEN-4)).. -o modified.bin
```

#### Replace

```bash
# Replace first 4 bytes with ELF magic
binfiddle -i file.bin edit replace 0..4 7F454C46 -o modified.bin

# Replace with shorter data (file shrinks)
binfiddle -i file.bin edit replace 0..8 CAFE -o modified.bin

# Replace with longer data (file grows)
binfiddle -i file.bin edit replace 0..2 DEADBEEFCAFE -o modified.bin
```

### Searching Data

The `search` command finds patterns in binary data.

#### Syntax

```bash
binfiddle -i <FILE> search <PATTERN> [OPTIONS]
```

#### Pattern Types

| Format | Description | Example |
|--------|-------------|---------|
| `hex` (default) | Hexadecimal bytes | `"DE AD BE EF"` |
| `ascii` | Literal string | `"PASSWORD"` |
| `dec` | Decimal bytes | `"222 173 190 239"` |
| `oct` | Octal bytes | `"336 255 276 357"` |
| `bin` | Binary bytes | `"11011110 10101101"` |
| `regex` | Regular expression | `"[A-Z]{4}"` |
| `mask` | Hex with wildcards | `"DE ?? BE EF"` |

#### Search Options

| Option | Description |
|--------|-------------|
| `--all` | Find all matches (default: first only) |
| `--count` | Output only the match count |
| `--offsets-only` | Output only match offsets |
| `--context <N>` | Show N bytes before/after each match |
| `--no-overlap` | Prevent overlapping matches |

#### Examples

```bash
# Find first occurrence
binfiddle -i file.bin search "DEADBEEF"

# Find all occurrences
binfiddle -i file.bin search "DEADBEEF" --all

# Search for ASCII string
binfiddle -i file.bin search "ERROR" --input-format ascii --all

# Count null bytes
binfiddle -i file.bin search "00 00" --all --count

# Get only offsets
binfiddle -i file.bin search "CAFE" --all --offsets-only

# Show context around matches
binfiddle -i file.bin search "7F454C46" --context 8

# Use wildcard mask (any byte in second position)
binfiddle -i file.bin search "DE ?? BE EF" --input-format mask --all

# Regex search for uppercase sequences
binfiddle -i file.bin search "[A-Z]{4,}" --input-format regex --all

# Non-overlapping search (e.g., "AA" in "AAAA" = 2 matches, not 3)
binfiddle -i file.bin search "AA AA" --all --no-overlap
```

#### Search Output Formats

**Default (with offsets and data):**
```
0x00000100: de ad be ef
0x00000250: de ad be ef
```

**Offsets only (`--offsets-only`):**
```
0x00000100
0x00000250
```

**Count only (`--count`):**
```
2
```

**With context (`--context 4`):**
```
0x00000100: 11 22 33 44 [de ad be ef] 55 66 77 88
```

---

## Input and Output

### File I/O

#### Reading from Files

```bash
# Explicit input file
binfiddle -i input.bin read 0..16

# Use "-" for stdin
cat file.bin | binfiddle -i - read 0..16

# Omit -i to read from stdin (default)
cat file.bin | binfiddle read 0..16
```

#### Writing to Files

```bash
# Explicit output file
binfiddle -i input.bin write 0 FF -o output.bin

# Use "-" for stdout (for piping)
binfiddle -i input.bin read 0..16 -o -
```

### Pipeline Usage

Binfiddle is designed for Unix pipelines:

```bash
# Read from stdin, write to stdout
cat data.bin | binfiddle read 0..16

# Chain with other tools
binfiddle -i file.bin read 0..100 | xxd -r -p > raw.bin

# Process multiple files
for f in *.bin; do
    echo "=== $f ==="
    binfiddle -i "$f" search "MAGIC" --all
done

# Use in conditionals
if binfiddle -i file.bin read 0..4 | grep -q "7f 45 4c 46"; then
    echo "ELF file"
fi
```

### In-Place Modification

Use `--in-file` to modify the input file directly:

```bash
# CAUTION: Modifies original file!
binfiddle -i config.dat --in-file write 0x10 FF

# Cannot combine with --output
binfiddle -i file.bin --in-file write 0 FF -o out.bin  # ERROR
```

**Warning**: In-place modification has no automatic backup. Create backups manually:

```bash
cp file.bin file.bin.bak
binfiddle -i file.bin --in-file write 0 FF
```

---

## Format Options

### Output Formats

Use `--format` or `-f` to control output display:

```bash
# Hexadecimal (default)
binfiddle -i file.bin read 0..4 --format hex
# Output: de ad be ef

# Decimal
binfiddle -i file.bin read 0..4 --format dec
# Output: 222 173 190 239

# Octal
binfiddle -i file.bin read 0..4 --format oct
# Output: 336 255 276 357

# Binary
binfiddle -i file.bin read 0..4 --format bin
# Output: 11011110 10101101 10111110 11101111

# ASCII (printable chars, "." for non-printable)
binfiddle -i file.bin read 0..4 --format ascii
# Output: . . . .
```

### Input Formats

Use `--input-format` to specify how value arguments are parsed:

```bash
# Hexadecimal (default)
binfiddle -i f.bin write 0 DEADBEEF -o out.bin

# Decimal
binfiddle -i f.bin write 0 "222 173 190 239" --input-format dec -o out.bin

# Octal
binfiddle -i f.bin write 0 "336 255 276 357" --input-format oct -o out.bin

# Binary
binfiddle -i f.bin write 0 "11011110 10101101" --input-format bin -o out.bin

# ASCII
binfiddle -i f.bin write 0 "Hello" --input-format ascii -o out.bin
```

### Chunk Sizes

The `--chunk-size` option controls the bit granularity of output:

```bash
# Default: 8-bit (byte) chunks
binfiddle -i file.bin read 0..2 --chunk-size 8
# Output: ab cd

# 4-bit (nibble) chunks
binfiddle -i file.bin read 0..2 --chunk-size 4
# Output: a b c d

# 16-bit (word) chunks
binfiddle -i file.bin read 0..4 --chunk-size 16
# Output: abcd ef01

# 32-bit (dword) chunks
binfiddle -i file.bin read 0..8 --chunk-size 32
# Output: abcdef01 23456789
```

### Line Width

Use `--width` to control chunks per line:

```bash
# 16 chunks per line (default)
binfiddle -i file.bin read 0..32 --width 16

# 8 chunks per line
binfiddle -i file.bin read 0..32 --width 8

# No line wrapping
binfiddle -i file.bin read 0..32 --width 0
```

---

## Range Specifications

### Syntax

| Format | Meaning | Example |
|--------|---------|---------|
| `N` | Single byte at index N | `42` → byte 42 |
| `N..M` | Bytes N to M-1 | `10..20` → bytes 10-19 |
| `N..` | Byte N to end | `100..` → byte 100 to EOF |
| `..M` | Bytes 0 to M-1 | `..50` → bytes 0-49 |
| `..` | Entire file | `..` → all bytes |

### Numeric Formats

```bash
# Decimal
binfiddle -i f.bin read 256..512

# Hexadecimal (0x prefix)
binfiddle -i f.bin read 0x100..0x200

# Hexadecimal (leading zero, no prefix)
binfiddle -i f.bin read 0100..0200
```

---

## Common Workflows

### Firmware Analysis

```bash
# Check file type by magic bytes
binfiddle -i firmware.bin read 0..4
# 7f 45 4c 46 = ELF
# 4d 5a = PE/MZ (Windows)
# 89 50 4e 47 = PNG

# Find version strings
binfiddle -i firmware.bin search "v[0-9]" --input-format regex --all --context 16

# Extract a section
binfiddle -i firmware.bin read 0x1000..0x2000 -o section.bin

# Patch a byte
binfiddle -i firmware.bin write 0x5000 90 -o patched.bin
```

### Binary Comparison

```bash
# Simple diff
diff <(binfiddle -i v1.bin read ..) <(binfiddle -i v2.bin read ..)

# Side-by-side comparison
paste <(binfiddle -i v1.bin read 0..64) <(binfiddle -i v2.bin read 0..64)

# Find patterns in both files
for f in old.bin new.bin; do
    echo "=== $f ==="
    binfiddle -i "$f" search "CONFIG" --input-format ascii --all
done
```

### Data Extraction

```bash
# Find JPEG markers
binfiddle -i disk.img search "FF D8 FF" --all --offsets-only

# Extract data at offset
binfiddle -i disk.img read 0x1000..0x5000 -o extracted.jpg

# Search and extract
OFFSET=$(binfiddle -i data.bin search "PNG" --input-format ascii --offsets-only | head -1)
binfiddle -i data.bin read ${OFFSET}.. -o image.png
```

### Automated Patching

```bash
#!/bin/bash
# patch_binary.sh - Apply multiple patches

INPUT="$1"
OUTPUT="$2"

# Copy original
cp "$INPUT" "$OUTPUT"

# Apply patches
binfiddle -i "$OUTPUT" --in-file write 0x100 90909090  # NOP sled
binfiddle -i "$OUTPUT" --in-file write 0x200 EB10      # Short jump
binfiddle -i "$OUTPUT" --in-file write 0x300 "PATCHED" --input-format ascii

echo "Patched: $OUTPUT"
```

### Integration with Other Tools

```bash
# With radare2
SECTION=$(rabin2 -S binary | awk '/\.text/{print $2".."$3}')
binfiddle -i binary read "$SECTION" -o text.bin

# With xxd (hex to binary)
binfiddle -i file.bin read 0..100 | xxd -r -p > raw.bin

# With strings
binfiddle -i file.bin read 0x1000..0x2000 | strings

# With file (identify extracted data)
binfiddle -i archive.bin read 0..1000 -o sample.bin
file sample.bin
```

---

## Troubleshooting

### Common Errors

#### "Changes were made but no output specified"

```bash
# Wrong: No output for write/edit
binfiddle -i file.bin write 0 FF

# Correct: Specify output
binfiddle -i file.bin write 0 FF -o out.bin
# Or modify in-place
binfiddle -i file.bin --in-file write 0 FF
```

#### "Index out of bounds"

```bash
# Wrong: Offset exceeds file size
binfiddle -i small.bin read 0x10000..0x20000
# Error: Index 65536 out of bounds (data length: 1024)

# Check file size first
stat -c%s small.bin
# 1024
binfiddle -i small.bin read 0..1024
```

#### "Hex input must have even number of digits"

```bash
# Wrong: Odd number of hex digits
binfiddle -i file.bin write 0 ABC -o out.bin
# Error: Hex input must have even number of digits

# Correct: Even number of digits
binfiddle -i file.bin write 0 0ABC -o out.bin
```

#### "Invalid range"

```bash
# Wrong: Start >= End
binfiddle -i file.bin read 100..50
# Error: Start index 100 must be less than end index 50

# Correct
binfiddle -i file.bin read 50..100
```

### Performance Tips

- For large files (>100MB), operations may be slower due to in-memory processing
- Use specific ranges instead of `..` for large files
- Combine operations in scripts to minimize repeated file I/O

### Getting Help

```bash
# Full help
binfiddle --help

# Command-specific help
binfiddle read --help
binfiddle write --help
binfiddle edit --help
binfiddle search --help

# Check version
binfiddle --version
```

---

## Quick Reference Card

```
┌─────────────────────────────────────────────────────────────────┐
│                    BINFIDDLE QUICK REFERENCE                    │
├─────────────────────────────────────────────────────────────────┤
│ READ                                                            │
│   binfiddle -i FILE read RANGE                                  │
│   binfiddle -i FILE read 0..64 --format hex                     │
│   cat FILE | binfiddle read 0..16                               │
├─────────────────────────────────────────────────────────────────┤
│ WRITE                                                           │
│   binfiddle -i FILE write POS VALUE -o OUT                      │
│   binfiddle -i FILE write 0x100 DEADBEEF -o out.bin             │
│   binfiddle -i FILE --in-file write 0 7F454C46                  │
├─────────────────────────────────────────────────────────────────┤
│ EDIT                                                            │
│   binfiddle -i FILE edit insert POS DATA -o OUT                 │
│   binfiddle -i FILE edit remove START..END -o OUT               │
│   binfiddle -i FILE edit replace START..END DATA -o OUT         │
├─────────────────────────────────────────────────────────────────┤
│ SEARCH                                                          │
│   binfiddle -i FILE search "PATTERN" --all                      │
│   binfiddle -i FILE search "DE ?? EF" --input-format mask       │
│   binfiddle -i FILE search "text" --input-format ascii --count  │
├─────────────────────────────────────────────────────────────────┤
│ RANGE SYNTAX                                                    │
│   10        Single byte at index 10                             │
│   10..20    Bytes 10-19 (10 bytes)                              │
│   10..      Bytes 10 to end                                     │
│   ..20      Bytes 0-19                                          │
│   ..        Entire file                                         │
│   0x100     Hex index (256)                                     │
├─────────────────────────────────────────────────────────────────┤
│ FORMATS (--format / --input-format)                             │
│   hex       DEADBEEF                                            │
│   dec       222 173 190 239                                     │
│   oct       336 255 276 357                                     │
│   bin       11011110 10101101 10111110 11101111                 │
│   ascii     .... (output only)                                  │
│   regex     [A-Z]{4} (search only)                              │
│   mask      DE ?? BE EF (search only)                           │
├─────────────────────────────────────────────────────────────────┤
│ OPTIONS                                                         │
│   -i FILE         Input file (- for stdin)                      │
│   -o FILE         Output file (- for stdout)                    │
│   --in-file       Modify input in-place                         │
│   --format FMT    Output format                                 │
│   --input-format  Input format for values                       │
│   --chunk-size N  Bits per chunk (default: 8)                   │
│   --width N       Chunks per line (default: 16)                 │
│   --silent        Suppress diff output                          │
│   --all           Find all matches (search)                     │
│   --count         Count matches only (search)                   │
│   --context N     Show N bytes context (search)                 │
└─────────────────────────────────────────────────────────────────┘
```
