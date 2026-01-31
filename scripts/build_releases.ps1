<#
.SYNOPSIS
    Binfiddle Release Build Script (PowerShell)

.DESCRIPTION
    Cross-platform build script for creating release binaries of binfiddle.
    Supports native builds and cross-compilation for multiple targets.

.PARAMETER Native
    Build only for the current platform

.PARAMETER Target
    Build for a specific target triple

.PARAMETER Setup
    Install build dependencies (requires admin privileges)

.PARAMETER Clean
    Clean build artifacts

.PARAMETER List
    List all supported targets

.PARAMETER SkipMacOS
    Skip macOS targets

.PARAMETER SkipLinux
    Skip Linux targets

.PARAMETER SkipArchive
    Skip archive creation

.PARAMETER UseCross
    Use 'cross' tool instead of cargo for cross-compilation

.EXAMPLE
    .\build_releases.ps1
    Build all available targets

.EXAMPLE
    .\build_releases.ps1 -Native
    Build only for current platform

.EXAMPLE
    .\build_releases.ps1 -Target "x86_64-pc-windows-gnu"
    Build specific target

.EXAMPLE
    .\build_releases.ps1 -SkipMacOS -SkipLinux
    Build only Windows targets
#>

[CmdletBinding()]
param(
    [switch]$Native,
    [string]$Target,
    [switch]$Setup,
    [switch]$Clean,
    [switch]$List,
    [switch]$SkipMacOS,
    [switch]$SkipLinux,
    [switch]$SkipArchive,
    [switch]$UseCross,
    [switch]$Help
)

#Requires -Version 5.1

# Stop on errors
$ErrorActionPreference = "Stop"

# Configuration
$ProjectName = "binfiddle"
$BuildFlags = "--release"

# All supported targets
$AllTargets = @(
    "x86_64-unknown-linux-gnu",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-gnu",
    "x86_64-pc-windows-gnu",
    "x86_64-pc-windows-msvc",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin"
)

# Global tracking arrays
$script:BuiltTargets = @()
$script:FailedTargets = @()

#region Logging Functions

function Write-Info {
    param([string]$Message)
    Write-Host "[INFO] $Message" -ForegroundColor Blue
}

function Write-Success {
    param([string]$Message)
    Write-Host "[SUCCESS] $Message" -ForegroundColor Green
}

function Write-Warn {
    param([string]$Message)
    Write-Host "[WARN] $Message" -ForegroundColor Yellow
}

function Write-Err {
    param([string]$Message)
    Write-Host "[ERROR] $Message" -ForegroundColor Red
}

#endregion

#region Helper Functions

function Test-CommandExists {
    param([string]$Command)

    try {
        if (Get-Command $Command -ErrorAction SilentlyContinue) {
            return $true
        }
        return $false
    }
    catch {
        return $false
    }
}

function Get-Version {
    if (-not (Test-Path "Cargo.toml")) {
        Write-Err "Cargo.toml not found. Must be run from project root."
        exit 1
    }

    $cargoToml = Get-Content "Cargo.toml" -Raw
    if ($cargoToml -match 'version\s*=\s*"([^"]+)"') {
        return $Matches[1]
    }

    Write-Err "Could not determine version from Cargo.toml"
    exit 1
}

