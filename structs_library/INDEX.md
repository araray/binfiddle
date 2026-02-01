# Binfiddle Struct Templates - Complete Index

## Summary Statistics

- **Total Templates**: 44
- **Categories**: 12
- **New in this session**: 19 templates
- **Coverage**: Executables, Mobile, ML Models, Firmware, Containers, Filesystem, Network, Security

## Category Breakdown

### Executables (13 templates)
Binary executable and library formats across all major platforms.

- `elf32_header.yaml` - ELF 32-bit executable header
- `elf32_program_header.yaml` - ELF 32-bit program header entry
- `elf32_section_header.yaml` - ELF 32-bit section header entry
- `elf64_header.yaml` - ELF 64-bit executable header
- `elf64_program_header.yaml` - ELF 64-bit program header entry
- `elf64_section_header.yaml` - ELF 64-bit section header entry
- `elf64_dynamic_entry.yaml` - ELF 64-bit dynamic section entry (.so files)
- `elf64_symbol.yaml` - ELF 64-bit symbol table entry
- `java_class_header.yaml` - Java .class file header
- `macho64_header.yaml` - Mach-O 64-bit header (macOS/iOS)
- `macho_universal_header.yaml` - Mach-O universal/fat binary header
- `pe_header.yaml` - PE/COFF header (Windows .exe/.dll)

**Coverage**: ELF (32/64-bit, headers, program headers, section headers, dynamic, symbols), Mach-O (64-bit, universal), PE/COFF, Java bytecode

### Archives (2 templates)
Archive and static library formats.

- `ar_header.yaml` - Unix ar archive header (.a static libraries)
- `tar_ustar_header.yaml` - UNIX tar archive UStar header

**Coverage**: ar (.a libs), tar

### Audio (3 templates)
Audio file formats and metadata.

- `flac_header.yaml` - FLAC lossless audio header
- `mp3_id3v2_header.yaml` - MP3 ID3v2 metadata header
- `wav_riff_header.yaml` - WAV RIFF audio header

**Coverage**: FLAC, MP3/ID3v2, WAV

### Video (1 template)
Video container formats.

- `mp4_isobmff_header.yaml` - MP4/ISO Base Media File Format header

**Coverage**: MP4/ISOBMFF

### Images (6 templates)
Image file formats.

- `bmp_header.yaml` - Windows Bitmap header
- `gif_header.yaml` - GIF87a/89a header
- `jpeg_jfif_header.yaml` - JPEG/JFIF header with APP0
- `png_header.yaml` - PNG file signature and IHDR chunk
- `tiff_header.yaml` - TIFF image file header
- `webp_header.yaml` - WebP RIFF container header

**Coverage**: BMP, GIF, JPEG/JFIF, PNG, TIFF, WebP

### Compressed (4 templates)
Compression formats.

- `bzip2_header.yaml` - Bzip2 compressed file header
- `gzip_header.yaml` - Gzip compressed file header
- `xz_header.yaml` - XZ/LZMA2 compressed file header
- `zip_header.yaml` - ZIP archive local file header

**Coverage**: Bzip2, Gzip, XZ/LZMA2, ZIP

### Data Formats (1 template)
Database and structured data formats.

- `sqlite_header.yaml` - SQLite3 database file header

**Coverage**: SQLite3

### Document (1 template)
Document file formats.

- `pdf_header.yaml` - PDF file signature and version

**Coverage**: PDF

### Network (6 templates)
Network protocol headers.

- `dns_header.yaml` - DNS message header (queries/responses)
- `ethernet_frame.yaml` - Ethernet II frame header
- `ipv4_header.yaml` - IPv4 packet header
- `ipv6_header.yaml` - IPv6 packet header
- `tcp_header.yaml` - TCP segment header
- `udp_header.yaml` - UDP datagram header

**Coverage**: DNS, Ethernet, IPv4, IPv6, TCP, UDP

### Mobile (1 template)
Mobile platform formats (Android, iOS).

- `android_dex_header.yaml` - Android DEX (Dalvik Executable) header

**Coverage**: Android DEX

### ML Models (2 templates)
Machine learning model formats.

