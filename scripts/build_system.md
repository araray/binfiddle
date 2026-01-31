# Build System Documentation

## Overview

Binfiddle provides two cross-platform build scripts for creating release binaries:

- **`build_releases.sh`** - Bash script for Linux/macOS/WSL
- **`build_releases.ps1`** - PowerShell script for Windows/cross-platform PowerShell Core

Both scripts support:
- Multi-target compilation
- Cross-compilation
- Automated archive creation
- SHA256 checksum generation
- Dependency installation
- GitHub release automation

---

## Quick Start

### Linux/macOS/WSL

```bash
# Build all available targets
./build_releases.sh

# Build only for current platform
./build_releases.sh --native

# Build specific target
./build_releases.sh --target x86_64-unknown-linux-musl

# Install dependencies (Ubuntu/Debian)
./build_releases.sh --setup
```

### Windows

```powershell
# Build all available targets
.\build_releases.ps1

# Build only for current platform
.\build_releases.ps1 -Native

# Build specific target
.\build_releases.ps1 -Target x86_64-pc-windows-msvc

# Install dependencies (requires admin)
.\build_releases.ps1 -Setup
```

---

## Supported Targets

| Target | Platform | Notes |
|--------|----------|-------|
| `x86_64-unknown-linux-gnu` | Linux x86_64 | Native Linux builds |
| `x86_64-unknown-linux-musl` | Linux x86_64 (static) | Requires `musl-tools` |
| `aarch64-unknown-linux-gnu` | Linux ARM64 | Requires cross-compiler |
| `x86_64-pc-windows-gnu` | Windows x86_64 (MinGW) | Requires `mingw-w64` |
| `x86_64-pc-windows-msvc` | Windows x86_64 (MSVC) | Requires Visual Studio/Build Tools |
| `x86_64-apple-darwin` | macOS Intel | Requires macOS or osxcross |
| `aarch64-apple-darwin` | macOS Apple Silicon | Requires macOS or osxcross |

---

## Command Reference

### Bash Script (`build_releases.sh`)

#### Basic Usage

```bash
./build_releases.sh [OPTIONS]
```

#### Options

| Option | Description |
|--------|-------------|
| `--native` | Build only for current platform |
| `--target <TRIPLE>` | Build for specific target |
| `--setup` | Install build dependencies (requires sudo) |
| `--clean` | Clean build artifacts |
| `--list` | List all supported targets |
| `--help` | Show help message |

#### Environment Variables

| Variable | Effect |
|----------|--------|
| `SKIP_MACOS=1` | Skip macOS targets |
| `SKIP_WINDOWS=1` | Skip Windows targets |
| `SKIP_ARCHIVE=1` | Skip archive creation |
| `USE_CROSS=1` | Use `cross` tool instead of cargo |

#### Examples

```bash
# Build all targets except macOS
SKIP_MACOS=1 ./build_releases.sh

# Use 'cross' for cross-compilation
USE_CROSS=1 ./build_releases.sh

# Build without creating archives
SKIP_ARCHIVE=1 ./build_releases.sh --native

# Clean and rebuild
./build_releases.sh --clean
./build_releases.sh
```

### PowerShell Script (`build_releases.ps1`)

#### Basic Usage

```powershell
.\build_releases.ps1 [OPTIONS]
```

#### Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `-Native` | Switch | Build only for current platform |
| `-Target` | String | Build for specific target |
| `-Setup` | Switch | Install build dependencies |
| `-Clean` | Switch | Clean build artifacts |
| `-List` | Switch | List all supported targets |
| `-Help` | Switch | Show help message |
| `-SkipMacOS` | Switch | Skip macOS targets |
| `-SkipLinux` | Switch | Skip Linux targets |
| `-SkipArchive` | Switch | Skip archive creation |
| `-UseCross` | Switch | Use `cross` tool |

#### Examples

```powershell
# Build all targets except macOS
.\build_releases.ps1 -SkipMacOS

# Use 'cross' for cross-compilation
.\build_releases.ps1 -UseCross

# Build without creating archives
.\build_releases.ps1 -Native -SkipArchive

# Clean and rebuild
.\build_releases.ps1 -Clean
.\build_releases.ps1
```

---

## Dependency Installation

### Linux (Debian/Ubuntu)

```bash
# Automated (recommended)
./build_releases.sh --setup

# Manual
sudo apt-get update
sudo apt-get install -y \
    build-essential \
    gcc \
    mingw-w64 \
    musl-tools \
    gcc-aarch64-linux-gnu \
    binutils-aarch64-linux-gnu

# Install 'cross' for easier cross-compilation
cargo install cross --git https://github.com/cross-rs/cross
```

### Linux (Fedora/RHEL)

```bash
sudo dnf install -y \
    gcc \
    mingw64-gcc \
    musl-gcc
```

