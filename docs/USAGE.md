# Binfiddle Usage Guide

This guide provides detailed usage instructions, examples, and common workflows for binfiddle.

## Table of Contents

- [Getting Started](#getting-started)
- [Basic Operations](#basic-operations)
  - [Reading Data](#reading-data)
  - [Writing Data](#writing-data)
  - [Editing Data](#editing-data)
  - [Searching Data](#searching-data)
  - [Hashing Data](#hashing-data)
  - [Analyzing Data](#analyzing-data)
  - [Comparing Files (Diff)](#comparing-files-diff)
  - [Converting Encodings](#converting-encodings)
  - [Applying Patches](#applying-patches)
  - [Chaining Commands](#chaining-commands)
  - [Reading Current Process Memory](#reading-current-process-memory)
  - [Parsing Structures](#parsing-structures)
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
binfiddle analyze --help      # Analyze command help
binfiddle diff --help         # Diff command help
binfiddle convert --help      # Convert command help
binfiddle patch --help        # Patch command help
binfiddle chain --help        # Chain command help
binfiddle struct --help       # Struct command help
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

| Format | Description | Example | Use When |
|--------|-------------|---------|----------|
| `hex` (default) | Hexadecimal byte sequence | `"DE AD BE EF"` | You know exact hex bytes to find |
| `ascii` | Literal ASCII string | `"PASSWORD"` | Searching for readable text strings |
| `dec` | Decimal byte values (space-separated) | `"222 173 190 239"` | Working with decimal byte values |
| `oct` | Octal byte values (space-separated) | `"336 255 276 357"` | Working with octal notation |
| `bin` | Binary strings (space-separated) | `"11011110 10101101"` | Bit-level pattern matching |
| `regex` | **Byte-pattern** regular expression (operates on raw bytes) | `"[A-Z]{4}"` | Pattern matching on byte values |
| `mask` | Hex bytes with wildcards (`??` or `XX`) | `"DE ?? BE EF"` | Hex patterns with unknown bytes |

#### ⚠️ Critical: Understanding Regex vs. Hex Patterns

**Regex operates on RAW BYTES, not hex strings!** This is a common source of confusion.

**Example of the confusion:**
```bash
# ❌ WRONG ASSUMPTION: "ff" means hex byte 0xFF
binfiddle -i file.bin search "ff" --input-format regex

# ✅ ACTUAL BEHAVIOR: "ff" matches two ASCII 'f' characters (bytes 0x66 0x66)
# This would find "fflush", "buffer", "iffe" in strings!

# ✅ TO SEARCH FOR HEX 0xFF: Use hex or mask format instead
binfiddle -i file.bin search "FF" --input-format hex --all
binfiddle -i file.bin search "FF ?? ??" --input-format mask --all
```

**How each format interprets "ff":**
- `hex`: One byte with value 0xFF
- `ascii`: Two bytes: 'f' (0x66) and 'f' (0x66)
- `regex`: Pattern matching two ASCII 'f' characters (bytes 0x66 0x66)
- `mask`: One byte with value 0xFF

**Regex examples (byte values in hex notation for clarity):**

```bash
# Find 4+ uppercase ASCII letters (bytes 0x41-0x5A)
binfiddle -i file.bin search "[A-Z]{4,}" --input-format regex --all

# Find sequences starting with NULL byte (0x00)
binfiddle -i file.bin search "^\x00.+" --input-format regex --all

# Find printable ASCII sequences (bytes 0x20-0x7E)
binfiddle -i file.bin search "[ -~]{5,}" --input-format regex --all

# Find bytes in range 0x00-0x0F followed by any byte
binfiddle -i file.bin search "[\x00-\x0F]." --input-format regex --all
```

#### Format Selection Guide

**Choose your format based on what you know:**

| You Know | Use Format | Example |
|----------|------------|---------|
| Exact hex bytes | `hex` | `"7F454C46"` for ELF magic |
| Exact text string | `ascii` | `"ERROR"` to find error messages |
| Hex pattern with gaps | `mask` | `"7F ?? 4C 46"` for ELF-like patterns |
| Byte value patterns | `regex` | `"[\x00-\xFF]{100,}"` for long sequences |
| Character patterns | `regex` | `"[A-Z][a-z]{3,}"` for capitalized words |

#### Search Examples (Corrected)

```bash
# === HEX SEARCHES ===

# Find exact hex sequence
binfiddle -i file.bin search "DEADBEEF" --all

# Find ELF magic number
binfiddle -i file.bin search "7F454C46" --all --count

# Find null word (0x0000)
binfiddle -i file.bin search "0000" --all

# === ASCII SEARCHES ===

# Search for ASCII string
binfiddle -i file.bin search "ERROR" --input-format ascii --all

# Find password strings
binfiddle -i file.bin search "password" --input-format ascii --all

# Case-sensitive ASCII search
binfiddle -i file.bin search "Password" --input-format ascii --all

# === MASK SEARCHES (Wildcards) ===

# Find ELF header with any class/data bytes
binfiddle -i file.bin search "7F 45 4C 46 ?? ??" --input-format mask --all

# Find jump instructions (0xFF followed by any 2 bytes)
binfiddle -i file.bin search "FF ?? ??" --input-format mask --all

# Find patterns with specific prefix/suffix
binfiddle -i file.bin search "DE ?? ?? EF" --input-format mask --all

# === REGEX SEARCHES (Byte Patterns) ===

# Find sequences of 4+ uppercase ASCII letters (function names, constants)
binfiddle -i file.bin search "[A-Z]{4,}" --input-format regex --all

# Find email-like patterns in memory
binfiddle -i file.bin search "[a-z0-9]+@[a-z0-9]+\.[a-z]{2,}" --input-format regex --all

# Find printable strings at least 10 chars long
binfiddle -i file.bin search "[ -~]{10,}" --input-format regex --all

# Find version strings like "v1.2.3" or "2.4.1"
binfiddle -i file.bin search "[vV]?[0-9]+\.[0-9]+\.[0-9]+" --input-format regex --all

# Find NULL-terminated strings (NULL followed by printable chars)
binfiddle -i file.bin search "\x00[ -~]{3,}" --input-format regex --all

# === ADVANCED EXAMPLES ===

# Show context around matches
binfiddle -i file.bin search "7F454C46" --context 16 --all

# Get only offsets (for scripting)
binfiddle -i file.bin search "CAFE" --all --offsets-only

# Non-overlapping search (e.g., "AA" in "AAAA" = 2 matches, not 3)
binfiddle -i file.bin search "AAAA" --all --no-overlap

# Count occurrences without displaying matches
binfiddle -i file.bin search "00" --all --count

# Combined: Find C strings, show context, use color
binfiddle -i file.bin search "char \*" --input-format ascii --all --context 16 --color always
```

#### Common Pitfalls and Solutions

**Pitfall 1: Using regex for hex patterns**
```bash
# ❌ WRONG: Looking for hex 0xFF but using regex
binfiddle -i file.bin search "ff" --input-format regex
# Finds ASCII "ff" (bytes 0x66 0x66) - like in "fflush", "buffer"

# ✅ CORRECT: Use hex or mask format
binfiddle -i file.bin search "FF" --input-format hex --all
```

**Pitfall 2: Forgetting spaces in hex input**
```bash
# ❌ WRONG: This searches for one byte 0xDE, not four bytes
binfiddle -i file.bin search "DEADBEEF"  # OK - this works (no spaces needed for hex)

# Both valid hex syntaxes:
binfiddle -i file.bin search "DEADBEEF"           # Continuous hex
binfiddle -i file.bin search "DE AD BE EF"        # Spaced hex
```

**Pitfall 3: Using ASCII format for binary data**
```bash
# ❌ WRONG: ASCII strings often contain non-printable bytes
binfiddle -i file.bin search "\x00\x01\x02" --input-format ascii
# Won't work as expected

# ✅ CORRECT: Use hex format
binfiddle -i file.bin search "000102" --input-format hex
```

**Pitfall 4: Regex special characters**
```bash
# ❌ WRONG: Unescaped dots match any character
binfiddle -i file.bin search "192.168.1.1" --input-format regex
# Matches "192X168Y1Z1" too!

# ✅ CORRECT: Escape dots or use ASCII
binfiddle -i file.bin search "192\.168\.1\.1" --input-format regex
binfiddle -i file.bin search "192.168.1.1" --input-format ascii
```

**Pitfall 5: Case sensitivity**
```bash
# Hex is case-insensitive
binfiddle -i file.bin search "deadbeef"  # Same as "DEADBEEF"

# ASCII and regex are case-sensitive
binfiddle -i file.bin search "error" --input-format ascii   # Won't find "ERROR"

# For case-insensitive ASCII, use regex
binfiddle -i file.bin search "(?i)error" --input-format regex --all
```

#### When to Use Each Format - Decision Tree

```
Need to find exact bytes you know in hex?
    → Use `hex` format

Need to find exact text string?
    → Use `ascii` format

Know some bytes but not others?
    → Use `mask` format with ??

Need to find byte value ranges or patterns?
    → Use `regex` format with byte escapes (\x00-\xFF)

Need to find ASCII character patterns (letters, numbers)?
    → Use `regex` format with [A-Z], [0-9], etc.

Need to find repeating patterns?
    → Use `regex` format with {n,m} quantifiers
```

#### Advanced: Regex Pattern Matching on Bytes

The `regex` format uses Rust's `regex::bytes` engine, which operates on raw byte sequences.

**Character Classes (Byte Ranges):**
- `[A-Z]` - Uppercase ASCII letters (bytes 0x41-0x5A)
- `[a-z]` - Lowercase ASCII letters (bytes 0x61-0x7A)
- `[0-9]` - ASCII digits (bytes 0x30-0x39)
- `[\x00-\xFF]` - Any byte (full range)
- `[\x20-\x7E]` - Printable ASCII (space through ~)

**Special Regex Constructs:**
- `.` - Any single byte
- `*` - Zero or more repetitions
- `+` - One or more repetitions
- `{n,m}` - Between n and m repetitions
- `^` - Start of data (rarely useful in binary search)
- `$` - End of data (rarely useful in binary search)
- `|` - Alternation (OR)
- `()` - Grouping
- `(?i)` - Case-insensitive flag (for ASCII characters only)

**Escaping Bytes:**
- `\x00` - NULL byte
- `\xFF` - Byte value 255
- `\x20` - Space character
- `\xHH` - Any byte in hex notation

**Example Patterns:**

```bash
# Find version strings
binfiddle search "[0-9]+\.[0-9]+\.[0-9]+" --input-format regex -i app.bin --all

# Find URL patterns
binfiddle search "https?://[^\x00]{5,}" --input-format regex -i memory.dump --all

# Find C function names (uppercase start, alphanumeric)
binfiddle search "[A-Z][A-Za-z0-9_]{3,}" --input-format regex -i binary --all

# Find UUIDs
binfiddle search "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}" --input-format regex -i file --all

# Find null-terminated strings longer than 10 chars
binfiddle search "[\x20-\x7E]{10,}\x00" --input-format regex -i data.bin --all
```

**Performance Note:** Regex searches are slower than exact matches. For simple patterns, prefer `hex`, `ascii`, or `mask` formats.

#### Search Options

| Option | Description |
|--------|-------------|
| `--all` | Find all matches (default: first only) |
| `--count` | Output only the match count |
| `--offsets-only` | Output only match offsets |
| `--context <N>` | Show N bytes before/after each match |
| `--no-overlap` | Prevent overlapping matches |
| `--color <MODE>` | Color output: always, auto, never |
| `--block-size <SIZE>` | Stream input in blocks (e.g. `64M`, `1G`) |

#### Streaming Search for Very Large Files

By default `search` memory-maps the input. For files that are larger than RAM,
or when you want to avoid paging the whole file into memory, use
`--block-size` to stream the input in fixed-size blocks. Boundary matches are
still detected.

```bash
# Search a 100 GB file without loading it whole
binfiddle -i huge.bin search "7F454C46" --all --block-size 64M

# Stop at the first match while streaming
binfiddle -i huge.bin search "CAFEBABE" --block-size 256M
```

Limitations of streaming mode:

- Only `hex`, `ascii`, `dec`, `oct`, `bin`, and `mask` patterns are supported.
  Regex patterns need an unbounded lookback window, so they require the default
  memory-mapped path.
- `--context` is disabled because the surrounding bytes are not kept in memory.

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


### Hashing Data

The `hash` command computes common digests over binary data.

#### Supported Algorithms

| Algorithm | Digest Size | Use Case |
|-----------|-------------|----------|
| `md5` | 128-bit | Legacy checksums |
| `sha1` | 160-bit | Legacy / git-style verification |
| `sha256` | 256-bit | Cryptographic verification |
| `blake3` | 256-bit | Fast, modern cryptographic hash |
| `crc32` | 32-bit | Data integrity / zip-style checksums |
| `xxhash64` | 64-bit | Fast non-cryptographic checksum |

#### Syntax

```bash
binfiddle -i <FILE> hash <ALGORITHM> [OPTIONS]
```

#### Options

| Option | Description | Default |
|--------|-------------|---------|
| `--output-format` | Output encoding (`hex`, `base64`) | `hex` |
| `--block-size <N>` | Hash non-overlapping blocks of N bytes (`0` = whole file) | `0` |
| `--stream` | Read the input incrementally instead of memory-mapping | off |
| `--read-block-size <N>` | Chunk size when `--stream` is used (supports K/M/G suffixes) | `1M` |

#### Examples

```bash
# SHA-256 of an entire file
binfiddle -i firmware.bin hash sha256

# MD5 checksum
binfiddle -i file.bin hash md5

# Per-block CRC32 (useful for finding corrupted regions)
binfiddle -i disk.img hash crc32 --block-size 4096

# BLAKE3 of a file
binfiddle -i data.bin hash blake3

# SHA-1 in base64
binfiddle -i file.bin hash sha1 --output-format base64

# xxhash64 (very fast)
binfiddle -i large.bin hash xxhash64

# Stream-hash a file that does not fit in memory
binfiddle -i huge.bin hash sha256 --stream --read-block-size 64M
```

#### Block-Based Output

When `--block-size` is set, each block is printed on its own line with its
starting offset:

```
0x00000000: a3b2c1d4
0x00001000: e5f6a7b8
...
```

### Analyzing Data

The `analyze` command provides statistical analysis of binary data.

#### Syntax

```bash
binfiddle -i <FILE> analyze <ANALYSIS_TYPE> [OPTIONS]
```

#### Analysis Types

| Type | Description |
|------|-------------|
| `entropy` | Shannon entropy (0-8 bits/byte, measures randomness) |
| `histogram` | Byte frequency distribution |
| `ic` | Index of Coincidence (cryptanalysis metric) |

#### Analyze Options

| Option | Description |
|--------|-------------|
| `--block-size <N>` | Analyze in blocks of N bytes (0 = entire file, supports K/M/G suffixes) [default: 256] |
| `--output-format <FMT>` | Output format: human, csv, json [default: human] |
| `--range <RANGE>` | Only analyze specified range |

#### Entropy Analysis

Entropy measures the randomness/disorder in data:

| Entropy Range | Typical Content |
|---------------|-----------------|
| 0.0 - 1.0 | Highly repetitive (null bytes, single value) |
| 1.0 - 4.0 | Text, code, structured data |
| 4.0 - 6.0 | Mixed content, some patterns |
| 6.0 - 7.5 | Compressed data |
| 7.5 - 8.0 | Encrypted or highly random data |

```bash
# Analyze entire file entropy
binfiddle -i file.bin analyze entropy --block-size 0

# Block-based entropy (useful for finding encrypted sections)
binfiddle -i firmware.bin analyze entropy --block-size 4096

# Output as CSV for graphing
binfiddle -i file.bin analyze entropy --block-size 1024 --output-format csv > entropy.csv

# Output as JSON
binfiddle -i file.bin analyze entropy --output-format json
```

**Sample Output (Human):**
```
=== Entropy Analysis ===
Blocks: 4
Block size: 1024 bytes
Min entropy: 0.0000 bits/byte
Max entropy: 7.8034 bits/byte
Avg entropy: 4.5000 bits/byte

--- Block Details ---
Offset 0x00000000: 0.0000 bits/byte (highly repetitive/uniform)
Offset 0x00000400: 4.2000 bits/byte (mixed content)
Offset 0x00000800: 7.8034 bits/byte (encrypted or random)
Offset 0x00000c00: 5.8000 bits/byte (mixed content)
```

#### Streaming Analyze for Huge Files

When `--block-size` is provided, `analyze` reads the input in blocks of that
size instead of memory-mapping the whole file. This makes block-based entropy
and IC analysis practical on files that are larger than RAM. Histogram mode
accumulates counts across all blocks and still returns a single global result.

```bash
# Entropy of a 100 GB disk image without paging it all in
binfiddle -i disk.img analyze entropy --block-size 64M

# Per-block IC analysis of a huge memory dump
binfiddle -i memory.dump analyze ic --block-size 16M --output-format csv
```

Limitations of streaming mode:

- `--range` is not supported because the surrounding file offsets are not kept
  in memory.
- Process-memory sources (`--process-self`, `--pid`) use the normal in-memory
  path.

#### Histogram Analysis

Byte frequency distribution shows which byte values appear most often:

```bash
# Full histogram
binfiddle -i file.bin analyze histogram

# Histogram for specific range
binfiddle -i file.bin analyze histogram --range 0x100..0x200

# CSV output for spreadsheet analysis
binfiddle -i file.bin analyze histogram --output-format csv > histogram.csv
```

**Sample Output (Human):**
```
=== Byte Frequency Histogram ===
Total bytes: 1024
Unique byte values: 42

Top 20 most frequent bytes:
Byte   Hex   Count      Percentage  Bar
─────────────────────────────────────────
' '  0x20        128   12.50%     ██████
'e'  0x65         89    8.69%     ████
't'  0x74         72    7.03%     ████
...
```

#### Index of Coincidence (IC)

IC is useful for cryptanalysis - it measures the probability that two randomly selected bytes are the same:

| IC Value | Interpretation |
|----------|----------------|
| ~0.0039 | Random/encrypted data (1/256) |
| ~0.0667 | English text |
| >0.05 | Text-like patterns |

```bash
# Calculate IC for entire file
binfiddle -i file.bin analyze ic --block-size 0

# Block-based IC
binfiddle -i file.bin analyze ic --block-size 512
```

**Sample Output:**
```
=== Index of Coincidence Analysis ===
Size: 1024 bytes
IC: 0.058100
Interpretation: text-like patterns

Reference values:
  Random data:  ~0.0039 (1/256)
  English text: ~0.0667
```

---

### Comparing Files (Diff)

The `diff` command compares two binary files byte-by-byte and displays their differences.

#### Syntax

```bash
binfiddle diff <FILE1> <FILE2> [OPTIONS]
```

#### Diff Options

| Option | Description | Default |
|--------|-------------|---------|
| `--diff-format <FMT>` | Output format: simple, unified, side-by-side, patch | simple |
| `--context <N>` | Context bytes around differences (unified/side-by-side) | 3 |
| `--color <MODE>` | Color output: always, auto, never | auto |
| `--ignore-offsets <RANGES>` | Comma-separated ranges to ignore | (none) |
| `--diff-width <N>` | Bytes per line in output | 16 |
| `--summary` | Print summary of differences | false |

#### Output Formats

| Format | Description |
|--------|-------------|
| `simple` | One line per difference: `Offset: 0xXX != 0xYY` |
| `unified` | Unified diff with context lines, similar to text diff |
| `side-by-side` | Two-column parallel comparison |
| `patch` | Machine-readable format for `binfiddle patch` command |

#### Examples

```bash
# Simple format (default) - one line per byte difference
binfiddle diff original.bin modified.bin

# Unified format with 5 lines of context
binfiddle diff original.bin modified.bin --diff-format unified --context 5

# Side-by-side comparison
binfiddle diff v1.bin v2.bin --diff-format side-by-side

# Generate patch file
binfiddle diff original.bin modified.bin --diff-format patch > changes.patch

# Ignore timestamp bytes at offset 0x10-0x18
binfiddle diff v1.bin v2.bin --ignore-offsets "0x10..0x18"

# Ignore multiple ranges
binfiddle diff v1.bin v2.bin --ignore-offsets "0x0..0x10,0x100..0x110,0x200..0x210"

# Force color output (useful for piping to less -R)
binfiddle diff file1.bin file2.bin --color always | less -R

# Show summary statistics
binfiddle diff file1.bin file2.bin --summary
```

#### Sample Output

**Simple Format:**
```
0x00000001: 0xad != 0xca
0x00000010: 0xde != 0xff
0x00000020: 0xbe != EOF
```

**Unified Format:**
```
--- file1.bin
+++ file2.bin
@@ -0x0,0x10 +0x0,0x10 @@
 0x00000000: de ad be ef ca fe ba be 00 11 22 33 44 55 66 77  |................|
-0x00000010: 88 99 aa bb cc dd ee ff 00 11 22 33 44 55 66 77  |................|
+0x00000010: 88 99 aa bb cc dd ee 00 00 11 22 33 44 55 66 77  |................|
 0x00000020: 00 11 22 33 44 55 66 77 88 99 aa bb cc dd ee ff  |................|
```

**Side-by-Side Format:**
```
file1.bin                                    | file2.bin
---------------------------------------------+---------------------------------------------
0x00000000: de ad be ef ca fe ba be          | 0x00000000: de ad be ef ca fe ba be
0x00000008: 00 11 22 33 44 55 66 77          ! 0x00000008: 00 11 22 33 44 55 66 00
```

**Patch Format:**
```
# binfiddle patch file
# source: original.bin
# target: modified.bin
# format: OFFSET:OLD_HEX:NEW_HEX
# differences: 3
#
0x00000001:ad:ca
0x00000010:de:ff
0x00000020:be:
```

**Summary Output:**
```
42 difference(s): 38 changed, 2 deleted, 2 added (file1: 1024 bytes, file2: 1026 bytes)
```

#### Use Cases

**Firmware Version Comparison:**
```bash
# Compare firmware versions, ignoring build timestamps
binfiddle diff firmware_v1.bin firmware_v2.bin \
    --diff-format unified \
    --ignore-offsets "0x10..0x18" \
    --context 5

# Generate a patch for the update
binfiddle diff firmware_v1.bin firmware_v2.bin --diff-format patch > update.patch
```

**Binary Patch Development:**
```bash
# Compare original and cracked binary
binfiddle diff original.exe cracked.exe --diff-format simple

# Find exact bytes to patch
binfiddle diff original.exe cracked.exe --summary
# Output: 3 difference(s): 3 changed, 0 deleted, 0 added
```

**Security Analysis:**
```bash
# Compare malware variants
binfiddle diff sample_a.bin sample_b.bin --diff-format side-by-side --diff-width 8
```

### Converting Encodings

The `convert` command transforms text encoding and line endings. It's essential for working with configuration files, string tables, or cross-platform text data embedded in binaries.

#### Syntax

```bash
binfiddle -i <FILE> convert [OPTIONS]
```

#### Options

| Option | Values | Default | Description |
|--------|--------|---------|-------------|
| `--from` | utf-8, utf-16le, utf-16be, latin-1, windows-1252 | utf-8 | Source encoding |
| `--to` | utf-8, utf-16le, utf-16be, latin-1, windows-1252 | utf-8 | Target encoding |
| `--newlines` | unix, windows, mac, keep | keep | Line ending conversion |
| `--bom` | add, remove, keep | keep | BOM handling |
| `--on-error` | strict, replace, ignore | replace | Error handling mode |

#### Encoding Conversion

```bash
# Convert UTF-8 to UTF-16LE
binfiddle -i document.txt convert --to utf-16le -o document_utf16.txt

# Convert UTF-16LE file (from Windows) to UTF-8
binfiddle -i windows_file.txt convert --from utf-16le --to utf-8 -o unix_file.txt

# Round-trip test (should produce identical output)
echo -n "Hello 世界" | binfiddle convert --to utf-16le | \
    binfiddle convert --from utf-16le --to utf-8
# Output: Hello 世界

# Convert Windows-1252 (extended ASCII) to UTF-8
binfiddle -i legacy.txt convert --from windows-1252 --to utf-8 -o modern.txt
```

#### Line Ending Conversion

```bash
# Convert Windows line endings (CRLF) to Unix (LF)
binfiddle -i script.bat convert --newlines unix -o script.sh

# Convert Unix line endings to Windows
binfiddle -i config.conf convert --newlines windows -o config_win.conf

# Check line endings by viewing as hex
binfiddle -i file.txt read .. --format hex | grep -E "0d 0a|0a"
# 0d 0a = CRLF (Windows)
# 0a alone = LF (Unix)
```

#### BOM Handling

```bash
# Add UTF-8 BOM (some Windows apps require this)
binfiddle -i document.txt convert --bom add -o document_bom.txt

# Remove BOM from a file
binfiddle -i file_with_bom.txt convert --bom remove -o file_no_bom.txt

# Check for BOM presence
binfiddle -i file.txt read 0..3 --format hex
# Output: ef bb bf (if UTF-8 BOM is present)
```

#### Error Handling

```bash
# Strict mode: fail on any invalid sequences
binfiddle -i mixed.bin convert --from utf-8 --on-error strict

# Replace mode (default): replace invalid chars with U+FFFD (�)
binfiddle -i mixed.bin convert --from utf-8 --on-error replace

# Ignore mode: skip invalid sequences (may lose data)
binfiddle -i mixed.bin convert --from utf-8 --on-error ignore
```

#### Combined Operations

```bash
# Full conversion: UTF-16LE with BOM → UTF-8, Unix newlines, no BOM
binfiddle -i windows_doc.txt convert \
    --from utf-16le --to utf-8 \
    --newlines unix --bom remove \
    -o unix_doc.txt

# Prepare a file for cross-platform use
binfiddle -i local.txt convert \
    --newlines unix --bom remove \
    -o portable.txt
```

#### Practical Use Cases

**Converting Legacy Files:**
```bash
# Process a batch of legacy Windows-1252 files
for f in *.txt; do
    binfiddle -i "$f" convert --from windows-1252 --to utf-8 -o "utf8_$f"
done
```

**Preparing Files for Unix Systems:**
```bash
# Convert all text files to Unix format
binfiddle -i config.ini convert --newlines unix --bom remove -o config_unix.ini
```

**Creating UTF-16 Files for Windows APIs:**
```bash
# Some Windows APIs expect UTF-16LE with BOM
binfiddle -i data.txt convert --to utf-16le --bom add -o data_win.txt
```

---

### Applying Patches

The `patch` command applies binary patches to files. Patches can be generated using `binfiddle diff --format patch` or created manually.

#### Syntax

```bash
binfiddle [GLOBAL_OPTIONS] patch <TARGET> <PATCH_FILE> [OPTIONS]
```

#### Options

| Option | Description |
|--------|-------------|
| `--backup <SUFFIX>` | Create backup with this suffix before patching (e.g., `.bak`) |
| `--dry-run` | Show what would be done without making any changes |
| `--revert` | Apply the patch in reverse (undo changes) |

#### Basic Usage

```bash
# Apply a patch file
binfiddle --output patched.bin patch original.bin changes.patch

# Preview changes without applying (dry run)
binfiddle patch original.bin changes.patch --dry-run

# Apply patch in-place with backup
binfiddle --in-file -i target.bin patch target.bin changes.patch --backup .bak

# Revert a previously applied patch
binfiddle --output reverted.bin patch patched.bin changes.patch --revert
```

#### The Diff-Patch Workflow

The `diff` and `patch` commands work together for version control of binary files:

```bash
# Step 1: Generate a patch from two files
binfiddle diff original.bin modified.bin --diff-format patch > changes.patch

# Step 2: Review the patch file
cat changes.patch
# # binfiddle patch file
# # source: original.bin
# # target: modified.bin
# # format: OFFSET:OLD_HEX:NEW_HEX
# # differences: 2
# #
# 0x00000000:de:ff
# 0x00000002:be:ca

# Step 3: Apply patch to recreate modified version
binfiddle --output reconstructed.bin patch original.bin changes.patch

# Step 4: Verify the result matches
diff modified.bin reconstructed.bin && echo "Perfect match!"
```

#### Patch File Format

Patch files use a simple line-based format:

```
# binfiddle patch file
# source: <original_file>
# target: <modified_file>
# format: OFFSET:OLD_HEX:NEW_HEX
# differences: <count>
#
0xOFFSET:OLD_HEX:NEW_HEX
```

**Format Details:**
- Lines starting with `#` are comments (ignored)
- Each data line has format: `OFFSET:OLD_HEX:NEW_HEX`
  - `OFFSET`: Hexadecimal offset (with or without `0x` prefix)
  - `OLD_HEX`: Expected bytes at offset (empty for additions)
  - `NEW_HEX`: New bytes to write (empty for deletions)

**Examples:**
```
0x00000000:de:ff         # Change byte 0xDE to 0xFF at offset 0
0x00000100:deadbeef:cafebabe  # Change 4 bytes at offset 256
0x00000200::abcd         # Add bytes at offset 512 (addition)
0x00000300:1234:         # Remove bytes at offset 768 (deletion)
```

#### Validation

The patch command validates before applying:

1. **Bounds checking**: Verifies offsets are within file bounds
2. **Content verification**: Confirms OLD_HEX matches current file content
3. **Atomic application**: Either all patches succeed or none are applied

```bash
# If validation fails, no changes are made
binfiddle patch wrong_file.bin changes.patch
# ✗ 0x00000000: de -> ff
#    Mismatch at 0x00000000: expected de, found 00
# Summary: 0 succeeded, 1 failed
# Some patches failed - no changes written
```

#### Dry Run Mode

Preview changes before applying:

```bash
binfiddle patch file.bin changes.patch --dry-run
# Dry run - no changes made:
#
# ✓ 0x00000000: de -> ff
#    OK
# ✓ 0x00000002: be -> ca
#    OK
#
# Summary: 2 succeeded, 0 failed
```

#### Creating and Reverting Patches

```bash
# Create a patch
binfiddle diff v1.bin v2.bin --diff-format patch > v1_to_v2.patch

# Apply patch (upgrade v1 → v2)
binfiddle --output upgraded.bin patch v1.bin v1_to_v2.patch

# Revert patch (downgrade v2 → v1)
binfiddle --output downgraded.bin patch v2.bin v1_to_v2.patch --revert
```

#### Backup Management

```bash
# Create backup before in-place modification
binfiddle --in-file -i config.bin patch config.bin update.patch --backup .orig

# Multiple patches with sequential backups
binfiddle --in-file -i file.bin patch file.bin patch1.patch --backup .v1
binfiddle --in-file -i file.bin patch file.bin patch2.patch --backup .v2
# Creates: file.bin.v1, file.bin.v2, and updated file.bin
```

#### Practical Use Cases

**Firmware Patching:**
```bash
# Generate patch for firmware modification
binfiddle diff original_firmware.bin patched_firmware.bin --diff-format patch > mod.patch

# Apply to other firmware instances
binfiddle --output new_patched.bin patch another_firmware.bin mod.patch
```

**Binary Delta Updates:**
```bash
# Create delta for software updates
binfiddle diff app_v1.0.exe app_v1.1.exe --diff-format patch > update_1.0_to_1.1.patch

# Apply delta update
binfiddle --output app_v1.1.exe patch app_v1.0.exe update_1.0_to_1.1.patch
```

**Batch Patching:**
```bash
# Apply same patch to multiple files
for file in *.bin; do
    binfiddle --output "patched_$file" patch "$file" fix.patch
done
```

---

### Chaining Commands

The `chain` command runs multiple binfiddle commands in sequence, passing the byte output of each step as the input to the next. This avoids shell pipe escaping and makes multi-step workflows explicit and portable.

#### Syntax

```bash
binfiddle [OPTIONS] chain --step <COMMAND> [--step <COMMAND>] ...
```

- `--step` is repeatable and required.
- Each step is parsed with shell quoting rules.
- Intermediate steps must produce byte output (e.g., `write`, `edit`, `replace`, `convert`).
- The final step may produce text output (e.g., `read`, `search`, `analyze`).

#### Examples

```bash
# Replace a header and then patch a byte
binfiddle -i firmware.bin -o patched.bin chain \
    --step "edit replace 0..4 44415431" \
    --step "write 8 00"

# Read result after modification, without shell pipes
binfiddle -i data.bin chain \
    --step "edit replace 0..8 1234567890abcdef" \
    --step "read 0..16"

# Chain from stdin
printf '\x00\x11\x22\x33' | binfiddle --input - chain \
    --step "edit replace 0..2 4142" \
    --step "read 0..4"

# Suppress intermediate diagnostics
binfiddle --silent -i data.bin -o out.bin chain \
    --step "edit replace 0..2 9999" \
    --step "write 0 42"
```

#### Important Notes

- `chain` bypasses the normal command execution path and launches each step as a subprocess connected by temporary files.
- If an intermediate step fails or produces no byte output, the chain aborts with a clear error.
- Use `--silent` to prevent intermediate commands from writing diagnostics to stderr.

---

### Process Memory

> **Experimental — Linux only**

Process memory support lets you inspect and patch memory via `/proc/<pid>/mem`. You can target the current process with `--process-self` or another same-user process with `--pid <PID>`. The `--list-regions` flag prints the target's memory map so you can pick a valid address and size.

#### Syntax

```bash
# List mapped regions
binfiddle --process-self --list-regions
binfiddle --pid <PID> --list-regions

# Read memory
binfiddle --process-self --address <ADDR> --size <N> <COMMAND> [OPTIONS]
binfiddle --pid <PID> --address <ADDR> --size <N> <COMMAND> [OPTIONS]
```

- `--process-self` targets `/proc/self/mem`.
- `--pid <PID>` targets `/proc/<PID>/mem`.
- `--list-regions` prints regions from `/proc/<PID>/maps` and exits.
- `--address` and `--size` are hex or decimal.
- `--allow-write` is required for any write back to process memory.

#### Examples

```bash
# List memory regions of the current process
binfiddle --process-self --list-regions

# List regions of another process
binfiddle --pid 1234 --list-regions

# Read 16 bytes from the current process
binfiddle --process-self --address 0x7ffd12345678 --size 16 read 0..16

# Read from another process
binfiddle --pid 1234 --address 0x7f8a1b2c3000 --size 16 read 0..16

# Search process memory for a hex pattern
binfiddle --process-self --address 0x400000 --size 0x1000 search 474343

# Overwrite 4 bytes in the current process (requires --allow-write)
binfiddle --process-self --address 0x7ffd12345678 --size 4 \
    --allow-write write 0 DEADBEEF

# Overwrite bytes in another process's writable memory
binfiddle --pid 1234 --address 0x7f8a1b2c3000 --size 4 \
    --allow-write write 0 CAFEBABE

# Force-write a read-only region in the current process (dangerous)
binfiddle --process-self --address 0x7ffd12345678 --size 4 \
    --allow-write --force-writable write 0 DEADBEEF

# Force-write a read-only region in another process (Linux x86_64, dangerous)
binfiddle --pid 1234 --address 0x7f8a1b2c3000 --size 4 \
    --allow-write --force-writable write 0 CAFEBABE

# Search process memory for an ASCII pattern
binfiddle --process-self --address 0x400000 --size 0x1000 \
    search "PASSWORD" --input-format ascii --all

# Read a range that spans an inaccessible page, filling gaps with zeros
binfiddle --process-self --address 0x7f8a1b2c3000 --size 0x2000 \
    --zero-fill-inaccessible read 0..0x2000
```

#### Process Memory Safety

- **`--allow-write` is required** for any write back to process memory. Without it, `write` and `edit` commands are rejected.
- **`--force-writable` requires `--allow-write`** and temporarily changes read-only pages to writable. Binfiddle restores the original protection after the write, but if the operation is interrupted or an error occurs the target pages may be left writable.
- **Writes are bounded**: a write that would extend past the mapped region found in `/proc/<pid>/maps` is rejected.
- **Inaccessible pages**: by default a read fails if it touches an unmapped or non-readable page. Use `--zero-fill-inaccessible` to replace those bytes with zeros (useful for `read` and `search`) or `--skip-inaccessible` to omit them (read only).
- **Cross-process access requires ptrace permissions**: reads use `process_vm_readv` and writes use `process_vm_writev` / ptrace injection. On systems with Yama LSM, check `/proc/sys/kernel/yama/ptrace_scope`:
  - `0` — unrestricted (same-user processes are traceable).
  - `1` — restricted to parent-child and direct descendants (default on many distributions).
  - `2` — administrator-only.
  - `3` — no ptrace allowed.
- **Use at your own risk**: modifying running process memory can crash the target, corrupt data, or trigger security mitigations. Always target your own processes and prefer writable regions.

#### Limitations

- **Writable regions only by default**: cross-process writes use `process_vm_writev` and can only modify already-writable memory.
- **`--force-writable`**: uses `mprotect` for `--process-self` and ptrace syscall injection for `--pid` (Linux x86_64 and aarch64). It temporarily changes page protection and restores it afterward, but it is inherently risky.
- **`--skip-inaccessible` is read-only**: it cannot be used with `search` because reported offsets would no longer match the original process address space.
- **Size must stay constant**: `insert` and `remove` are rejected because they would change the memory region size.
- **No region enumeration for other processes** beyond `--list-regions`; you must supply a valid address and size.

---

### Parsing Structures

The `struct` command parses binary data according to YAML structure templates, making it easy to analyze file headers, network protocols, and data structures.

#### Basic Structure Parsing

```bash
# Parse a binary file using a template
binfiddle struct header_template.yaml < firmware.bin

# Example output:
# Structure: Firmware Header
# Assertions: ✓ All passed
#
# Name         Offset  Size  Value          Status
# -------  ----------  ----  -------------  ------
# magic    0x00000000     4  de ad be ef    ✓
# version  0x00000004     2  2 (v2.0)
# checksum 0x00000006     4  305419896
```

#### Creating Structure Templates

Templates are defined in YAML format:

```yaml
name: MyHeader
description: Binary header structure
endian: little  # or 'big' for big-endian
fields:
  - name: magic
    offset: 0x00
    size: 4
    type: hex_string
    assert: "deadbeef"
    description: "Magic number"
  
  - name: version
    offset: 0x04
    size: 2
    type: u16
    description: "Version number"
    enum:
      "1": "v1.0"
      "2": "v2.0"
      "3": "v3.0"
  
  - name: flags
    offset: 0x06
    size: 1
    type: u8
    description: "Flags byte"
  
  - name: name
    offset: 0x08
    size: 32
    type: string
    description: "Name string"
```

#### Template Field Options

| Option | Required | Description |
|--------|----------|-------------|
| `name` | Yes | Field identifier |
| `offset` | Yes* | Byte offset or expression (default: `$@prev_end`) |
| `size` | Yes* | Size in bytes or expression (omitted for `computed` or when `bit_size` is used) |
| `bit_offset` | No | Bit index inside the byte at `offset` (0-7, default 0) |
| `bit_size` | No | Size in bits (1-64). When present, the field is read at bit precision |
| `type` | No | Data type (default: `bytes`) |
| `assert` | No | Expected hex value for validation |
| `enum` | No | Map numeric values to names |
| `description` | No | Field description |
| `display` | No | Display format override |
| `when` | No | Conditional expression for parsing |
| `value` | No | Expression for `computed` fields |
| `bitfields` | No | Extract bitfields from integer fields |
| `count` | No | Number of elements for arrays |

* `offset`/`size` can be expressions such as `$filename_length` or `$@prev_end + 4`.

#### Supported Field Types

| Type | Size | Description |
|------|------|-------------|
| `u8` | 1 byte | Unsigned 8-bit integer |
| `u16` | 2 bytes | Unsigned 16-bit integer |
| `u32` | 4 bytes | Unsigned 32-bit integer |
| `u64` | 8 bytes | Unsigned 64-bit integer |
| `i8` | 1 byte | Signed 8-bit integer |
| `i16` | 2 bytes | Signed 16-bit integer |
| `i32` | 4 bytes | Signed 32-bit integer |
| `i64` | 8 bytes | Signed 64-bit integer |
| `hex_string` | Variable | Raw bytes as hex (default display) |
| `string` | Variable | Null-terminated or fixed-length ASCII/UTF-8 |
| `bytes` | Variable | Raw byte array |
| `computed` | — | Virtual field calculated from an expression |

#### Advanced Template Features

Templates support a small expression language for dynamic parsing.

**Field References and Magic Variables**:

| Reference | Meaning |
|-----------|---------|
| `$fieldname` | Numeric value of a previously parsed field |
| `$fieldname.subfield` | Bitfield value |
| `$@current` | Current parse offset |
| `$@prev_end` | End offset of the previous field |
| `$@file_size` | Total size of the input data |
| `$@base` | Base offset of the current template |

**Dynamic offset/size**:

```yaml
fields:
  - name: filename_length
    offset: 0x1A
    size: 2
    type: u16
  - name: filename
    offset: 0x1E
    size: $filename_length
    type: string
```

**Conditional fields** (`when`):

```yaml
fields:
  - name: version
    offset: 0
    size: 1
    type: u8
  - name: extra
    offset: 1
    size: 1
    type: u8
    when: $version >= 2
```

**Computed fields**:

```yaml
fields:
  - name: total_size
    type: computed
    value: $header_size + $payload_size
```

**Bitfields**:

```yaml
fields:
  - name: flags
    offset: 0
    size: 1
    type: u8
    bitfields:
      - name: is_compressed
        bits: 0
        type: bool
      - name: compression_level
        bits: 2..5
        type: u8
```

**Counted arrays**:

```yaml
fields:
  - name: count
    offset: 0
    size: 1
    type: u8
  - name: values
    offset: 1
    size: 1
    type: u8
    count: $count
```

**Bit-level fields**:

Parse packed bit fields at arbitrary positions. Bit ordering follows the
template `endian`:

- `big` — MSB-first (network protocols, hardware registers).
- `little` — LSB-first (some file formats).

```yaml
endian: big
fields:
  - name: data_offset
    offset: 0x0C
    bit_size: 4
    type: u8

  - name: reserved
    offset: 0x0C
    bit_offset: 4
    bit_size: 3
    type: u8

  - name: ns_flag
    offset: 0x0C
    bit_offset: 7
    bit_size: 1
    type: u8
```

#### List Template Fields

```bash
# List all fields without parsing data
binfiddle struct elf_header.yaml --list-fields

# Output:
# Template: ELF Header
# Endianness: Little
# Total size: 48 bytes
# Fields: 11
#
# Name           Offset  Size  Type        Description
# -----------  --------  ----  ----------  -----------
# e_ident      0x000000     4  HexString   Magic number
# e_class      0x000004     1  U8          32/64-bit flag
# ...
```

#### Getting Specific Fields

```bash
# Get a single field value
binfiddle struct elf_header.yaml --get e_entry < /bin/ls
# Output: 286416

# Get multiple fields
binfiddle struct elf_header.yaml --get e_type --get e_machine < /bin/ls
```

#### Output Formats

```bash
# Human-readable table (default)
binfiddle struct template.yaml --output-format human < data.bin

# JSON output
binfiddle struct template.yaml --output-format json < data.bin

# YAML output
binfiddle struct template.yaml --output-format yaml < data.bin
```

#### Assertions and Validation

Templates can include assertions to validate expected values:

```yaml
fields:
  - name: magic
    offset: 0x00
    size: 4
    type: hex_string
    assert: "7f454c46"  # Must match ELF magic
```

If an assertion fails, it's marked with ✗ in the output:
```
magic    0x00000000     4  de ad be ef    ✗ FAIL
```

The command exits with code 1 if any assertions fail (unless `--silent` is used).

#### Example: ELF Header Template

```yaml
name: ELF Header
description: ELF file header (64-bit)
endian: little
fields:
  - name: e_ident_magic
    offset: 0x00
    size: 4
    type: hex_string
    assert: "7f454c46"
    description: "ELF magic number"
  
  - name: e_ident_class
    offset: 0x04
    size: 1
    type: u8
    description: "32/64-bit flag"
    enum:
      "1": "32-bit"
      "2": "64-bit"
  
  - name: e_ident_data
    offset: 0x05
    size: 1
    type: u8
    description: "Endianness"
    enum:
      "1": "Little Endian"
      "2": "Big Endian"
  
  - name: e_type
    offset: 0x10
    size: 2
    type: u16
    description: "Object file type"
    enum:
      "1": "Relocatable"
      "2": "Executable"
      "3": "Shared object"
      "4": "Core file"
  
  - name: e_entry
    offset: 0x18
    size: 8
    type: u64
    description: "Entry point address"
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

#### Memory-Mapped File Input

File input is memory-mapped using `memmap2` instead of being loaded entirely
into RAM. This means binfiddle can open files that are larger than available
memory for read-only operations such as `read`, `search`, `analyze`, and `diff`.

```bash
# Search a multi-gigabyte firmware image without loading it whole
binfiddle -i firmware.bin search "7F454C46" --all

# Analyze entropy of a large dump with minimal resident memory
binfiddle -i memory.dump analyze entropy --block-size 4096
```

When a command mutates the data (`write`, `edit`), the mapped region is lazily
copied into an owned in-memory buffer first. Size-changing edits (`insert`,
`remove`, `replace`) therefore still require enough memory to hold the modified
data.

#### Large Files

For best results with very large inputs:

- Prefer read-only commands (`read`, `search`, `analyze`, `diff`) — they stream
  pages from disk through the OS cache on demand.
- Use `--in-file` or `-o` for writes only when necessary; writes copy the file
  into memory before modifying it.
- Avoid running `convert` on files larger than RAM because `convert` reads the
  entire input to produce a transformed output buffer.

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

**In-place `write` uses a mutable memory map.** When `--in-file` is combined
with the `write` command, binfiddle maps the file read-write and flushes each
change directly to disk. This avoids copying the entire file into memory, so
you can patch large files efficiently. Size-changing operations (`insert`,
`remove`, `replace`) still require an in-memory copy and are written back when
finished.

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

# Raw binary (no formatting — writes bytes directly to stdout)
binfiddle -i file.bin read 0..4 --format raw | file -
binfiddle -i file.bin read 0..4 --format raw > header.bin
```

### Offset Display (xxd-style)

Use `--show-offset` to add hex address prefixes on each output line, or
`--show-ascii` to also include an ASCII sidebar:

```bash
# Address prefixes only
binfiddle -i /bin/ls read 0..64 --show-offset
# Output:
# 0x0000: 7f 45 4c 46 02 01 01 00 00 00 00 00 00 00 00 00
# 0x0010: 03 00 3e 00 01 00 00 00 40 10 00 00 00 00 00 00
# 0x0020: 40 00 00 00 00 00 00 00 e8 e1 02 00 00 00 00 00
# 0x0030: 00 00 00 00 40 00 38 00 0d 00 40 00 1f 00 1e 00

# With ASCII sidebar (classic xxd/hexdump style)
binfiddle -i /bin/ls read 0..64 --show-ascii
# Output:
# 0x0000: 7f 45 4c 46 02 01 01 00 00 00 00 00 00 00 00 00  |.ELF............|
# 0x0010: 03 00 3e 00 01 00 00 00 40 10 00 00 00 00 00 00  |..>.....@.......|
# 0x0020: 40 00 00 00 00 00 00 00 e8 e1 02 00 00 00 00 00  |@...............|
# 0x0030: 00 00 00 00 40 00 38 00 0d 00 40 00 1f 00 1e 00  |....@.8...@.....|

# Offset display respects the range start offset
binfiddle -i firmware.bin read 0x1000..0x1040 --show-ascii
# Output starts from 0x1000, not 0x0000

# Combine with other formats
binfiddle -i file.bin read 0..32 --format dec --show-offset
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

# Analyze entropy to find encrypted sections
binfiddle -i firmware.bin analyze entropy --block-size 4096
```

### Binary Comparison

```bash
# Compare two firmware versions
binfiddle diff v1.bin v2.bin --diff-format unified --context 5

# Generate a patch file
binfiddle diff original.bin modified.bin --diff-format patch > changes.patch

# Quick summary of differences
binfiddle diff v1.bin v2.bin --summary

# Compare ignoring timestamp at offset 0x10-0x18
binfiddle diff v1.bin v2.bin --ignore-offsets "0x10..0x18"

# Side-by-side comparison with color
binfiddle diff v1.bin v2.bin --diff-format side-by-side --color always | less -R
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

### Security Analysis

```bash
# Find high-entropy (encrypted/packed) sections
binfiddle -i suspicious.exe analyze entropy --block-size 4096 --output-format csv > entropy.csv

# Analyze byte distribution
binfiddle -i malware.bin analyze histogram --output-format json

# Search for shellcode patterns
binfiddle -i dump.bin search "31 c0 50 68" --all --context 16

# Compare malware samples
binfiddle diff sample_a.bin sample_b.bin --diff-format side-by-side
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
- Search uses parallel processing automatically for files >1MB
- Use specific ranges instead of `..` for large files
- Combine operations in scripts to minimize repeated file I/O
- Use `--output-format csv` or `--output-format json` for analyze when processing with other tools

### Getting Help

```bash
# Full help
binfiddle --help

# Command-specific help
binfiddle read --help
binfiddle write --help
binfiddle edit --help
binfiddle search --help
binfiddle analyze --help
binfiddle diff --help

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
│ ANALYZE                                                         │
│   binfiddle -i FILE analyze entropy --block-size 4096           │
│   binfiddle -i FILE analyze histogram --output-format csv       │
│   binfiddle -i FILE analyze ic --block-size 0                   │
├─────────────────────────────────────────────────────────────────┤
│ DIFF                                                            │
│   binfiddle diff FILE1 FILE2 --diff-format simple               │
│   binfiddle diff FILE1 FILE2 --diff-format unified --context 5  │
│   binfiddle diff FILE1 FILE2 --diff-format patch > changes.patch│
│   binfiddle diff FILE1 FILE2 --ignore-offsets "0x0..0x10"       │
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
│   raw       Raw bytes, no formatting (output only)              │
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
│   --show-offset   Show hex address prefix on each line          │
│   --show-ascii    Show ASCII sidebar (implies --show-offset)    │
│   --silent        Suppress diff output                          │
│   --all           Find all matches (search)                     │
│   --count         Count matches only (search)                   │
│   --context N     Show N bytes context (search/diff)            │
│   --color MODE    Color output: always, auto, never             │
└─────────────────────────────────────────────────────────────────┘
```
