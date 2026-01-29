#!/bin/bash
#
# Binfiddle Release Build Script
#
# Usage:
#   ./build_releases.sh              # Build all available targets
#   ./build_releases.sh --native     # Build only for current platform
#   ./build_releases.sh --target <T> # Build specific target
#   ./build_releases.sh --setup      # Install dependencies only
#   ./build_releases.sh --clean      # Clean build artifacts
#   ./build_releases.sh --help       # Show help
#
# Environment Variables:
#   SKIP_MACOS=1    Skip macOS targets (default if osxcross not found)
#   SKIP_WINDOWS=1  Skip Windows targets
#   SKIP_ARCHIVE=1  Skip archive creation
#   USE_CROSS=1     Use 'cross' tool instead of cargo (recommended for cross-compilation)
#

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
VERSION=$(grep '^version' Cargo.toml | head -1 | cut -d '"' -f2)
PROJECT_NAME="binfiddle"
OUTPUT_DIR="releases/v${VERSION}"
BUILD_FLAGS="--release"

# All supported targets
ALL_TARGETS=(
    "x86_64-unknown-linux-gnu"
    "x86_64-unknown-linux-musl"
    "aarch64-unknown-linux-gnu"
    "x86_64-pc-windows-gnu"
    "x86_64-apple-darwin"
    "aarch64-apple-darwin"
)

# Detect current platform
detect_native_target() {
    local arch=$(uname -m)
    local os=$(uname -s)

    case "$os" in
        Linux)
            case "$arch" in
                x86_64)  echo "x86_64-unknown-linux-gnu" ;;
                aarch64) echo "aarch64-unknown-linux-gnu" ;;
                *)       echo "unknown" ;;
            esac
            ;;
        Darwin)
            case "$arch" in
                x86_64)  echo "x86_64-apple-darwin" ;;
                arm64)   echo "aarch64-apple-darwin" ;;
                *)       echo "unknown" ;;
            esac
            ;;
        MINGW*|MSYS*|CYGWIN*)
            echo "x86_64-pc-windows-gnu"
            ;;
        *)
            echo "unknown"
            ;;
    esac
}

NATIVE_TARGET=$(detect_native_target)

# Logging functions
log_info()    { echo -e "${BLUE}[INFO]${NC} $*"; }
log_success() { echo -e "${GREEN}[SUCCESS]${NC} $*"; }
log_warn()    { echo -e "${YELLOW}[WARN]${NC} $*"; }
log_error()   { echo -e "${RED}[ERROR]${NC} $*" >&2; }

# Check if a command exists
command_exists() {
    command -v "$1" &> /dev/null
}

# Check if we can build for a target
can_build_target() {
    local target=$1

    case "$target" in
        *-apple-darwin)
            # macOS targets require osxcross on Linux, or native macOS
            if [[ "$(uname -s)" == "Darwin" ]]; then
                return 0
            elif [[ -n "${OSXCROSS_SDK:-}" ]] || command_exists x86_64-apple-darwin-gcc; then
                return 0
            else
                return 1
            fi
            ;;
        *-windows-gnu)
            # Windows targets require mingw-w64
            command_exists x86_64-w64-mingw32-gcc
            ;;
        aarch64-unknown-linux-gnu)
            # ARM64 Linux requires cross-compiler or native
            if [[ "$NATIVE_TARGET" == "$target" ]]; then
                return 0
            fi
            command_exists aarch64-linux-gnu-gcc
            ;;
        x86_64-unknown-linux-musl)
            # musl target requires musl-gcc or musl-tools
            command_exists musl-gcc || [[ -f /usr/lib/x86_64-linux-musl/libc.a ]]
            ;;
        x86_64-unknown-linux-gnu)
            # Native Linux x86_64 - always available on Linux
            [[ "$(uname -s)" == "Linux" ]]
            ;;
        *)
            return 0
            ;;
    esac
}

# Get available targets based on installed toolchains
get_available_targets() {
    local available=()

    for target in "${ALL_TARGETS[@]}"; do
        # Skip macOS if SKIP_MACOS is set
        if [[ "${SKIP_MACOS:-0}" == "1" ]] && [[ "$target" == *-apple-darwin ]]; then
            continue
        fi

        # Skip Windows if SKIP_WINDOWS is set
        if [[ "${SKIP_WINDOWS:-0}" == "1" ]] && [[ "$target" == *-windows* ]]; then
            continue
        fi

        if can_build_target "$target"; then
            available+=("$target")
        else
            log_warn "Skipping $target (toolchain not available)"
        fi
    done

    echo "${available[@]}"
}

