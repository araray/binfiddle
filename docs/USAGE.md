# Binfiddle User Guide

*Version 0.27.0*

This guide is the definitive reference for **binfiddle**, a Rust binary manipulation toolkit. It covers every command, option, I/O mode, streaming/block feature, and real-world workflow.

> **Quick example**
> ```bash
> binfiddle -i file.bin read 0..16
> binfiddle -i file.bin search "7F454C46" --all
> binfiddle -i firmware.bin analyze entropy --block-size 4096
> ```

---

## Table of Contents

- [Getting Started](#getting-started)
  - [Basic Invocation](#basic-invocation)
  - [Getting Help](#getting-help)
  - [First Steps](#first-steps)
- [Global Options](#global-options)
  - [Input/Output](#inputoutput)
  - [Display](#display)
  - [Process Memory Source](#process-memory-source)
- [Command Reference](#command-reference)
  - [read](#read) — read bytes
  - [write](#write) — overwrite bytes
  - [edit](#edit) — insert, remove, replace
  - [search](#search) — exact, regex, mask
  - [hash](#hash) — digests and checksums
  - [analyze](#analyze) — entropy, histogram, IC
  - [diff](#diff) — compare files
  - [convert](#convert) — encodings and line endings
  - [patch](#patch) — apply binary patches
  - [chain](#chain) — multi-step workflows
  - [struct](#struct) — template-based parsing
  - [Process Memory](#process-memory) — Linux live memory
- [Memory-Mapped Input, Streaming & Large Files](#memory-mapped-input-streaming--large-files)
  - [Memory-Mapped File Input](#memory-mapped-file-input)
  - [In-Place Modification with `MmapMut`](#in-place-modification-with-mmapmut)
  - [Streaming Search](#streaming-search)
  - [Streaming Analyze](#streaming-analyze)
  - [Streaming Hash](#streaming-hash)
  - [Large-File Guidance](#large-file-guidance)
- [Progress Bars](#progress-bars)
- [Checksum Verification Workflow](#checksum-verification-workflow)
- [Input and Output Formats](#input-and-output-formats)
  - [Output Formats](#output-formats)
  - [Offset Display](#offset-display)
  - [Input Formats](#input-formats)
  - [Chunk Sizes and Line Width](#chunk-sizes-and-line-width)
- [Range Specifications](#range-specifications)
- [Tips, Tricks & Cookbook](#tips-tricks--cookbook)
  - [Firmware Analysis](#firmware-analysis)
  - [Malware & Reverse Engineering](#malware--reverse-engineering)
  - [Disk & File Recovery](#disk--file-recovery)
  - [Batch Checksumming](#batch-checksumming)
  - [Binary Patch Pipelines](#binary-patch-pipelines)
  - [Integration with Other Tools](#integration-with-other-tools)
- [Troubleshooting](#troubleshooting)
- [Quick Reference Card](#quick-reference-card)

---

## Getting Started

### Basic Invocation

```bash
binfiddle [GLOBAL_OPTIONS] <COMMAND> [COMMAND_OPTIONS]
```

`-i` / `--input` selects the input file (`-` means stdin). Most commands can also read from stdin by default if no `-i` is supplied.

### Getting Help

```bash
binfiddle --help                 # Global help and options
binfiddle --version              # Show version
binfiddle read --help
binfiddle write --help
binfiddle edit --help
binfiddle search --help
binfiddle hash --help
binfiddle analyze --help
binfiddle diff --help
binfiddle convert --help
binfiddle patch --help
binfiddle chain --help
binfiddle struct --help
```

### First Steps

```bash
# Create a test file
echo -n "Hello, World!" > test.txt

# Read it as hex
binfiddle -i test.txt read ..
# Output: 48 65 6c 6c 6f 2c 20 57 6f 72 6c 64 21

# Read as ASCII
binfiddle -i test.txt read .. --format ascii
# Output: H e l l o ,   W o r l d !

# Search for the word "World"
binfiddle -i test.txt search "World" --input-format ascii --all
```

---

## Global Options

These options apply to most commands. Some commands also accept their own options (see each command below).

### Input/Output

| Option | Short | Description | Default |
|--------|-------|-------------|---------|
| `--input <FILE>` | `-i` | Input file (`-` for stdin) | stdin |
| `--output <FILE>` | `-o` | Output file (`-` for stdout) | — |
| `--in-file` | — | Modify the input file in-place | false |
| `--silent` | — | Suppress diagnostic/diff output | false |
| `--progress` | — | Show progress bars for long-running operations | false |
| `--input-format <FMT>` | — | Format of value arguments (`hex`, `dec`, `oct`, `bin`, `ascii`) | `hex` |

### Display

| Option | Short | Description | Default |
|--------|-------|-------------|---------|
| `--format <FMT>` | `-f` | Output format: `hex`, `dec`, `oct`, `bin`, `ascii`, `raw` | `hex` |
| `--chunk-size <BITS>` | `-c` | Bits per display chunk (1–64) | 8 |
| `--width <N>` | — | Chunks per output line (0 = no wrap) | 16 |
| `--show-offset` | — | Show hex address prefix on each line | false |
| `--show-ascii` | — | Show ASCII sidebar (implies `--show-offset`) | false |

### Process Memory Source

| Option | Description |
|--------|-------------|
| `--process-self` | Read from `/proc/self/mem` |
| `--pid <PID>` | Read from `/proc/<PID>/mem` |
| `--list-regions` | List mapped regions from `/proc/<PID>/maps` and exit |
| `--address <ADDR>` | Base address (hex or decimal) |
| `--size <N>` | Number of bytes to read/write (hex or decimal) |
| `--allow-write` | Opt-in required for any process-memory write |
| `--force-writable` | Temporarily make read-only pages writable (requires `--allow-write`) |
| `--zero-fill-inaccessible` | Replace inaccessible pages with zeros instead of failing |
| `--skip-inaccessible` | Omit inaccessible pages instead of failing (read only) |

> `--process-self` and `--pid` are mutually exclusive and cannot be used with `--input` or `chain`.

---

## Command Reference

### read

Extract and display bytes from binary data.

**Syntax**

```bash
binfiddle -i <FILE> read <RANGE> [OPTIONS]
```

**Options**

| Option | Description | Default |
|--------|-------------|---------|
| `--format <FMT>` / `-f` | Output format: `hex`, `dec`, `oct`, `bin`, `ascii`, `raw` | `hex` |
| `--chunk-size <BITS>` / `-c` | Bits per display chunk | 8 |
| `--width <N>` | Chunks per line | 16 |
| `--show-offset` | Prefix each line with its hex offset | false |
| `--show-ascii` | Add ASCII sidebar (implies `--show-offset`) | false |

**Examples**

```bash
# Read first 16 bytes
binfiddle -i file.bin read 0..16

# Read bytes 256-511 using hex offsets
binfiddle -i file.bin read 0x100..0x200

# Read from byte 100 to end
binfiddle -i file.bin read 100..

# Read first 50 bytes
binfiddle -i file.bin read ..50

# Entire file
binfiddle -i file.bin read ..

# Single byte at offset 42
binfiddle -i file.bin read 42

# Different output formats
binfiddle -i file.bin read 0..8 --format dec
binfiddle -i file.bin read 0..8 --format bin
binfiddle -i file.bin read 0..8 --format ascii

# xxd-style with offsets and ASCII
binfiddle -i /bin/ls read 0..64 --show-ascii

# Raw binary output for piping
binfiddle -i file.bin read 0..4 --format raw | file -

# Custom line width and nibble chunks
binfiddle -i file.bin read 0..4 --chunk-size 4 --width 8
```

---

### write

Overwrite bytes at a specified position. The file size does **not** change.

**Syntax**

```bash
binfiddle -i <FILE> write <POSITION> <VALUE> [OPTIONS]
```

**Options**

| Option | Description | Default |
|--------|-------------|---------|
| `--input-format <FMT>` | Format of `VALUE` | `hex` |
| `--silent` | Suppress previous/new diff output | false |
| `--in-file` | Modify the input file in-place | false |
| `-o <FILE>` | Output file | — |

**Examples**

```bash
# Write hex bytes at offset 0x100, save to new file
binfiddle -i file.bin write 0x100 DEADBEEF -o modified.bin

# Modify in place
binfiddle -i file.bin --in-file write 0 7F454C46

# Write decimal values
binfiddle -i file.bin write 0 "127 69 76 70" --input-format dec -o out.bin

# Write an ASCII string
binfiddle -i file.bin write 0x200 "HELLO" --input-format ascii -o out.bin

# Write binary values
binfiddle -i file.bin write 0 "11111111 00000000" --input-format bin -o out.bin

# Suppress diff output
binfiddle -i file.bin write 0 FF --silent -o out.bin
```

**Write diff output**

By default `write` shows the old and new values:

```
Previous: deadbeef
New:      cafebabe
```

Use `--silent` to hide this.

---

### edit

Perform structural modifications that **can** change the file size.

**Operations**

| Operation | Effect |
|-----------|--------|
| `insert` | Add bytes at a position (data shifts right, file grows) |
| `remove` | Delete a byte range (data shifts left, file shrinks) |
| `replace` | Remove a range and insert new data |

**Syntax**

```bash
binfiddle -i <FILE> edit <OPERATION> <RANGE> [DATA] [OPTIONS]
```

**Options**

| Option | Description | Default |
|--------|-------------|---------|
| `--input-format <FMT>` | Format of inserted/replaced data | `hex` |
| `--silent` | Suppress diagnostics | false |
| `--in-file` | Modify the input file in-place | false |
| `-o <FILE>` | Output file | — |

**Examples**

```bash
# Insert 4 bytes at 0x100
binfiddle -i file.bin edit insert 0x100 DEADBEEF -o modified.bin

# Prepend a header
binfiddle -i file.bin edit insert 0 HEADER --input-format ascii -o modified.bin

# Append a footer
binfiddle -i file.bin edit insert $(stat -c%s file.bin) FOOTER --input-format ascii -o modified.bin

# Remove 256 bytes
binfiddle -i file.bin edit remove 0x500..0x600 -o modified.bin

# Remove first 16 bytes
binfiddle -i file.bin edit remove 0..16 -o modified.bin

# Replace first 4 bytes with ELF magic
binfiddle -i file.bin edit replace 0..4 7F454C46 -o modified.bin

# Replace with shorter data (file shrinks)
binfiddle -i file.bin edit replace 0..8 CAFE -o modified.bin

# Replace with longer data (file grows)
binfiddle -i file.bin edit replace 0..2 DEADBEEFCAFE -o modified.bin
```

---

### search

Find patterns in binary data using exact, regex, or wildcard matching.

**Syntax**

```bash
binfiddle -i <FILE> search <PATTERN> [OPTIONS]
```

**Pattern formats (`--input-format`)**

| Format | Description | Example | Use when |
|--------|-------------|---------|----------|
| `hex` (default) | Hex byte sequence | `DEADBEEF` | You know exact bytes |
| `ascii` | Literal ASCII string | `PASSWORD` | Searching readable text |
| `dec` | Decimal byte values | `222 173 190 239` | Decimal values |
| `oct` | Octal byte values | `336 255 276 357` | Octal notation |
| `bin` | Binary strings | `11011110 10101101` | Bit-level patterns |
| `regex` | Byte-level regular expression | `[A-Z]{4,}` | Value ranges / patterns |
| `hex-regex` / `hexregex` | Regex over hex-like tokens | — | Regex on nibbles/hex chars |
| `mask` | Hex bytes with wildcards (`??` or `XX`) | `DE ?? BE EF` | Unknown bytes |

**Options**

| Option | Description | Default |
|--------|-------------|---------|
| `--input-format <FMT>` | Pattern format | `hex` |
| `--all` | Find all matches (default: first only) | false |
| `--count` | Output only the match count | false |
| `--offsets-only` | Output only match offsets | false |
| `--context <N>` | Show N bytes before/after each match | 0 |
| `--no-overlap` | Prevent overlapping matches | false |
| `--color <MODE>` | Color output: `always`, `auto`, `never` | `auto` |
| `--block-size <SIZE>` | Stream input in blocks (e.g. `64M`, `1G`) | — |
| `--format <FMT>` / `-f` | Format used to display matched bytes | `hex` |

**⚠️ Critical: regex operates on raw bytes, not hex strings**

```bash
# ❌ WRONG: "ff" means two ASCII 'f' bytes (0x66 0x66)
binfiddle -i file.bin search "ff" --input-format regex

# ✅ CORRECT: search for byte 0xFF
binfiddle -i file.bin search "FF" --input-format hex --all
```

**Examples**

```bash
# Exact hex
binfiddle -i file.bin search "DEADBEEF" --all

# ELF magic
binfiddle -i file.bin search "7F454C46" --all --count

# ASCII string
binfiddle -i file.bin search "ERROR" --input-format ascii --all

# Mask with wildcard
binfiddle -i file.bin search "7F 45 4C 46 ?? ??" --input-format mask --all

# Regex: 4+ uppercase ASCII letters
binfiddle -i file.bin search "[A-Z]{4,}" --input-format regex --all

# Regex: printable strings of 10+ chars
binfiddle -i file.bin search "[ -~]{10,}" --input-format regex --all

# Context around matches
binfiddle -i file.bin search "7F454C46" --context 16 --all

# Offsets only (script-friendly)
binfiddle -i file.bin search "CAFE" --all --offsets-only

# Non-overlapping: "AAAA" in AAAAAA = 2 matches, not 5
binfiddle -i file.bin search "AAAA" --all --no-overlap
```

**Format selection guide**

| You know | Use format | Example |
|----------|------------|---------|
| Exact hex bytes | `hex` | `7F454C46` for ELF magic |
| Exact text string | `ascii` | `ERROR` for error messages |
| Hex pattern with gaps | `mask` | `7F ?? 4C 46` |
| Byte value ranges / patterns | `regex` | `[\x00-\xFF]{100,}` |
| ASCII character classes | `regex` | `[A-Z][a-z]{3,}` |
| Repeating patterns | `regex` | `[ -~]{10,}\x00` |

**Decision tree**

```
Need exact bytes you know in hex?            → Use `hex`
Need an exact text string?                    → Use `ascii`
Know some bytes but not others?               → Use `mask` with ??
Need byte value ranges or complex patterns?   → Use `regex` with \xHH escapes
Need ASCII character classes?                 → Use `regex`
Need repeating/quantified patterns?           → Use `regex`
```

**Advanced regex constructs**

The `regex` format uses Rust's `regex::bytes` engine on raw byte sequences.

| Construct | Meaning |
|-----------|---------|
| `.` | Any single byte |
| `*` | Zero or more repetitions |
| `+` | One or more repetitions |
| `{n,m}` | Between n and m repetitions |
| `^` | Start of data |
| `$` | End of data |
| `\|` | Alternation (OR) |
| `()` | Grouping |
| `(?i)` | Case-insensitive (ASCII letters only) |

**Byte escapes**

| Escape | Byte |
|--------|------|
| `\x00` | NULL |
| `\xFF` | 255 |
| `\x20` | Space |
| `\xHH` | Any hex byte value |

**Common regex patterns**

```bash
# Version strings
binfiddle -i app.bin search "[0-9]+\.[0-9]+\.[0-9]+" --input-format regex --all

# URLs
binfiddle -i memory.dump search "https?://[^\x00]{5,}" --input-format regex --all

# UUIDs
binfiddle -i file.bin search "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}" --input-format regex --all

# Null-terminated strings of 10+ printable chars
binfiddle -i data.bin search "[\x20-\x7E]{10,}\x00" --input-format regex --all
```

**Output formats**

```
# Default
0x00000100: de ad be ef

# Offsets only
0x00000100
0x00000250

# Count only
2

# With context (--context 4)
0x00000100: 11 22 33 44 [de ad be ef] 55 66 77 88
```

---

### hash

Compute cryptographic and non-cryptographic digests.

**Supported algorithms**

| Algorithm | Aliases | Digest size | Use case |
|-----------|---------|-------------|----------|
| `md5` | — | 128-bit | Legacy checksums |
| `sha1` | `sha-1` | 160-bit | Legacy verification |
| `sha256` | `sha-256` | 256-bit | Cryptographic verification |
| `blake3` | `blake3-256` | 256-bit | Fast modern hash |
| `crc32` | `crc-32` | 32-bit | Data integrity / ZIP-style |
| `xxhash64` | `xxh64` | 64-bit | Fast non-cryptographic checksum |

**Syntax**

```bash
binfiddle -i <FILE> hash <ALGORITHM> [OPTIONS]
```

**Options**

| Option | Description | Default |
|--------|-------------|---------|
| `--output-format <FMT>` | `hex` or `base64` (standard, with padding) | `hex` |
| `--block-size <N>` | Hash non-overlapping blocks of N bytes (`0` = whole file) | 0 |
| `--stream` | Read input incrementally instead of memory-mapping | false |
| `--read-block-size <N>` | Chunk size when streaming (supports `K`/`M`/`G`) | `1M` |
| `--check <FILE>` | Verify checksums from a checksum file | — |

**Examples**

```bash
# SHA-256 of an entire file
binfiddle -i firmware.bin hash sha256

# MD5
binfiddle -i file.bin hash md5

# BLAKE3
binfiddle -i data.bin hash blake3

# xxhash64 (very fast)
binfiddle -i large.bin hash xxhash64

# SHA-1 in base64
binfiddle -i file.bin hash sha1 --output-format base64

# Per-block CRC32 (find corrupted regions)
binfiddle -i disk.img hash crc32 --block-size 4096

# Stream-hash a huge file
binfiddle -i huge.bin hash sha256 --stream --read-block-size 64M

# Stream-hash with per-block output
binfiddle -i huge.bin hash crc32 --stream --read-block-size 64M --block-size 4096
```

**Block-based output**

```
0x00000000: a3b2c1d4
0x00001000: e5f6a7b8
...
```

---

### analyze

Statistical analysis: entropy, histograms, and Index of Coincidence.

**Syntax**

```bash
binfiddle -i <FILE> analyze <TYPE> [OPTIONS]
```

**Analysis types**

| Type | Aliases | Description |
|------|---------|-------------|
| `entropy` | — | Shannon entropy (0–8 bits/byte) |
| `histogram` | `hist` | Byte frequency distribution |
| `ic` | `ioc`, `index-of-coincidence` | Index of Coincidence |

**Options**

| Option | Description | Default |
|--------|-------------|---------|
| `--block-size <N>` | Block size (`0` = entire file, supports `K`/`M`/`G`) | `256` |
| `--output-format <FMT>` | `human`, `csv`, `json` | `human` |
| `--range <RANGE>` | Restrict analysis to a range | — |

**Entropy interpretation**

| Entropy Range | Typical Content |
|---------------|-----------------|
| 0.0 – 1.0 | Highly repetitive (nulls, single value) |
| 1.0 – 4.0 | Text, code, structured data |
| 4.0 – 6.0 | Mixed content |
| 6.0 – 7.5 | Compressed data |
| 7.5 – 8.0 | Encrypted or highly random data |

**Examples**

```bash
# Entire-file entropy
binfiddle -i file.bin analyze entropy --block-size 0

# Block entropy to find encrypted sections
binfiddle -i firmware.bin analyze entropy --block-size 4096

# CSV for graphing
binfiddle -i file.bin analyze entropy --block-size 1024 --output-format csv > entropy.csv

# JSON
binfiddle -i file.bin analyze entropy --output-format json

# Histogram for a range
binfiddle -i file.bin analyze histogram --range 0x100..0x200

# Index of Coincidence
binfiddle -i file.bin analyze ic --block-size 0
binfiddle -i file.bin analyze ic --block-size 512
```

**Sample entropy output**

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

**Sample histogram output**

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

**Sample IC output**

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

### diff

Compare two binary files byte-by-byte.

**Syntax**

```bash
binfiddle diff <FILE1> <FILE2> [OPTIONS]
```

**Options**

| Option | Description | Default |
|--------|-------------|---------|
| `--diff-format <FMT>` | `simple`, `unified`, `side-by-side` / `sidebyside`, `patch`, `summary`, `auto` | `auto` |
| `--context <N>` | Context bytes around differences | 3 |
| `--color <MODE>` | `always`, `auto`, `never` | `auto` |
| `--ignore-offsets <RANGES>` | Comma-separated ranges to ignore | — |
| `--diff-width <N>` | Bytes per line in output | 16 |
| `--summary` | Print summary of differences | false |

**Output formats**

| Format | Description |
|--------|-------------|
| `simple` | One line per difference: `Offset: 0xXX != 0xYY` |
| `unified` | Unified diff with context lines |
| `side-by-side` | Two-column comparison |
| `patch` | Machine-readable patch for `binfiddle patch` |
| `summary` | Count of changed/added/deleted bytes |
| `auto` | Pick a format automatically based on diff size |

**Examples**

```bash
# Simple format
binfiddle diff original.bin modified.bin

# Unified with extra context
binfiddle diff original.bin modified.bin --diff-format unified --context 5

# Side-by-side
binfiddle diff v1.bin v2.bin --diff-format side-by-side

# Generate patch
binfiddle diff original.bin modified.bin --diff-format patch > changes.patch

# Ignore timestamp bytes
binfiddle diff v1.bin v2.bin --ignore-offsets "0x10..0x18"

# Ignore multiple ranges
binfiddle diff v1.bin v2.bin --ignore-offsets "0x0..0x10,0x100..0x110,0x200..0x210"

# Summary + color
binfiddle diff file1.bin file2.bin --summary --color always | less -R
```

**Sample output**

*Simple format:*
```
0x00000001: 0xad != 0xca
0x00000010: 0xde != 0xff
0x00000020: 0xbe != EOF
```

*Unified format:*
```
--- file1.bin
+++ file2.bin
@@ -0x0,0x10 +0x0,0x10 @@
 0x00000000: de ad be ef ca fe ba be 00 11 22 33 44 55 66 77  |................|
-0x00000010: 88 99 aa bb cc dd ee ff 00 11 22 33 44 55 66 77  |................|
+0x00000010: 88 99 aa bb cc dd ee 00 00 11 22 33 44 55 66 77  |................|
 0x00000020: 00 11 22 33 44 55 66 77 88 99 aa bb cc dd ee ff  |................|
```

*Side-by-side format:*
```
file1.bin                                    | file2.bin
---------------------------------------------+---------------------------------------------
0x00000000: de ad be ef ca fe ba be          | 0x00000000: de ad be ef ca fe ba be
0x00000008: 00 11 22 33 44 55 66 77          ! 0x00000008: 00 11 22 33 44 55 66 00
```

*Patch format:*
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

*Summary output:*
```
42 difference(s): 38 changed, 2 deleted, 2 added (file1: 1024 bytes, file2: 1026 bytes)
```

---

### convert

Convert text encodings and normalize line endings.

**Syntax**

```bash
binfiddle -i <FILE> convert [OPTIONS]
```

**Options**

| Option | Values | Default | Description |
|--------|--------|---------|-------------|
| `--from` | `utf-8`, `utf-16le`, `utf-16be`, `latin-1`, `windows-1252` | `utf-8` | Source encoding |
| `--to` | `utf-8`, `utf-16le`, `utf-16be`, `latin-1`, `windows-1252` | `utf-8` | Target encoding |
| `--newlines` | `unix`, `windows`, `mac`, `keep` | `keep` | Line ending conversion |
| `--bom` | `add`, `remove`, `keep` | `keep` | BOM handling |
| `--on-error` | `strict`, `replace`, `ignore` | `replace` | Error handling |

**Examples**

```bash
# UTF-8 → UTF-16LE
binfiddle -i doc.txt convert --to utf-16le -o doc_utf16.txt

# UTF-16LE → UTF-8
binfiddle -i windows_file.txt convert --from utf-16le --to utf-8 -o unix_file.txt

# CRLF → LF
binfiddle -i script.bat convert --newlines unix -o script.sh

# Add/remove BOM
binfiddle -i doc.txt convert --bom add -o doc_bom.txt
binfiddle -i file_with_bom.txt convert --bom remove -o file_no_bom.txt

# Full conversion
binfiddle -i windows_doc.txt convert \
    --from utf-16le --to utf-8 --newlines unix --bom remove \
    -o unix_doc.txt

# Round-trip test
echo -n "Hello 世界" | binfiddle convert --to utf-16le | \
    binfiddle convert --from utf-16le --to utf-8
```

**Practical use cases**

```bash
# Batch-convert legacy Windows-1252 files
for f in *.txt; do
    binfiddle -i "$f" convert --from windows-1252 --to utf-8 -o "utf8_$f"
done

# Normalize all text files for Unix
binfiddle -i config.ini convert --newlines unix --bom remove -o config_unix.ini

# Produce UTF-16LE with BOM for a Windows API
binfiddle -i data.txt convert --to utf-16le --bom add -o data_win.txt
```

---

### patch

Apply binary patches. Patches are normally generated with `binfiddle diff --diff-format patch`, but can be written by hand.

**Syntax**

```bash
binfiddle [GLOBAL_OPTIONS] patch <TARGET> <PATCH_FILE> [OPTIONS]
```

**Options**

| Option | Description |
|--------|-------------|
| `--backup <SUFFIX>` | Create a backup before patching (e.g. `.bak`) |
| `--dry-run` | Show what would be done without applying |
| `--revert` | Apply patch in reverse (undo) |

**Examples**

```bash
# Apply a patch to create a new file
binfiddle --output patched.bin patch original.bin changes.patch

# Dry run
binfiddle patch original.bin changes.patch --dry-run

# In-place with backup
binfiddle --in-file -i target.bin patch target.bin changes.patch --backup .bak

# Revert a patch
binfiddle --output reverted.bin patch patched.bin changes.patch --revert
```

**Patch file format**

```
# binfiddle patch file
# source: original.bin
# target: modified.bin
# format: OFFSET:OLD_HEX:NEW_HEX
# differences: 3
#
0x00000000:de:ff
0x00000100:deadbeef:cafebabe
0x00000200::abcd          # insert bytes (empty old)
0x00000300:1234:          # delete bytes (empty new)
```

**Full diff-patch workflow**

```bash
binfiddle diff original.bin modified.bin --diff-format patch > changes.patch
binfiddle --output reconstructed.bin patch original.bin changes.patch
diff modified.bin reconstructed.bin && echo "Perfect match!"
```

**Validation and dry run**

The patch command validates offsets and expected old bytes before applying. If any entry fails, **no changes are written**.

```bash
# Preview changes
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

If a target does not match the expected old bytes:

```bash
binfiddle patch wrong_file.bin changes.patch
# ✗ 0x00000000: de -> ff
#    Mismatch at 0x00000000: expected de, found 00
# Summary: 0 succeeded, 1 failed
# Some patches failed - no changes written
```

---

### chain

Run several binfiddle commands in sequence, passing the byte output of each step as the input to the next. This avoids shell quoting issues and makes multi-step transformations explicit.

**Syntax**

```bash
binfiddle [OPTIONS] chain --step <COMMAND> [--step <COMMAND>] ...
```

**Options**

| Option | Description |
|--------|-------------|
| `--step <COMMAND>` | One step (repeatable, required) |

**How it works**

- Each step is parsed with shell quoting rules and executed as a subprocess.
- Intermediate steps must produce byte output (`write`, `edit`, `convert`, etc.).
- The final step may produce text output (`read`, `search`, `analyze`).
- Use `--silent` to prevent intermediate diagnostics from polluting stderr.
- `chain` cannot be combined with `--process-self` or `--pid`.

**Examples**

```bash
# Replace a header and patch a byte
binfiddle -i firmware.bin -o patched.bin chain \
    --step "edit replace 0..4 44415431" \
    --step "write 8 00"

# Modify then read result
binfiddle -i data.bin chain \
    --step "edit replace 0..8 1234567890abcdef" \
    --step "read 0..16"

# From stdin
printf '\x00\x11\x22\x33' | binfiddle --input - chain \
    --step "edit replace 0..2 4142" \
    --step "read 0..4"

# Silent multi-step pipeline
binfiddle --silent -i data.bin -o out.bin chain \
    --step "edit replace 0..2 9999" \
    --step "write 0 42"
```

---

### struct

Parse binary data with YAML structure templates. Useful for headers, protocols, and firmware formats.

**Syntax**

```bash
binfiddle struct <TEMPLATE> [OPTIONS] [< input.bin]
binfiddle -i input.bin struct <TEMPLATE> [OPTIONS]
```

**Options**

| Option | Description |
|--------|-------------|
| `--list-fields` | List template fields without parsing |
| `--get <FIELD>` | Get one or more field values |
| `--output-format <FMT>` | `human`, `json`, `yaml` |

**Examples**

```bash
# Parse an ELF header
binfiddle struct elf_header.yaml < /bin/ls

# List fields
binfiddle struct my_format.yaml --list-fields

# Get specific values
binfiddle struct header.yaml --get version < firmware.bin
binfiddle struct elf_header.yaml --get e_type --get e_machine < /bin/ls

# JSON output
binfiddle struct format.yaml --output-format json < data.bin
```

**Simple template**

```yaml
name: MyHeader
description: Binary header structure
endian: little
fields:
  - name: magic
    offset: 0x00
    size: 4
    type: hex_string
    assert: "7f454c46"
    description: "Magic number"
  - name: version
    offset: 0x04
    size: 2
    type: u16
    enum:
      "1": "v1.0"
      "2": "v2.0"
```

**Supported field types**

| Type | Size | Description |
|------|------|-------------|
| `u8`, `u16`, `u32`, `u64` | 1/2/4/8 bytes | Unsigned integers |
| `i8`, `i16`, `i32`, `i64` | 1/2/4/8 bytes | Signed integers |
| `hex_string` | Variable | Raw bytes as hex |
| `string` | Variable | ASCII/UTF-8 string |
| `bytes` | Variable | Raw byte array |
| `computed` | — | Virtual field from expression |

Templates also support field references (`$fieldname`), magic variables (`$@prev_end`, `$@file_size`), conditional fields (`when:`), bitfields, counted arrays, and bit-level fields (`bit_offset`/`bit_size`).

---

### Process Memory

> **Experimental — Linux only**

Inspect and patch memory via `/proc/<pid>/mem`. Target the current process with `--process-self` or another same-user process with `--pid <PID>`.

**Listing regions**

```bash
# Current process
binfiddle --process-self --list-regions

# Another process
binfiddle --pid 1234 --list-regions
```

**Reading memory**

```bash
binfiddle --process-self --address 0x7ffd12345678 --size 16 read 0..16
binfiddle --pid 1234 --address 0x7f8a1b2c3000 --size 16 read 0..16
```

**Searching memory**

```bash
binfiddle --process-self --address 0x400000 --size 0x1000 search 474CCC --all
binfiddle --process-self --address 0x400000 --size 0x1000 \
    search "PASSWORD" --input-format ascii --all
```

**Writing memory**

```bash
# Writable region in current process
binfiddle --process-self --address 0x7ffd12345678 --size 4 \
    --allow-write write 0 DEADBEEF

# Writable region in another process
binfiddle --pid 1234 --address 0x7f8a1b2c3000 --size 4 \
    --allow-write write 0 CAFEBABE

# Force-write a read-only region (dangerous)
binfiddle --process-self --address 0x7ffd12345678 --size 4 \
    --allow-write --force-writable write 0 DEADBEEF
```

**Fill modes for inaccessible pages**

| Mode | Flag | Behavior | Supported commands |
|------|------|----------|--------------------|
| Error | (default) | Fail if any requested byte is inaccessible | all |
| Zero-fill | `--zero-fill-inaccessible` | Replace inaccessible bytes with zeros | `read`, `search` |
| Skip | `--skip-inaccessible` | Omit inaccessible bytes (result may be shorter) | `read` |

```bash
# Read a range that may span an unmapped page, filling gaps with zeros
binfiddle --process-self --address 0x7f8a1b2c3000 --size 0x2000 \
    --zero-fill-inaccessible read 0..0x2000
```

**Safety notes**

- `--allow-write` is required for any process-memory write.
- `--force-writable` requires `--allow-write`. It uses `mprotect` for `--process-self` and ptrace syscall injection for `--pid` (Linux x86_64 and aarch64). Protection is restored afterward, but crashes or interruptions may leave pages writable.
- Cross-process access requires ptrace permissions. Check `/proc/sys/kernel/yama/ptrace_scope`:
  - `0` — unrestricted
  - `1` — restricted to parent/child (default on many distros)
  - `2` — admin-only
  - `3` — no ptrace
- `insert` and `remove` are rejected because they would change region size.
- `--skip-inaccessible` cannot be used with `search` because offsets would no longer match the original address space.
- Modifying running process memory can crash the target. Always target your own processes.

---

## Memory-Mapped Input, Streaming & Large Files

### Memory-Mapped File Input

File input is memory-mapped using `memmap2`. Read-only commands (`read`, `search`, `analyze`, `diff`, `hash`) can work with files much larger than RAM because the OS pages data from disk on demand.

```bash
# Search a multi-gigabyte firmware image
binfiddle -i firmware.bin search "7F454C46" --all

# Analyze a huge dump with minimal resident memory
binfiddle -i memory.dump analyze entropy --block-size 4096
```

When a command mutates data (`write`, `edit`), the mapped region is lazily copied into an owned in-memory buffer first, unless you use `--in-file` with `write` (see below). Size-changing edits (`insert`, `remove`, `replace`) therefore require enough memory to hold the modified data.

### In-Place Modification with `MmapMut`

Use `--in-file` to modify the input file directly.

```bash
# CAUTION: modifies the original file with no automatic backup
binfiddle -i config.dat --in-file write 0x10 FF
```

**How it works**

- `write` + `--in-file` opens the file read-write and maps it with `MmapMut`. Changes are flushed directly to disk without copying the whole file.
- `edit` + `--in-file` still loads the file into memory, applies the structural change, and writes it back.
- `--in-file` cannot be combined with `-o` / `--output`.

```bash
# Efficiently patch many bytes in a large file
binfiddle -i game.exe --in-file write 0x123456 90909090
binfiddle -i game.exe --in-file write 0x12345A EB10
```

### Streaming Search

For files larger than RAM, or when you want bounded memory usage, use `--block-size` to stream `search` in fixed-size blocks.

```bash
# Search a 100 GB file without loading it whole
binfiddle -i huge.bin search "7F454C46" --all --block-size 64M

# Stop at first match while streaming
binfiddle -i huge.bin search "CAFEBABE" --block-size 256M
```

**Limitations**

- Only `hex`, `ascii`, `dec`, `oct`, `bin`, and `mask` patterns are supported. Regex needs an unbounded lookback window and requires the memory-mapped path.
- `--context` is disabled because surrounding bytes are not kept in memory.

### Streaming Analyze

When `--block-size` is provided, `analyze` reads the input block-by-block instead of memory-mapping. This makes block-based entropy and IC analysis practical on huge files. Histogram mode still returns a single global result.

```bash
# Entropy of a 100 GB disk image
binfiddle -i disk.img analyze entropy --block-size 64M

# Per-block IC as CSV
binfiddle -i memory.dump analyze ic --block-size 16M --output-format csv
```

**Limitations**

- `--range` is not supported.
- Process-memory sources (`--process-self`, `--pid`) use the normal in-memory path.

### Streaming Hash

Use `--stream` to hash files incrementally.

```bash
binfiddle -i huge.bin hash sha256 --stream --read-block-size 64M
```

With `--block-size N` you get per-block digests:

```bash
binfiddle -i disk.img hash crc32 --stream --read-block-size 64M --block-size 4096
```

**Limitations**

- `--stream` cannot be used with `--process-self` or `--pid`.

### Large-File Guidance

| Goal | Recommended approach |
|------|----------------------|
| Inspect / search / analyze / hash huge files | Memory-mapped input (default) or `--stream` / `--block-size` |
| Patch a huge file in-place | `write --in-file` (uses `MmapMut`) |
| Resize a huge file | `edit insert/remove/replace` requires loading the file into memory |
| Convert a huge file | `convert` reads the whole input; avoid on files larger than RAM |
| Compare huge files | `diff` loads both files; ensure enough RAM |

---

## Progress Bars

Long-running commands can display an `indicatif` progress bar on stderr when you pass `--progress`. The bar shows bytes processed, throughput, and ETA.

```bash
binfiddle -i huge.bin hash sha256 --stream --read-block-size 64M --progress
binfiddle -i huge.bin search DEADBEEF --all --block-size 64M --progress
binfiddle -i huge.bin analyze entropy --block-size 64M --progress
binfiddle -i huge.bin convert --to utf-16le --progress -o out.bin
```

Progress bars are:

- **Opt-in** — not shown unless `--progress` is supplied.
- Suppressed when stderr is not a TTY (scripts, pipes).
- Suppressed by `--silent`.
- Suppressed inside `chain`.

---

## Checksum Verification Workflow

`hash --check` verifies files against a checksum file in GNU coreutils format (`DIGEST  FILENAME`).

**Create a checksum file**

```bash
binfiddle -i file1.bin hash sha256 > SHA256SUMS
binfiddle -i file2.bin hash sha256 >> SHA256SUMS
```

The resulting file looks like:

```
2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824  file1.bin
b393b84f6f3acdaa5a9a517c7bb6a5f2c2f1b1f6f8b8f3a9f3a7f0b6b5b6b6b6  file2.bin
```

**Verify**

```bash
binfiddle hash sha256 --check SHA256SUMS
```

Output:

```
file1.bin: OK
file2.bin: FAILED
1 passed, 1 failed
```

The command exits with a non-zero status if any file fails. Empty lines and lines starting with `#` or `;` are ignored.

**Important:** `--check` requires `--block-size 0` (the default) and does not take an input file; it reads filenames from the checksum file.

---

## Input and Output Formats

### Output Formats

Used by `--format` / `-f`:

| Format | Example Output |
|--------|----------------|
| `hex` | `de ad be ef` |
| `dec` | `222 173 190 239` |
| `oct` | `336 255 276 357` |
| `bin` | `11011110 10101101 10111110 11101111` |
| `ascii` | `....` (non-printable shown as `.`) |
| `raw` | Raw bytes, no formatting (output/pipe only) |

### Offset Display

```bash
# Address prefixes only
binfiddle -i /bin/ls read 0..64 --show-offset

# xxd-style with ASCII sidebar
binfiddle -i /bin/ls read 0..64 --show-ascii

# Combine with other formats
binfiddle -i file.bin read 0..32 --format dec --show-offset
```

### Input Formats

Used by `--input-format` for `write`, `edit`, and `search` patterns:

| Format | Example |
|--------|---------|
| `hex` | `DEADBEEF` or `DE AD BE EF` |
| `dec` | `222 173 190 239` |
| `oct` | `336 255 276 357` |
| `bin` | `11011110 10101101` |
| `ascii` | `Hello` |

### Chunk Sizes and Line Width

```bash
# 4-bit (nibble) chunks
binfiddle -i file.bin read 0..2 --chunk-size 4

# 16-bit words
binfiddle -i file.bin read 0..4 --chunk-size 16

# 8 chunks per line
binfiddle -i file.bin read 0..32 --width 8

# No line wrapping
binfiddle -i file.bin read 0..32 --width 0
```

---

## Range Specifications

| Syntax | Meaning | Example |
|--------|---------|---------|
| `N` | Single byte at index N | `42` |
| `N..M` | Bytes N to M-1 | `10..20` |
| `N..` | Byte N to end | `100..` |
| `..M` | Bytes 0 to M-1 | `..50` |
| `..` | Entire file | `..` |
| `0xN` | Hex index | `0x100` |
| `0xN..0xM` | Hex range | `0x100..0x200` |

---

## Tips, Tricks & Cookbook

### Firmware Analysis

```bash
# Identify file type by magic
binfiddle -i firmware.bin read 0..4
# 7f 45 4c 46 = ELF
# 4d 5a        = PE/MZ
# 89 50 4e 47  = PNG

# Find version strings
binfiddle -i firmware.bin search "v[0-9]" --input-format regex --all --context 16

# Extract a section
binfiddle -i firmware.bin read 0x1000..0x2000 -o section.bin

# Find encrypted/compressed sections by entropy
binfiddle -i firmware.bin analyze entropy --block-size 4096

# Patch a version string in place
binfiddle -i firmware.bin edit replace 0x200..0x210 "v2.0.0" \
    --input-format ascii -o patched.bin
```

### Malware & Reverse Engineering

```bash
# High-entropy sections (packed/encrypted)
binfiddle -i suspicious.exe analyze entropy --block-size 4096 --output-format csv > entropy.csv

# Byte distribution anomalies
binfiddle -i malware.bin analyze histogram --output-format json

# Search for common shellcode prologues
binfiddle -i dump.bin search "31 c0 50 68" --all --context 16

# Extract all printable strings
binfiddle -i malware.bin search "[ -~]{6,}" --input-format regex --all --offsets-only

# Compare two samples
binfiddle diff sample_a.bin sample_b.bin --diff-format side-by-side --diff-width 8
```

### Disk & File Recovery

```bash
# Find JPEG headers
binfiddle -i disk.img search "FF D8 FF" --all --offsets-only

# Find PNG signatures
binfiddle -i disk.img search "89504E47" --all --offsets-only

# Extract from a found offset
binfiddle -i disk.img read 0x15000..0x20000 -o recovered.jpg

# Search-and-extract in one shot
OFFSET=$(binfiddle -i data.bin search "PNG" --input-format ascii --offsets-only | head -1)
binfiddle -i data.bin read ${OFFSET}.. -o image.png
```

### Batch Checksumming

```bash
# Create SHA-256 sums for a directory
for f in *.bin; do
    binfiddle -i "$f" hash sha256 >> SHA256SUMS
done

# Verify later
binfiddle hash sha256 --check SHA256SUMS

# Fast integrity check with xxhash64
for f in *.bin; do
    binfiddle -i "$f" hash xxhash64 >> XXHASH64SUMS
done
```

### Binary Patch Pipelines

```bash
# Build a reproducible patch
binfiddle diff original.bin modified.bin --diff-format patch > changes.patch

# Apply in-place with backup
binfiddle --in-file -i original.bin patch original.bin changes.patch --backup .orig

# Revert if needed
binfiddle --output reverted.bin patch original.bin changes.patch --revert

# Scripted multi-byte patch
binfiddle -i game.exe --in-file write 0x123456 90909090
binfiddle -i game.exe --in-file write 0x12345A EB10
binfiddle -i game.exe --in-file write 0x123500 "UNLOCKED" --input-format ascii
```

### Integration with Other Tools

```bash
# With radare2: extract .text section
RANGE=$(rabin2 -S binary | awk '/\.text/{print $2".."$3}')
binfiddle -i binary read "$RANGE" -o text.bin

# With xxd: hex to binary
binfiddle -i file.bin read 0..100 | xxd -r -p > raw.bin

# With strings
binfiddle -i file.bin read 0x1000..0x2000 | strings

# With file(1)
binfiddle -i archive.bin read 0..1000 -o sample.bin
file sample.bin
```

---

## Troubleshooting

### "Changes were made but no output specified"

```bash
# Wrong
binfiddle -i file.bin write 0 FF

# Correct
binfiddle -i file.bin write 0 FF -o out.bin
# Or
binfiddle -i file.bin --in-file write 0 FF
```

### "Index out of bounds"

```bash
# Check file size
stat -c%s file.bin
binfiddle -i file.bin read 0..$(stat -c%s file.bin)
```

### "Hex input must have even number of digits"

```bash
# Wrong
binfiddle -i file.bin write 0 ABC -o out.bin

# Correct
binfiddle -i file.bin write 0 0ABC -o out.bin
```

### "Invalid range"

Ranges are half-open: `10..20` means bytes 10–19. The start must be less than the end.

```bash
# Wrong
binfiddle -i file.bin read 100..50

# Correct
binfiddle -i file.bin read 50..100
```

### Regex searches are slow

For simple exact-byte patterns, prefer `hex`, `ascii`, or `mask`. Reserve `regex` for character classes, ranges, and quantifiers.

### Process-memory access denied

- Ensure you own the target process.
- Check `/proc/sys/kernel/yama/ptrace_scope`; values `1` or higher may block cross-process access.
- Some regions may be unreadable; use `--zero-fill-inaccessible` or `--skip-inaccessible` for reads.

### Performance Tips

- For files larger than RAM, use memory-mapped reads or `--stream` / `--block-size`.
- Use specific ranges instead of `..` when only a portion is needed.
- Combine operations in `chain` or scripts to minimize repeated I/O.
- Use CSV/JSON output for `analyze` when feeding results into other tools.

---

## Quick Reference Card

```
┌─────────────────────────────────────────────────────────────────┐
│                    BINFIDDLE QUICK REFERENCE                    │
├─────────────────────────────────────────────────────────────────┤
│ GLOBAL OPTIONS                                                  │
│   -i FILE          Input file (- for stdin)                     │
│   -o FILE          Output file (- for stdout)                   │
│   --in-file        Modify input in-place                        │
│   --format FMT     Output format (hex/dec/oct/bin/ascii/raw)    │
│   --input-format   Input value format                           │
│   --chunk-size N   Bits per chunk (default 8)                   │
│   --width N        Chunks per line (default 16)                 │
│   --show-offset    Show hex address prefix                      │
│   --show-ascii     Show ASCII sidebar                           │
│   --silent         Suppress diagnostics                         │
│   --progress       Show progress bars                           │
├─────────────────────────────────────────────────────────────────┤
│ READ                                                            │
│   binfiddle -i FILE read RANGE [--format FMT]                   │
│   binfiddle -i FILE read 0..64 --show-ascii                     │
├─────────────────────────────────────────────────────────────────┤
│ WRITE                                                           │
│   binfiddle -i FILE write POS VALUE -o OUT                      │
│   binfiddle -i FILE --in-file write 0x100 DEADBEEF              │
├─────────────────────────────────────────────────────────────────┤
│ EDIT                                                            │
│   binfiddle -i FILE edit insert POS DATA -o OUT                 │
│   binfiddle -i FILE edit remove START..END -o OUT               │
│   binfiddle -i FILE edit replace START..END DATA -o OUT         │
├─────────────────────────────────────────────────────────────────┤
│ SEARCH                                                          │
│   binfiddle -i FILE search PATTERN --all                        │
│   binfiddle -i FILE search "DE ?? EF" --input-format mask       │
│   binfiddle -i FILE search "text" --input-format ascii --count  │
├─────────────────────────────────────────────────────────────────┤
│ HASH                                                            │
│   binfiddle -i FILE hash sha256                                 │
│   binfiddle -i FILE hash sha1 --output-format base64            │
│   binfiddle -i FILE hash crc32 --block-size 4096                │
│   binfiddle -i FILE hash sha256 --stream --read-block-size 64M  │
│   binfiddle hash sha256 --check SHA256SUMS                      │
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
│ PROCESS MEMORY (Linux)                                          │
│   --process-self --list-regions                                 │
│   --pid PID --list-regions                                      │
│   --process-self --address ADDR --size N read 0..N              │
│   --process-self --address ADDR --size N --allow-write write 0..│
└─────────────────────────────────────────────────────────────────┘
```
