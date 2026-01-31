# Binfiddle Usage Guide

This guide provides detailed usage instructions, examples, and common workflows for binfiddle.

## Table of Contents

- [Getting Started](#getting-started)
- [Basic Operations](#basic-operations)
  - [Reading Data](#reading-data)
  - [Writing Data](#writing-data)
  - [Editing Data](#editing-data)
  - [Searching Data](#searching-data)
  - [Analyzing Data](#analyzing-data)
  - [Comparing Files (Diff)](#comparing-files-diff)
  - [Converting Encodings](#converting-encodings)
  - [Applying Patches](#applying-patches)
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
| `--color <MODE>` | Color output: always, auto, never |

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
| `--block-size <N>` | Analyze in blocks of N bytes (0 = entire file) [default: 256] |
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
│   --context N     Show N bytes context (search/diff)            │
│   --color MODE    Color output: always, auto, never             │
└─────────────────────────────────────────────────────────────────┘
```