# Install Rust targets
install_rust_targets() {
    local targets=("$@")

    # Skip if rustup is not available (using system Rust)
    if ! command_exists rustup; then
        log_warn "rustup not found, skipping target installation (using system Rust)"
        return 0
    fi

    log_info "Installing Rust targets..."
    for target in "${targets[@]}"; do
        if ! rustup target list --installed | grep -q "^${target}$"; then
            log_info "Adding target: $target"
            rustup target add "$target" || log_warn "Failed to add target $target"
        fi
    done
}

# Setup dependencies (run with --setup)
setup_dependencies() {
    log_info "Setting up build dependencies..."

    if [[ "$(uname -s)" != "Linux" ]]; then
        log_warn "Dependency setup is only automated for Linux"
        return 0
    fi

    # Detect package manager
    if command_exists apt-get; then
        log_info "Installing dependencies via apt..."
        sudo apt-get update
        sudo apt-get install -y \
            build-essential \
            gcc \
            mingw-w64 \
            musl-tools \
            || log_warn "Some packages failed to install"

        # Try to install ARM64 cross-compiler (may fail on some systems)
        sudo apt-get install -y \
            gcc-aarch64-linux-gnu \
            binutils-aarch64-linux-gnu \
            2>/dev/null || log_warn "ARM64 cross-compiler not available on this system"

    elif command_exists dnf; then
        log_info "Installing dependencies via dnf..."
        sudo dnf install -y \
            gcc \
            mingw64-gcc \
            musl-gcc \
            || log_warn "Some packages failed to install"

    elif command_exists pacman; then
        log_info "Installing dependencies via pacman..."
        sudo pacman -S --noconfirm \
            base-devel \
            mingw-w64-gcc \
            musl \
            || log_warn "Some packages failed to install"
    else
        log_warn "Unknown package manager. Please install dependencies manually."
    fi

    # Install cross tool for easier cross-compilation
    if ! command_exists cross; then
        log_info "Installing 'cross' for easier cross-compilation..."
        cargo install cross --git https://github.com/cross-rs/cross 2>/dev/null \
            || log_warn "Failed to install 'cross'. Cross-compilation may be limited."
    fi

    log_success "Dependency setup complete"
}

# Configure cargo for cross-compilation
configure_cargo_target() {
    local target=$1

    # Unset any previous configuration
    unset CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER 2>/dev/null || true
    unset CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER 2>/dev/null || true
    unset CARGO_TARGET_X86_64_APPLE_DARWIN_LINKER 2>/dev/null || true
    unset CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER 2>/dev/null || true
    unset CC 2>/dev/null || true
    unset AR 2>/dev/null || true

    case "$target" in
        x86_64-pc-windows-gnu)
            export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="x86_64-w64-mingw32-gcc"
            export CC="x86_64-w64-mingw32-gcc"
            export AR="x86_64-w64-mingw32-ar"
            ;;
        aarch64-unknown-linux-gnu)
            if [[ "$NATIVE_TARGET" != "$target" ]]; then
                export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="aarch64-linux-gnu-gcc"
                export CC="aarch64-linux-gnu-gcc"
                export AR="aarch64-linux-gnu-ar"
            fi
            ;;
        x86_64-apple-darwin)
            if [[ "$(uname -s)" != "Darwin" ]]; then
                # Requires osxcross
                export CARGO_TARGET_X86_64_APPLE_DARWIN_LINKER="x86_64-apple-darwin-clang"
                export CC="x86_64-apple-darwin-clang"
                export AR="x86_64-apple-darwin-ar"
            fi
            ;;
        aarch64-apple-darwin)
            if [[ "$(uname -s)" != "Darwin" ]]; then
                # Requires osxcross
                export CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER="aarch64-apple-darwin-clang"
                export CC="aarch64-apple-darwin-clang"
                export AR="aarch64-apple-darwin-ar"
            fi
            ;;
    esac
}

