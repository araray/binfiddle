#!/bin/bash
set -euo pipefail

# Configuration
VERSION=$(grep '^version' Cargo.toml | cut -d '"' -f2)
TARGETS=(
    "x86_64-unknown-linux-gnu"
    "x86_64-pc-windows-gnu"
    "x86_64-apple-darwin"
    "aarch64-apple-darwin"
)
OUTPUT_DIR="releases/v${VERSION}"
BUILD_FLAGS="--release"

# Install cross-compilation toolchains
rustup target add "${TARGETS[@]}"

# Install cross-compilers (Ubuntu/Debian example)
sudo apt-get install -y \
    gcc-multilib \
    mingw-w64 \
    clang \
    lld \
    gcc-aarch64-linux-gnu \
    binutils-aarch64-linux-gnu

# Create output directory
mkdir -p "$OUTPUT_DIR"

build_target() {
    local target=$1
    local suffix=""

    case $target in
        *windows*) suffix=".exe" ;;
    esac

    echo "Building for $target..."

    # Set linker for cross-compilation
    case $target in
        aarch64-unknown-linux-gnu)
            export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
            ;;
    esac

    cargo build $BUILD_FLAGS --target "$target"

    # Copy binary
    local bin_name="binfiddle${suffix}"
    cp "target/${target}/release/${bin_name}" "${OUTPUT_DIR}/${bin_name}-${target}"

    # Create checksum
    sha256sum "${OUTPUT_DIR}/${bin_name}-${target}" > "${OUTPUT_DIR}/${bin_name}-${target}.sha256"
}

# Build all targets
for target in "${TARGETS[@]}"; do
    build_target "$target"
done

# Create archive for each platform
create_archives() {
    pushd "$OUTPUT_DIR" >/dev/null

    for target in "${TARGETS[@]}"; do
        local suffix=""
        case $target in
            *windows*) suffix=".exe" ;;
        esac

        local bin_name="binfiddle${suffix}"
        local archive_name="binfiddle-v${VERSION}-${target}"

        echo "Creating archive for $target..."

        case $target in
            *windows*)
                7z a "${archive_name}.zip" "${bin_name}-${target}" "${bin_name}-${target}.sha256"
                ;;
            *)
                tar czf "${archive_name}.tar.gz" "${bin_name}-${target}" "${bin_name}-${target}.sha256"
                ;;
        esac
    done

    popd >/dev/null
}

create_archives

echo "Build complete! Archives available in ${OUTPUT_DIR}"
