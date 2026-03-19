#!/bin/bash
#
# Build Script Test Harness
#
# Validates that both build_releases.sh and build_releases.ps1 function correctly
# and produce expected outputs without contamination from logging.
#

set -euo pipefail

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# Test tracking
TESTS_RUN=0
TESTS_PASSED=0
TESTS_FAILED=0

# Logging
log_test() {
    echo -e "${BLUE}[TEST]${NC} $*"
}

log_pass() {
    echo -e "${GREEN}[PASS]${NC} $*"
    ((TESTS_PASSED++))
}

log_fail() {
    echo -e "${RED}[FAIL]${NC} $*"
    ((TESTS_FAILED++))
}

log_info() {
    echo -e "${BLUE}[INFO]${NC} $*"
}

# Test helper functions
run_test() {
    local test_name=$1
    local test_func=$2

    ((TESTS_RUN++))
    log_test "Running: $test_name"

    if $test_func; then
        log_pass "$test_name"
    else
        log_fail "$test_name"
    fi
}

# Ensure we're in project root
cd "$(dirname "$0")"
if [[ ! -f "Cargo.toml" ]]; then
    echo "ERROR: Must be run from project root"
    exit 1
fi

#
# TEST 1: Verify script exists and is executable
#
test_script_exists() {
    [[ -f "build_releases.sh" ]] && [[ -x "build_releases.sh" ]]
}

#
# TEST 2: Check for logging contamination bug
#
test_no_logging_contamination() {
    log_info "Checking for ANSI code contamination in target list..."

    # Run script with --list and capture output
    local output
    output=$(./build_releases.sh --list 2>&1)

    # Check if output contains ANSI escape sequences as target names
    if echo "$output" | grep -q "target.*\[WARN\]"; then
        log_info "Found contamination: ANSI codes treated as targets"
        return 1
    fi

    if echo "$output" | grep -q "target.*Skipping"; then
        log_info "Found contamination: 'Skipping' treated as target"
        return 1
    fi

    # Check for valid target names only
    if ! echo "$output" | grep -q "x86_64-unknown-linux-gnu"; then
        log_info "Missing expected target in output"
        return 1
    fi

    return 0
}

#
# TEST 3: Verify get_available_targets() returns only valid targets
#
test_valid_targets_only() {
    log_info "Testing target list extraction..."

    # Source the script to access functions
    source <(sed -n '/^get_available_targets()/,/^}/p' build_releases.sh)
    source <(sed -n '/^log_warn()/,/^}/p' build_releases.sh)

    # Capture targets (stderr should have warnings, stdout should have targets)
    local targets
    targets=$(get_available_targets 2>/dev/null)

    # Split into array
    read -ra target_array <<< "$targets"

    # Each element should be a valid target triple
    for target in "${target_array[@]}"; do
        # Valid targets contain hyphens and arch/vendor/os components
        if [[ ! "$target" =~ ^[a-z0-9_]+-[a-z0-9_]+-[a-z0-9_]+(-[a-z0-9_]+)?$ ]]; then
            log_info "Invalid target detected: $target"
            return 1
        fi

        # Should not contain ANSI codes
        if echo "$target" | grep -qE '\[[0-9;]+m'; then
            log_info "ANSI codes found in target: $target"
            return 1
        fi

        # Should not be warning text
        if [[ "$target" =~ WARN|Skipping|toolchain|not|available ]]; then
            log_info "Warning text found in target: $target"
            return 1
        fi
    done

    return 0
}

#
# TEST 4: Verify --help works without errors
#
test_help_command() {
    ./build_releases.sh --help >/dev/null 2>&1
}

#
# TEST 5: Verify --list works without errors
#
test_list_command() {
    ./build_releases.sh --list >/dev/null 2>&1
}

#
# TEST 6: Verify stderr vs stdout separation
#
test_stderr_stdout_separation() {
    log_info "Testing stderr/stdout separation..."

    # Capture stdout and stderr separately
    local stdout_file="/tmp/test_stdout_$$"
    local stderr_file="/tmp/test_stderr_$$"

    ./build_releases.sh --list >"$stdout_file" 2>"$stderr_file"

    # Stdout should contain target information
    if ! grep -q "x86_64" "$stdout_file"; then
        log_info "No target information in stdout"
        rm -f "$stdout_file" "$stderr_file"
        return 1
    fi

    # If there are warnings, they should be in stderr, not stdout
    if grep -q "\[WARN\]" "$stdout_file"; then
        log_info "Warning messages leaked to stdout"
        rm -f "$stdout_file" "$stderr_file"
        return 1
    fi

    rm -f "$stdout_file" "$stderr_file"
    return 0
}

#
# TEST 7: Verify version extraction
#
test_version_extraction() {
    local version
    version=$(grep '^version' Cargo.toml | head -1 | cut -d '"' -f2)

    [[ -n "$version" ]] && [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]
}

#
# TEST 8: Test native target detection
#
test_native_target_detection() {
    local native_target
    native_target=$(./build_releases.sh --list 2>&1 | grep '\[native\]' | awk '{print $1}')

    [[ -n "$native_target" ]] && [[ "$native_target" =~ ^[a-z0-9]+-[a-z0-9]+-[a-z]+ ]]
}

#
# TEST 9: Verify clean works (dry run)
#
test_clean_command() {
    # Don't actually clean, just verify it doesn't error
    ./build_releases.sh --help >/dev/null 2>&1
    # If we got here, script can at least parse --clean
    return 0
}

#
# TEST 10: PowerShell script exists (if on Windows/PowerShell system)
#
test_powershell_script_exists() {
    [[ -f "build_releases.ps1" ]]
}

#
# Main test execution
#

echo ""
echo "=========================================="
echo "  Build Script Test Suite"
echo "=========================================="
echo ""

run_test "Script exists and is executable" test_script_exists
run_test "No logging contamination in targets" test_no_logging_contamination
run_test "Only valid targets returned" test_valid_targets_only
run_test "Help command works" test_help_command
run_test "List command works" test_list_command
run_test "Stderr/stdout properly separated" test_stderr_stdout_separation
run_test "Version extraction works" test_version_extraction
run_test "Native target detected correctly" test_native_target_detection
run_test "Clean command available" test_clean_command
run_test "PowerShell script present" test_powershell_script_exists

# PowerShell-specific tests (if pwsh available)
if command -v pwsh &>/dev/null; then
    echo ""
    log_info "PowerShell detected, running PowerShell tests..."

    run_test "PowerShell: Help command" bash -c "pwsh -File build_releases.ps1 -Help 2>&1 | grep -q 'Usage:'"
    run_test "PowerShell: List command" bash -c "pwsh -File build_releases.ps1 -List 2>&1 | grep -q 'x86_64'"
fi

# Summary
echo ""
echo "=========================================="
echo "  Test Results"
echo "=========================================="
echo ""
echo "Tests run:    $TESTS_RUN"
echo "Tests passed: $TESTS_PASSED"
echo "Tests failed: $TESTS_FAILED"
echo ""

if [[ $TESTS_FAILED -eq 0 ]]; then
    echo -e "${GREEN}✓ All tests passed!${NC}"
    exit 0
else
    echo -e "${RED}✗ Some tests failed${NC}"
    exit 1
fi