# Build a single target
build_target() {
    local target=$1
    local suffix=""
    local use_cross="${USE_CROSS:-0}"

    case "$target" in
        *windows*) suffix=".exe" ;;
    esac

    log_info "Building for $target..."

    # Decide whether to use cross or cargo
    local build_cmd="cargo"
    if [[ "$use_cross" == "1" ]] && command_exists cross; then
        # Use cross for non-native targets
        if [[ "$target" != "$NATIVE_TARGET" ]]; then
            build_cmd="cross"
            log_info "Using 'cross' for $target"
        fi
    else
        # Configure cargo for cross-compilation
        configure_cargo_target "$target"
    fi

    # Build
    if ! $build_cmd build $BUILD_FLAGS --target "$target"; then
        log_error "Build failed for $target"
        return 1
    fi

    # Copy binary to output
    local bin_name="${PROJECT_NAME}${suffix}"
    local src_path="target/${target}/release/${bin_name}"
    local dst_path="${OUTPUT_DIR}/${bin_name}-${target}"

    if [[ ! -f "$src_path" ]]; then
        log_error "Binary not found: $src_path"
        return 1
    fi

    cp "$src_path" "$dst_path"

    # Strip binary (reduces size significantly)
    case "$target" in
        *linux*)
            strip "$dst_path" 2>/dev/null || true
            ;;
        *windows*)
            x86_64-w64-mingw32-strip "$dst_path" 2>/dev/null || true
            ;;
    esac

    # Create checksum
    if command_exists sha256sum; then
        sha256sum "$dst_path" > "${dst_path}.sha256"
    elif command_exists shasum; then
        shasum -a 256 "$dst_path" > "${dst_path}.sha256"
    fi

    local size=$(du -h "$dst_path" | cut -f1)
    log_success "Built $target ($size)"

    return 0
}

# Create release archives
create_archives() {
    if [[ "${SKIP_ARCHIVE:-0}" == "1" ]]; then
        log_info "Skipping archive creation (SKIP_ARCHIVE=1)"
        return 0
    fi

    log_info "Creating release archives..."

    pushd "$OUTPUT_DIR" >/dev/null

    for target in "${BUILT_TARGETS[@]}"; do
        local suffix=""
        case "$target" in
            *windows*) suffix=".exe" ;;
        esac

        local bin_name="${PROJECT_NAME}${suffix}-${target}"
        local archive_name="${PROJECT_NAME}-v${VERSION}-${target}"

        if [[ ! -f "$bin_name" ]]; then
            log_warn "Binary not found for $target, skipping archive"
            continue
        fi

        log_info "Creating archive: $archive_name"

        case "$target" in
            *windows*)
                if command_exists zip; then
                    zip -q "${archive_name}.zip" "$bin_name" "${bin_name}.sha256" 2>/dev/null || \
                    zip -q "${archive_name}.zip" "$bin_name"
                elif command_exists 7z; then
                    7z a -bso0 "${archive_name}.zip" "$bin_name" "${bin_name}.sha256" 2>/dev/null || \
                    7z a -bso0 "${archive_name}.zip" "$bin_name"
                else
                    log_warn "No zip tool available, creating tar.gz instead"
                    tar czf "${archive_name}.tar.gz" "$bin_name" "${bin_name}.sha256" 2>/dev/null || \
                    tar czf "${archive_name}.tar.gz" "$bin_name"
                fi
                ;;
            *)
                tar czf "${archive_name}.tar.gz" "$bin_name" "${bin_name}.sha256" 2>/dev/null || \
                tar czf "${archive_name}.tar.gz" "$bin_name"
                ;;
        esac
    done

    popd >/dev/null

    log_success "Archives created in $OUTPUT_DIR"
}