### Linux (Arch)

```bash
sudo pacman -S --noconfirm \
    base-devel \
    mingw-w64-gcc \
    musl
```

### macOS

```bash
# Install Xcode Command Line Tools
xcode-select --install

# Install Homebrew (if not present)
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"

# Install mingw-w64 for Windows cross-compilation
brew install mingw-w64
```

### Windows

```powershell
# Automated (requires admin)
.\build_releases.ps1 -Setup

# Manual via Chocolatey
choco install rust mingw 7zip -y

# Install Visual Studio Build Tools (for MSVC target)
# Download from: https://visualstudio.microsoft.com/downloads/
```

---

## Cross-Compilation

### Using `cross`

The [`cross`](https://github.com/cross-rs/cross) tool provides Docker-based cross-compilation:

```bash
# Install cross
cargo install cross --git https://github.com/cross-rs/cross

# Use with build script
USE_CROSS=1 ./build_releases.sh
```

```powershell
# PowerShell equivalent
.\build_releases.ps1 -UseCross
```

**Advantages:**
- No need to install cross-compilers
- Consistent builds across different host platforms
- Automatic Docker container management

**Requirements:**
- Docker installed and running
- User must have Docker permissions

### Using osxcross (Linux → macOS)

To cross-compile macOS binaries on Linux:

1. **Install osxcross:**
   ```bash
   git clone https://github.com/tpoechtrager/osxcross
   cd osxcross
   # Follow instructions in README.md to package SDK
   ./build.sh
   ```

2. **Set environment:**
   ```bash
   export PATH="$PATH:/path/to/osxcross/target/bin"
   export OSXCROSS_SDK=/path/to/osxcross/target/SDK/MacOSX.sdk
   ```

3. **Build:**
   ```bash
   ./build_releases.sh --target x86_64-apple-darwin
   ```

---

## Output Structure

After running a build, the output directory structure is:

```
releases/v<VERSION>/
├── binfiddle-x86_64-unknown-linux-gnu
├── binfiddle-x86_64-unknown-linux-gnu.sha256
├── binfiddle-v<VERSION>-x86_64-unknown-linux-gnu.tar.gz
├── binfiddle.exe-x86_64-pc-windows-msvc
├── binfiddle.exe-x86_64-pc-windows-msvc.sha256
├── binfiddle-v<VERSION>-x86_64-pc-windows-msvc.zip
└── ...
```

**File Types:**

| Extension | Content |
|-----------|---------|
| (no extension) | Compiled binary (Linux/macOS) |
| `.exe` | Compiled binary (Windows) |
| `.sha256` | SHA256 checksum of binary |
| `.tar.gz` | Archive for Linux/macOS targets |
| `.zip` | Archive for Windows targets |

---

## CI/CD Integration

### GitHub Actions

Create `.github/workflows/release.yml`:

```yaml
name: Release Build

on:
  push:
    tags:
      - 'v*'
  workflow_dispatch:

jobs:
  build-linux:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      
      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable
      
      - name: Install dependencies
        run: |
          sudo apt-get update
          sudo apt-get install -y mingw-w64 musl-tools
          cargo install cross --git https://github.com/cross-rs/cross
      
      - name: Build releases
        run: USE_CROSS=1 ./build_releases.sh
      
      - name: Upload artifacts
        uses: actions/upload-artifact@v4
        with:
          name: linux-builds
          path: releases/v*/

  build-macos:
    runs-on: macos-latest
    steps:
      - uses: actions/checkout@v4
      
      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable
      
      - name: Build releases
        run: ./build_releases.sh --native
      
      - name: Upload artifacts
        uses: actions/upload-artifact@v4
        with:
          name: macos-builds
          path: releases/v*/

  build-windows:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4
      
      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable
      
      - name: Build releases
        run: .\build_releases.ps1 -Native
      
      - name: Upload artifacts
        uses: actions/upload-artifact@v4
        with:
          name: windows-builds
          path: releases/v*/

  create-release:
    needs: [build-linux, build-macos, build-windows]
    runs-on: ubuntu-latest
    steps:
      - uses: actions/download-artifact@v4
      
      - name: Create Release
        uses: softprops/action-gh-release@v1
        with:
          files: |
            **/*.tar.gz
            **/*.zip
            **/*.sha256
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

### GitLab CI

Create `.gitlab-ci.yml`:

```yaml
stages:
  - build
  - release

variables:
  CARGO_HOME: ${CI_PROJECT_DIR}/.cargo

build:linux:
  stage: build
  image: rust:latest
  before_script:
    - apt-get update
    - apt-get install -y mingw-w64 musl-tools
  script:
    - ./build_releases.sh
  artifacts:
    paths:
      - releases/

build:macos:
  stage: build
  tags:
    - macos
  script:
    - ./build_releases.sh --native
  artifacts:
    paths:
      - releases/

release:
  stage: release
  image: registry.gitlab.com/gitlab-org/release-cli:latest
  script:
    - echo "Creating release"
  release:
    tag_name: $CI_COMMIT_TAG
    description: 'Release $CI_COMMIT_TAG'
    assets:
      links:
        - name: 'Binaries'
          url: '${CI_PROJECT_URL}/-/jobs/artifacts/${CI_COMMIT_TAG}/browse/releases'
```

---

## Troubleshooting

### Build Fails with "toolchain not available"

**Problem:** Missing cross-compilation toolchain

**Solution:**
```bash
# Install dependencies
./build_releases.sh --setup

# Or use 'cross' for Docker-based builds
USE_CROSS=1 ./build_releases.sh
```

### Permission Denied on Linux/macOS

**Problem:** Script not executable

**Solution:**
```bash
chmod +x build_releases.sh
./build_releases.sh
```

### PowerShell Execution Policy Error

**Problem:** Script execution blocked by policy

**Solution:**
```powershell
# Temporary bypass (current session only)
Set-ExecutionPolicy -Scope Process -ExecutionPolicy Bypass

# Or run with bypass
powershell -ExecutionPolicy Bypass -File .\build_releases.ps1
```

### Windows: "mingw-w64 not found"

**Problem:** MinGW not in PATH

**Solution:**
```powershell
# Install via Chocolatey
choco install mingw -y

# Or manually add to PATH
$env:Path += ";C:\Program Files\mingw-w64\bin"
```

### macOS: "osxcross not found"

**Problem:** Building macOS targets from Linux without osxcross

**Solution:**
```bash
# Skip macOS targets
SKIP_MACOS=1 ./build_releases.sh

# Or install osxcross (see Cross-Compilation section)
```

### Docker Errors with `cross`

**Problem:** Docker daemon not running or permission denied

**Solution:**
```bash
# Start Docker daemon
sudo systemctl start docker

# Add user to docker group (requires logout/login)
sudo usermod -aG docker $USER
```

---

## Advanced Usage

### Custom Build Flags

Modify `BUILD_FLAGS` in the script:

```bash
# Bash
BUILD_FLAGS="--release --features custom-feature" ./build_releases.sh

# PowerShell
$env:BUILD_FLAGS="--release --features custom-feature"
.\build_releases.ps1
```

### Building for Embedded Targets

Add custom targets to the `ALL_TARGETS` array in the script:

```bash
# Example: Add ARM Cortex-M target
ALL_TARGETS=(
    # ... existing targets ...
    "thumbv7em-none-eabihf"
)
```

### Custom Linker Configuration

For specialized linker settings, modify `configure_cargo_target()` function:

```bash
configure_cargo_target() {
    case "$target" in
        my-custom-target)
            export CARGO_TARGET_MY_CUSTOM_TARGET_LINKER="my-linker"
            export RUSTFLAGS="-C link-args=-custom-flag"
            ;;
    esac
}
```

---

## Performance Optimization

### Parallel Builds

Cargo builds in parallel by default. Adjust with:

```bash
# Use all CPU cores
cargo build --release -j $(nproc)