- `gguf_header.yaml` - GGUF (llama.cpp) LLM model header
- `onnx_header.yaml` - ONNX neural network model header

**Coverage**: GGUF (llama.cpp, Ollama, LM Studio), ONNX (PyTorch, TensorFlow)

### Firmware (2 templates)
Embedded system and firmware formats.

- `intel_hex_record.yaml` - Intel HEX firmware format (AVR, ARM, PIC)
- `uf2_block.yaml` - UF2 firmware format (RP2040, ESP32, SAMD, nRF52)

**Coverage**: Intel HEX, UF2 (Arduino, Raspberry Pi Pico, ESP32)

### Containers (1 template)
Container and virtualization formats.

- `docker_manifest.yaml` - Docker image manifest.json structure

**Coverage**: Docker images

### Filesystem (1 template)
Filesystem formats.

- `iso9660_volume_descriptor.yaml` - ISO 9660 CD/DVD filesystem

**Coverage**: ISO 9660 (CD/DVD images)

### Security (1 template)
Cryptography and security formats.

- `x509_der_certificate.yaml` - X.509 DER certificate encoding

**Coverage**: X.509 certificates (TLS/SSL, code signing)

## New Templates This Session (19)

### Executables (9)
- ELF 32-bit header, program header, section header
- ELF 64-bit program header, section header, dynamic entry, symbol table
- Mach-O universal binary header
- Java class file header

### Archives (1)
- Unix ar archive header (.a static libraries)

### Network (3)
- UDP datagram header
- IPv6 packet header
- DNS message header

### Mobile (1)
- Android DEX header

### ML Models (2)
- GGUF model header (llama.cpp)
- ONNX model header

### Firmware (2)
- Intel HEX firmware format
- UF2 firmware format

### Containers (1)
- Docker image manifest

### Filesystem (1)
- ISO 9660 volume descriptor

### Security (1)
- X.509 DER certificate

## Usage Examples

```bash
# Parse ELF executable
binfiddle struct structs_library/executables/elf64_header.yaml -i /bin/ls

# Analyze shared library dependencies
binfiddle struct structs_library/executables/elf64_dynamic_entry.yaml -i /lib/x86_64-linux-gnu/libc.so.6

# Inspect Android APK
unzip -p app.apk classes.dex | binfiddle struct structs_library/mobile/android_dex_header.yaml -i -

# Check LLM model format
binfiddle struct structs_library/ml_models/gguf_header.yaml -i model.gguf

# Analyze network packet
tcpdump -w packet.pcap -c 1 port 53
binfiddle struct structs_library/network/dns_header.yaml -i packet.pcap

# Inspect ISO image
binfiddle struct structs_library/filesystem/iso9660_volume_descriptor.yaml -i ubuntu.iso

# Examine TLS certificate
openssl s_client -connect example.com:443 </dev/null 2>/dev/null | \
  openssl x509 -outform DER | \
  binfiddle struct structs_library/security/x509_der_certificate.yaml -i -
```

## Quality Standards

All templates meet these criteria:
✅ Comprehensive field descriptions  
✅ Enum mappings for magic numbers and constants  
✅ Assertions for validation  
✅ Official specification references  
✅ Real-world usage examples  
✅ Complete field coverage  
✅ Forensics-aware security notes  

## Future Expansion Areas

Templates planned but not yet created (50+ formats):

- **More executables**: .NET assemblies, Python bytecode (.pyc), WebAssembly (.wasm)
- **ARM/Embedded**: ARM Cortex-M firmware, RISC-V binaries, FPGA bitstreams
- **Additional ML**: PyTorch (.pt), TensorFlow SavedModel, Safetensors
- **More firmware**: S-record (Motorola), BIN, HEX variants
- **Additional network**: ICMP, ARP, DHCP, HTTP/2, TLS records
- **Filesystems**: ext4, FAT32, NTFS boot sectors
- **Game formats**: Unity assets, Unreal .pak, Minecraft NBT
- **Additional security**: PGP packets, SSH keys, PKCS#12

See STRUCTS_INVENTORY.md for complete roadmap.
