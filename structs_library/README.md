# Binfiddle Struct Templates Library

Comprehensive collection of binary structure templates for use with the `binfiddle struct` command.

## Usage

```bash
# Parse a binary file using a template
binfiddle struct structs_library/executables/elf64_header.yaml < /bin/ls

# Get a specific field value
binfiddle struct structs_library/images/png_header.yaml --get width < image.png

# Output as JSON
binfiddle struct structs_library/network/ipv4_header.yaml --format json < packet.bin

# List all fields in a template
binfiddle struct structs_library/audio/wav_riff_header.yaml --list-fields
```

## Available Templates

### Executables (5 templates)
- `elf32_header.yaml` - ELF 32-bit executable header
- `elf64_header.yaml` - ELF 64-bit executable header  
- `elf64_program_header.yaml` - ELF 64-bit program header table entry
- `macho64_header.yaml` - Mach-O 64-bit header (macOS executables)
- `pe_header.yaml` - PE/COFF header (Windows executables)

### Images (6 templates)
- `bmp_header.yaml` - Windows Bitmap header
- `gif_header.yaml` - GIF87a/89a header
- `jpeg_jfif_header.yaml` - JPEG/JFIF header with APP0 segment
- `png_header.yaml` - PNG file signature and IHDR chunk
- `tiff_header.yaml` - TIFF image file header
- `webp_header.yaml` - WebP RIFF container header

### Audio (3 templates)
- `flac_header.yaml` - FLAC audio stream header
- `mp3_id3v2_header.yaml` - MP3 ID3v2 metadata header
- `wav_riff_header.yaml` - WAV RIFF audio header

### Video (1 template)
- `mp4_isobmff_header.yaml` - MP4/ISO Base Media File Format header

### Compressed (4 templates)
- `bzip2_header.yaml` - Bzip2 compressed file header
- `gzip_header.yaml` - Gzip compressed file header
- `xz_header.yaml` - XZ/LZMA2 compressed file header
- `zip_header.yaml` - ZIP archive local file header

### Archives (1 template)
- `tar_ustar_header.yaml` - UNIX tar archive UStar header

### Data Formats (1 template)
- `sqlite_header.yaml` - SQLite3 database file header

### Network (3 templates)
- `ethernet_frame.yaml` - Ethernet II frame header
- `ipv4_header.yaml` - IPv4 packet header
- `tcp_header.yaml` - TCP segment header

### Document (1 template)
- `pdf_header.yaml` - PDF file signature and version

## Template Structure

Each template follows this YAML structure:

```yaml
name: Structure Name
description: Detailed description
endian: little  # or 'big'
fields:
  - name: field_name
    offset: 0x00
    size: 4
    type: u32  # u8, u16, u32, u64, i8, i16, i32, i64, hex_string, string, bytes, computed
    description: "Field purpose"
    assert: "expected_value"  # Optional - validates field value
    enum:  # Optional - maps values to names
      0: "Zero"
      1: "One"
    display: hex  # Optional - format override (hex, dec, bin, oct)
    when: $version >= 2  # Optional - conditional parsing
    bitfields:  # Optional - extract bitfields from integer fields
      - name: is_set
        bits: 0
        type: bool
```

## Supported Field Types

- **Integers**: `u8`, `u16`, `u32`, `u64` (unsigned), `i8`, `i16`, `i32`, `i64` (signed)
- **Strings**: `string` (ASCII/UTF-8, null-terminated or fixed length)
- **Raw Data**: `hex_string` (displayed as hex), `bytes` (raw bytes)
- **Computed**: `computed` — virtual field calculated from an expression

## Dynamic Templates

Templates support a small expression language for dynamic offsets, sizes, conditions, and computed values.

### Expressions

- **Field references**: `$fieldname`, `$fieldname.bitfield`
- **Magic variables**:
  - `$@current` — current parse offset
  - `$@prev_end` — end of previous field
  - `$@file_size` — total input size
  - `$@base` — base offset of the current template
- **Operators**: `+`, `-`, `*`, `/`, `%`, `**`, `==`, `!=`, `<`, `<=`, `>`, `>=`, `and`/`&&`, `or`/`||`, `not`
- **Numbers**: decimal (`42`) or hex (`0x2a`)

### Dynamic Offset/Size Example

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

### Conditional Field Example

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

### Computed Field Example

```yaml
fields:
  - name: total_size
    type: computed
    value: $header_size + $payload_size
```

### Bitfield Example

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
      - name: level
        bits: 2..5
        type: u8
```

### Bit-Level Field Example

Fields can be declared at arbitrary bit positions and widths. Bit ordering
follows the template's `endian` setting:

- `endian: big` — MSB-first within each byte (network protocols, registers).
- `endian: little` — LSB-first within each byte (some file formats).

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

### Array Example

```yaml
fields:
  - name: count
    offset: 0
    size: 1
    type: u8
  - name: entries
    offset: 1
    size: 1
    type: u8
    count: $count
```

## Contributing

When adding new templates:

1. Follow the existing naming convention: `format_component.yaml`
2. Include comprehensive field descriptions
3. Add enum mappings for magic numbers and flags
4. Use assertions for fixed values (magic bytes, version numbers)
5. Reference official specifications in comments
6. Test with real-world binary files

## References

Templates are based on official specifications:
- ELF: https://refspecs.linuxfoundation.org/elf/elf.pdf
- PE: https://learn.microsoft.com/en-us/windows/win32/debug/pe-format
- Mach-O: https://github.com/aidansteele/osx-abi-macho-file-format-reference
- PNG: http://www.libpng.org/pub/png/spec/
- JPEG: https://www.w3.org/Graphics/JPEG/
- And many more format-specific references included in template comments