# Limit to 4 cores
cargo build --release -j 4
```

### Caching

For CI/CD, cache Cargo dependencies:

```yaml
# GitHub Actions
- uses: actions/cache@v4
  with:
    path: |
      ~/.cargo/registry
      ~/.cargo/git
      target
    key: ${{ runner.os }}-cargo-${{ hashFiles('**/Cargo.lock') }}
```

### Link-Time Optimization (LTO)

Already enabled in `Cargo.toml`:

```toml
[profile.release]
lto = true
codegen-units = 1
```

For even smaller binaries, consider:

```toml
[profile.release]
lto = "fat"
strip = true
opt-level = "z"  # Optimize for size
```

---

## Maintenance

### Updating Target Support

When Rust adds new targets:

1. Add to `ALL_TARGETS` array
2. Add toolchain check in `can_build_target()`
3. Add linker configuration in `configure_cargo_target()`
4. Test build
5. Update documentation

### Version Management

Version is automatically extracted from `Cargo.toml`:

```toml
[package]
version = "0.9.0"  # This determines release version
```

---

## References

- [Rust Platform Support](https://doc.rust-lang.org/nightly/rustc/platform-support.html)
- [cross Tool](https://github.com/cross-rs/cross)
- [osxcross](https://github.com/tpoechtrager/osxcross)
- [Cargo Build Options](https://doc.rust-lang.org/cargo/commands/cargo-build.html)
- [GitHub Actions](https://docs.github.com/en/actions)

---

## License

This build system is part of binfiddle and follows the same license (BSD 3-Clause Clear).