function Get-NativeTarget {
    $arch = [System.Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture
    $os = [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform

    if ($os([System.Runtime.InteropServices.OSPlatform]::Windows)) {
        switch ($arch) {
            "X64" { return "x86_64-pc-windows-msvc" }
            "Arm64" { return "aarch64-pc-windows-msvc" }
            default { return "unknown" }
        }
    }
    elseif ($os([System.Runtime.InteropServices.OSPlatform]::Linux)) {
        switch ($arch) {
            "X64" { return "x86_64-unknown-linux-gnu" }
            "Arm64" { return "aarch64-unknown-linux-gnu" }
            default { return "unknown" }
        }
    }
    elseif ($os([System.Runtime.InteropServices.OSPlatform]::OSX)) {
        switch ($arch) {
            "X64" { return "x86_64-apple-darwin" }
            "Arm64" { return "aarch64-apple-darwin" }
            default { return "unknown" }
        }
    }

    return "unknown"
}

function Test-CanBuildTarget {
    param([string]$TargetTriple)

    $nativeTarget = Get-NativeTarget

    switch -Regex ($TargetTriple) {
        ".*-apple-darwin" {
            # macOS targets require native macOS or osxcross
            if ([System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform([System.Runtime.InteropServices.OSPlatform]::OSX)) {
                return $true
            }
            # Check for osxcross (unlikely on Windows)
            return $false
        }
        ".*-windows-gnu" {
            # Windows GNU targets require mingw-w64
            return Test-CommandExists "x86_64-w64-mingw32-gcc"
        }
        ".*-windows-msvc" {
            # MSVC targets require Visual Studio/Build Tools
            return [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform([System.Runtime.InteropServices.OSPlatform]::Windows)
        }
        "aarch64-unknown-linux-gnu" {
            # ARM64 Linux requires cross-compiler or native
            if ($nativeTarget -eq $TargetTriple) {
                return $true
            }
            return Test-CommandExists "aarch64-linux-gnu-gcc"
        }
        "x86_64-unknown-linux-musl" {
            # musl requires musl-gcc (unlikely on Windows)
            return Test-CommandExists "musl-gcc"
        }
        "x86_64-unknown-linux-gnu" {
            # Native Linux or cross-compilation environment
            if ([System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform([System.Runtime.InteropServices.OSPlatform]::Linux)) {
                return $true
            }
            # Check for cross-compilation support
            return Test-CommandExists "x86_64-linux-gnu-gcc"
        }
        default {
            return $true
        }
    }
}

function Get-AvailableTargets {
    $available = @()

    foreach ($target in $AllTargets) {
        # Skip based on flags
        if ($SkipMacOS -and $target -like "*-apple-darwin") {
            continue
        }
        if ($SkipLinux -and $target -like "*-linux-*") {
            continue
        }

        if (Test-CanBuildTarget $target) {
            $available += $target
        }
        else {
            Write-Warn "Skipping $target (toolchain not available)"
        }
    }

    return $available
}

function Install-RustTargets {
    param([string[]]$Targets)

    if (-not (Test-CommandExists "rustup")) {
        Write-Warn "rustup not found, skipping target installation"
        return
    }

    Write-Info "Installing Rust targets..."

    foreach ($target in $Targets) {
        $installed = rustup target list --installed
        if ($installed -notcontains $target) {
            Write-Info "Adding target: $target"
            try {
                rustup target add $target 2>&1 | Out-Null
            }
            catch {
                Write-Warn "Failed to add target $target"
            }
        }
    }
}

function Set-CargoEnvironment {
    param([string]$TargetTriple)

    # Clear previous configuration
    Remove-Item Env:CARGO_TARGET_* -ErrorAction SilentlyContinue
    Remove-Item Env:CC -ErrorAction SilentlyContinue
    Remove-Item Env:AR -ErrorAction SilentlyContinue

    switch -Regex ($TargetTriple) {
        "x86_64-pc-windows-gnu" {
            if (Test-CommandExists "x86_64-w64-mingw32-gcc") {
                $env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = "x86_64-w64-mingw32-gcc"
                $env:CC = "x86_64-w64-mingw32-gcc"
                $env:AR = "x86_64-w64-mingw32-ar"
            }
        }
        "aarch64-unknown-linux-gnu" {
            $nativeTarget = Get-NativeTarget
            if ($nativeTarget -ne $TargetTriple) {
                $env:CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER = "aarch64-linux-gnu-gcc"
                $env:CC = "aarch64-linux-gnu-gcc"
                $env:AR = "aarch64-linux-gnu-ar"
            }
        }
        "x86_64-unknown-linux-gnu" {
            if (-not [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform([System.Runtime.InteropServices.OSPlatform]::Linux)) {
                $env:CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER = "x86_64-linux-gnu-gcc"
                $env:CC = "x86_64-linux-gnu-gcc"
                $env:AR = "x86_64-linux-gnu-ar"
            }
        }
    }
}

function Build-Target {
    param([string]$TargetTriple)

    $suffix = ""
    if ($TargetTriple -like "*-windows-*") {
        $suffix = ".exe"
    }

    Write-Info "Building for ${TargetTriple}..."

    Set-CargoEnvironment $TargetTriple

    try {
        if ($UseCross -and (Test-CommandExists "cross")) {
            cross build $BuildFlags --target $TargetTriple 2>&1 | Out-Host
        }
        else {
            cargo build $BuildFlags --target $TargetTriple 2>&1 | Out-Host
        }
    }
    catch {
        Write-Err "Build failed for ${TargetTriple}: $_"
        return $false
    }

    # Locate and copy binary
    $version = Get-Version
    $outputDir = "releases\v$version"

    if (-not (Test-Path $outputDir)) {
        New-Item -ItemType Directory -Path $outputDir -Force | Out-Null
    }

    $srcPath = "target\$TargetTriple\release\$ProjectName$suffix"
    $dstPath = "$outputDir\$ProjectName$suffix-$TargetTriple"

    if (-not (Test-Path $srcPath)) {
        Write-Err "Binary not found: $srcPath"
        return $false
    }

    Copy-Item $srcPath $dstPath -Force

    # Create checksum
    try {
        $hash = (Get-FileHash $dstPath -Algorithm SHA256).Hash
        "$hash  $(Split-Path $dstPath -Leaf)" | Out-File -FilePath "$dstPath.sha256" -Encoding ASCII
    }
    catch {
        Write-Warn "Failed to create checksum for $TargetTriple"
    }

    $size = (Get-Item $dstPath).Length / 1MB
    Write-Success "Built $TargetTriple ($([Math]::Round($size, 1))M)"

    return $true
}

function New-Archives {
    if ($SkipArchive) {
        Write-Info "Skipping archive creation"
        return
    }

    Write-Info "Creating release archives..."

    $version = Get-Version
    $outputDir = "releases\v$version"

    Push-Location $outputDir

    try {
        foreach ($target in $script:BuiltTargets) {
            $suffix = ""
            if ($target -like "*-windows-*") {
                $suffix = ".exe"
            }

            $binName = "$ProjectName$suffix-$target"
            $archiveName = "$ProjectName-v$version-$target"

            if (-not (Test-Path $binName)) {
                Write-Warn "Binary not found for $target, skipping archive"
                continue
            }

            Write-Info "Creating archive: $archiveName"

            # Include checksum if it exists
            $filesToArchive = @($binName)
            if (Test-Path "$binName.sha256") {
                $filesToArchive += "$binName.sha256"
            }

            if ($target -like "*-windows-*") {
                # Create ZIP for Windows targets
                Compress-Archive -Path $filesToArchive -DestinationPath "$archiveName.zip" -Force
            }
            else {
                # Create tar.gz for other targets
                if (Test-CommandExists "tar") {
                    tar -czf "$archiveName.tar.gz" $filesToArchive 2>$null
                }
                else {
                    # Fallback to ZIP if tar not available
                    Compress-Archive -Path $filesToArchive -DestinationPath "$archiveName.zip" -Force
                }
            }
        }
    }
    finally {
        Pop-Location
    }

    Write-Success "Archives created in $outputDir"
}

function Show-Summary {
    $version = Get-Version

    Write-Host ""
    Write-Host "=========================================="
    Write-Host "  Binfiddle v$version Build Summary"
    Write-Host "=========================================="
    Write-Host ""

    if ($script:BuiltTargets.Count -gt 0) {
        Write-Success "Successfully built $($script:BuiltTargets.Count) target(s):"
        foreach ($target in $script:BuiltTargets) {
            $suffix = ""
            if ($target -like "*-windows-*") {
                $suffix = ".exe"
            }
            $binPath = "releases\v$version\$ProjectName$suffix-$target"
            if (Test-Path $binPath) {
                $size = (Get-Item $binPath).Length / 1MB
                Write-Host "  ✓ $target ($([Math]::Round($size, 1))M)" -ForegroundColor Green
            }
        }
    }

    if ($script:FailedTargets.Count -gt 0) {
        Write-Host ""
        Write-Err "Failed to build $($script:FailedTargets.Count) target(s):"
        foreach ($target in $script:FailedTargets) {
            Write-Host "  ✗ $target" -ForegroundColor Red
        }
    }

    Write-Host ""
    Write-Host "Output directory: releases\v$version"
    Write-Host ""
}

function Invoke-Clean {
    Write-Info "Cleaning build artifacts..."

    if (Test-Path "target") {
        Remove-Item -Recurse -Force "target"
    }

    if (Test-Path "releases") {
        Remove-Item -Recurse -Force "releases"
    }

    Write-Success "Clean complete"
}

function Show-Help {
    $nativeTarget = Get-NativeTarget

    Write-Host @"
Binfiddle Release Build Script (PowerShell)

Usage: .\build_releases.ps1 [OPTIONS]

Options:
    -Native         Build only for current platform ($nativeTarget)
    -Target <T>     Build for specific target
    -Setup          Install build dependencies (requires admin)
    -Clean          Clean build artifacts
    -List           List all supported targets
    -Help           Show this help message

Switches:
    -SkipMacOS      Skip macOS targets
    -SkipLinux      Skip Linux targets
    -SkipArchive    Skip archive creation
    -UseCross       Use 'cross' tool for cross-compilation

Supported Targets:
"@

    foreach ($target in $AllTargets) {
        $status = if (Test-CanBuildTarget $target) { "available" } else { "unavailable" }
        $native = if ($target -eq $nativeTarget) { " [native]" } else { "" }
        Write-Host "    $target [$status]$native"
    }

    Write-Host @"

Examples:
    .\build_releases.ps1                    # Build all available targets
    .\build_releases.ps1 -Native            # Build only for current platform
    .\build_releases.ps1 -Target x86_64-pc-windows-msvc
    .\build_releases.ps1 -SkipMacOS         # Build all except macOS
    .\build_releases.ps1 -UseCross          # Use 'cross' for cross-compilation

"@
}

function Invoke-Setup {
    Write-Info "Setting up build dependencies..."

    if (-not ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        Write-Warn "Setup requires administrator privileges"
        Write-Warn "Please run PowerShell as Administrator and try again"
        exit 1
    }

    # Check for Chocolatey (Windows package manager)
    if (-not (Test-CommandExists "choco")) {
        Write-Info "Chocolatey not found. Installing..."
        Set-ExecutionPolicy Bypass -Scope Process -Force
        [System.Net.ServicePointManager]::SecurityProtocol = [System.Net.ServicePointManager]::SecurityProtocol -bor 3072
        Invoke-Expression ((New-Object System.Net.WebClient).DownloadString('https://community.chocolatey.org/install.ps1'))
    }

    # Install build tools
    Write-Info "Installing build dependencies via Chocolatey..."

    $packages = @(
        "rust",
        "mingw",
        "7zip"
    )

    foreach ($pkg in $packages) {
        try {
            choco install $pkg -y
        }
        catch {
            Write-Warn "Failed to install $pkg"
        }
    }

    # Install cross if not present
    if (-not (Test-CommandExists "cross")) {
        Write-Info "Installing 'cross' for cross-compilation..."
        try {
            cargo install cross --git https://github.com/cross-rs/cross
        }
        catch {
            Write-Warn "Failed to install 'cross'"
        }
    }

    Write-Success "Dependency setup complete"
}

#endregion

#region Main

function Main {
    # Handle special commands
    if ($Help) {
        Show-Help
        exit 0
    }

    if ($Clean) {
        Invoke-Clean
        exit 0
    }

    if ($Setup) {
        Invoke-Setup
        exit 0
    }

    if ($List) {
        Write-Host "Supported targets:"
        foreach ($t in $AllTargets) {
            Write-Host "  $t"
        }
        exit 0
    }

    # Ensure we're in project root
    if (-not (Test-Path "Cargo.toml")) {
        Write-Err "Must be run from project root (Cargo.toml not found)"
        exit 1
    }

    # Determine targets to build
    $targetsToBuild = @()

    if ($Target) {
        $targetsToBuild = @($Target)
    }
    elseif ($Native) {
        $targetsToBuild = @(Get-NativeTarget)
    }
    else {
        $targetsToBuild = Get-AvailableTargets
    }

    if ($targetsToBuild.Count -eq 0) {
        Write-Err "No targets available to build"
        exit 1
    }

    $version = Get-Version
    Write-Info "Building binfiddle v$version"
    Write-Info "Targets: $($targetsToBuild -join ', ')"

    # Install Rust targets
    Install-RustTargets $targetsToBuild

    # Build each target
    foreach ($target in $targetsToBuild) {
        if (Build-Target $target) {
            $script:BuiltTargets += $target
        }
        else {
            $script:FailedTargets += $target
        }
    }

    # Create archives
    if ($script:BuiltTargets.Count -gt 0) {
        New-Archives
    }

    # Show summary
    Show-Summary

    # Exit with error if any builds failed
    if ($script:FailedTargets.Count -gt 0) {
        exit 1
    }
}

# Execute main
Main

#endregion