# Print build summary
print_summary() {
    echo ""
    echo "=========================================="
    echo "  Binfiddle v${VERSION} Build Summary"
    echo "=========================================="
    echo ""

    if [[ ${#BUILT_TARGETS[@]} -gt 0 ]]; then
        log_success "Successfully built ${#BUILT_TARGETS[@]} target(s):"
        for target in "${BUILT_TARGETS[@]}"; do
            local suffix=""
            case "$target" in
                *windows*) suffix=".exe" ;;
            esac
            local bin="${OUTPUT_DIR}/${PROJECT_NAME}${suffix}-${target}"
            if [[ -f "$bin" ]]; then
                local size=$(du -h "$bin" | cut -f1)
                echo "  ✓ $target ($size)"
            fi
        done
    fi

    if [[ ${#FAILED_TARGETS[@]} -gt 0 ]]; then
        echo ""
        log_error "Failed to build ${#FAILED_TARGETS[@]} target(s):"
        for target in "${FAILED_TARGETS[@]}"; do
            echo "  ✗ $target"
        done
    fi

    echo ""
    echo "Output directory: $OUTPUT_DIR"
    echo ""
}

# Clean build artifacts
clean() {
    log_info "Cleaning build artifacts..."
    cargo clean
    rm -rf releases/
    log_success "Clean complete"
}

# Show help
show_help() {
    cat << EOF
Binfiddle Release Build Script

Usage: $0 [OPTIONS]

Options:
    --native        Build only for current platform ($NATIVE_TARGET)
    --target <T>    Build for specific target
    --setup         Install build dependencies (requires sudo)
    --clean         Clean build artifacts
    --list          List all supported targets
    --help          Show this help message

Environment Variables:
    SKIP_MACOS=1    Skip macOS targets
    SKIP_WINDOWS=1  Skip Windows targets
    SKIP_ARCHIVE=1  Skip archive creation
    USE_CROSS=1     Use 'cross' tool for cross-compilation

Supported Targets:
EOF
    for target in "${ALL_TARGETS[@]}"; do
        local status="available"
        if ! can_build_target "$target"; then
            status="unavailable (missing toolchain)"
        fi
        if [[ "$target" == "$NATIVE_TARGET" ]]; then
            status="native"
        fi
        echo "    $target [$status]"
    done

    cat << EOF

Examples:
    $0                          # Build all available targets
    $0 --native                 # Build only for current platform
    $0 --target x86_64-unknown-linux-gnu
    SKIP_MACOS=1 $0             # Build all except macOS
    USE_CROSS=1 $0              # Use 'cross' for cross-compilation

EOF
}

# Main entry point
main() {
    local build_native_only=false
    local specific_target=""

    # Parse arguments
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --native)
                build_native_only=true
                shift
                ;;
            --target)
                specific_target="$2"
                shift 2
                ;;
            --setup)
                setup_dependencies
                exit 0
                ;;
            --clean)
                clean
                exit 0
                ;;
            --list)
                echo "Supported targets:"
                for t in "${ALL_TARGETS[@]}"; do
                    echo "  $t"
                done
                exit 0
                ;;
            --help|-h)
                show_help
                exit 0
                ;;
            *)
                log_error "Unknown option: $1"
                show_help
                exit 1
                ;;
        esac
    done

    # Ensure we're in the project root
    if [[ ! -f "Cargo.toml" ]]; then
        log_error "Must be run from project root (Cargo.toml not found)"
        exit 1
    fi

    # Determine which targets to build
    local targets_to_build=()

    if [[ -n "$specific_target" ]]; then
        targets_to_build=("$specific_target")
    elif [[ "$build_native_only" == true ]]; then
        targets_to_build=("$NATIVE_TARGET")
    else
        read -ra targets_to_build <<< "$(get_available_targets)"
    fi

    if [[ ${#targets_to_build[@]} -eq 0 ]]; then
        log_error "No targets available to build"
        exit 1
    fi

    log_info "Building binfiddle v${VERSION}"
    log_info "Targets: ${targets_to_build[*]}"

    # Install Rust targets
    install_rust_targets "${targets_to_build[@]}"

    # Create output directory
    mkdir -p "$OUTPUT_DIR"

    # Track results
    BUILT_TARGETS=()
    FAILED_TARGETS=()

    # Build each target
    for target in "${targets_to_build[@]}"; do
        if build_target "$target"; then
            BUILT_TARGETS+=("$target")
        else
            FAILED_TARGETS+=("$target")
        fi
    done

    # Create archives
    if [[ ${#BUILT_TARGETS[@]} -gt 0 ]]; then
        create_archives
    fi

    # Print summary
    print_summary

    # Exit with error if any builds failed
    if [[ ${#FAILED_TARGETS[@]} -gt 0 ]]; then
        exit 1
    fi
}

# Run main
main "$@"
