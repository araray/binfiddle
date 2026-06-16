//! Process memory access helpers (Linux-only).
//!
//! This module provides a minimal, read-mostly interface to `/proc/<pid>/mem`
//! and `/proc/<pid>/maps`. It is intentionally scoped to Linux and focuses on
//! same-user process inspection.

use crate::{BinfiddleError, Result};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

/// Describes a single memory region parsed from `/proc/<pid>/maps`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryRegion {
    pub start: u64,
    pub end: u64,
    pub perms: String,
    pub offset: u64,
    pub dev: String,
    pub inode: u64,
    pub pathname: Option<String>,
}

impl MemoryRegion {
    /// Returns true if the region contains the given address.
    pub fn contains(&self, address: u64) -> bool {
        address >= self.start && address < self.end
    }

    /// Returns true if the region is writable.
    pub fn is_writable(&self) -> bool {
        self.perms.contains('w')
    }

    /// Returns true if the region is readable.
    pub fn is_readable(&self) -> bool {
        self.perms.contains('r')
    }
}

/// Returns the path to a process's `/proc/<pid>/mem` file.
///
/// Uses `/proc/self/mem` when the pid is the current process so tests and
/// self-introspection work without caring about the actual pid.
fn proc_mem_path(pid: u32) -> PathBuf {
    if pid == 0 || pid == std::process::id() {
        PathBuf::from("/proc/self/mem")
    } else {
        PathBuf::from(format!("/proc/{}/mem", pid))
    }
}

/// Returns the path to a process's `/proc/<pid>/maps` file.
fn proc_maps_path(pid: u32) -> PathBuf {
    if pid == 0 || pid == std::process::id() {
        PathBuf::from("/proc/self/maps")
    } else {
        PathBuf::from(format!("/proc/{}/maps", pid))
    }
}

/// Reads `size` bytes from `pid`'s memory starting at `address`.
pub fn read_process_memory(pid: u32, address: u64, size: u64) -> Result<Vec<u8>> {
    if size > usize::MAX as u64 {
        return Err(BinfiddleError::ProcessMemoryError(
            "Requested size exceeds addressable memory".to_string(),
        ));
    }
    let size = size as usize;

    let mut file = File::open(proc_mem_path(pid)).map_err(|e| {
        BinfiddleError::ProcessMemoryError(format!(
            "Failed to open /proc/{}/mem: {}",
            pid_label(pid),
            e
        ))
    })?;

    file.seek(SeekFrom::Start(address)).map_err(|e| {
        BinfiddleError::ProcessMemoryError(format!(
            "Failed to seek to address 0x{:x} in process {}: {}",
            address,
            pid_label(pid),
            e
        ))
    })?;

    let mut buffer = vec![0u8; size];
    let mut total_read = 0usize;

    while total_read < size {
        match file.read(&mut buffer[total_read..]) {
            Ok(0) => {
                return Err(BinfiddleError::ProcessMemoryError(format!(
                    "Short read while reading process {} memory at 0x{:x}: expected {}, got {}",
                    pid_label(pid),
                    address,
                    size,
                    total_read
                )));
            }
            Ok(n) => total_read += n,
            Err(e) => {
                return Err(BinfiddleError::ProcessMemoryError(format!(
                    "Failed to read process {} memory at 0x{:x}: {}",
                    pid_label(pid),
                    address + total_read as u64,
                    e
                )));
            }
        }
    }

    Ok(buffer)
}

/// Writes `data` to `pid`'s memory starting at `address`.
///
/// - For the current process, `/proc/self/mem` is used.
/// - For other processes, `process_vm_writev` is used and the target region
///   must already be writable, unless `force_writable` is set.
/// - When `force_writable` is set, read-only pages are temporarily made
///   writable via `mprotect` (self) or ptrace syscall injection (cross-process),
///   then restored.
pub fn write_process_memory(
    pid: u32,
    address: u64,
    data: &[u8],
    force_writable: bool,
) -> Result<()> {
    if pid == 0 || pid == std::process::id() {
        if force_writable {
            force_write_self_memory(address, data)
        } else {
            write_self_memory(address, data)
        }
    } else {
        write_cross_process_memory(pid, address, data, force_writable)
    }
}

fn write_self_memory(address: u64, data: &[u8]) -> Result<()> {
    let mut file = File::options()
        .read(true)
        .write(true)
        .open("/proc/self/mem")
        .map_err(|e| {
            BinfiddleError::ProcessMemoryError(format!(
                "Failed to open /proc/self/mem for writing: {}",
                e
            ))
        })?;

    file.seek(SeekFrom::Start(address)).map_err(|e| {
        BinfiddleError::ProcessMemoryError(format!(
            "Failed to seek to address 0x{:x} for writing: {}",
            address, e
        ))
    })?;

    file.write_all(data).map_err(|e| {
        BinfiddleError::ProcessMemoryError(format!(
            "Failed to write process memory at 0x{:x}: {}",
            address, e
        ))
    })?;

    Ok(())
}

fn force_write_self_memory(address: u64, data: &[u8]) -> Result<()> {
    let regions = parse_maps(0)?;
    let region = find_region(&regions, address).ok_or_else(|| {
        BinfiddleError::ProcessMemoryError(format!(
            "Address 0x{:x} is not in any mapped region of the current process",
            address
        ))
    })?;

    let original_prot = prot_from_perms(&region.perms);

    let page_size = page_size();
    let protect_start = region.start & !(page_size - 1);
    let protect_end = (region.end + page_size - 1) & !(page_size - 1);
    let protect_len = protect_end - protect_start;

    let result = unsafe {
        if libc::mprotect(
            protect_start as *mut libc::c_void,
            protect_len as usize,
            libc::PROT_READ | libc::PROT_WRITE,
        ) != 0
        {
            Err(BinfiddleError::ProcessMemoryError(format!(
                "mprotect failed for region 0x{:x}-0x{:x}: {}",
                protect_start,
                protect_end,
                std::io::Error::last_os_error()
            )))
        } else {
            write_self_memory(address, data)
        }
    };

    unsafe {
        let _ = libc::mprotect(
            protect_start as *mut libc::c_void,
            protect_len as usize,
            original_prot,
        );
    }

    result
}

fn write_cross_process_memory(
    pid: u32,
    address: u64,
    data: &[u8],
    force_writable: bool,
) -> Result<()> {
    let regions = parse_maps(pid)?;
    let region = find_region(&regions, address).ok_or_else(|| {
        BinfiddleError::ProcessMemoryError(format!(
            "Address 0x{:x} is not in any mapped region of process {}",
            address, pid
        ))
    })?;

    if !region.is_writable() && !force_writable {
        return Err(BinfiddleError::ProcessMemoryError(format!(
            "Memory region 0x{:x}-0x{:x} in process {} is not writable (use --force-writable to override)",
            region.start, region.end, pid
        )));
    }

    if force_writable {
        force_write_cross_process_memory(pid, address, data, region)?;
    } else {
        process_vm_writev_data(pid, address, data)?;
    }

    Ok(())
}

fn process_vm_writev_data(pid: u32, address: u64, data: &[u8]) -> Result<()> {
    let local_iov = libc::iovec {
        iov_base: data.as_ptr() as *mut libc::c_void,
        iov_len: data.len(),
    };
    let remote_iov = libc::iovec {
        iov_base: address as *mut libc::c_void,
        iov_len: data.len(),
    };

    let ret =
        unsafe { libc::process_vm_writev(pid as libc::pid_t, &local_iov, 1, &remote_iov, 1, 0) };

    if ret < 0 {
        let err = std::io::Error::last_os_error();
        return Err(BinfiddleError::ProcessMemoryError(format!(
            "process_vm_writev failed for pid {}: {} (ensure ptrace access is permitted)",
            pid, err
        )));
    }

    if ret as usize != data.len() {
        return Err(BinfiddleError::ProcessMemoryError(
            "Short write while writing process memory".to_string(),
        ));
    }

    Ok(())
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn force_write_cross_process_memory(
    pid: u32,
    address: u64,
    data: &[u8],
    region: &MemoryRegion,
) -> Result<()> {
    use nix::unistd::Pid;

    let target = Pid::from_raw(pid as i32);
    let original_prot = prot_from_perms(&region.perms);

    // Temporarily change page protection to writable via ptrace-injected mprotect.
    ptrace_inject_mprotect(target, region, libc::PROT_READ | libc::PROT_WRITE)?;

    let result = process_vm_writev_data(pid, address, data);

    // Best-effort restoration of original protection. We ignore restoration
    // failures so the write result is still reported.
    let _ = ptrace_inject_mprotect(target, region, original_prot);

    result
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
fn force_write_cross_process_memory(
    _pid: u32,
    _address: u64,
    _data: &[u8],
    _region: &MemoryRegion,
) -> Result<()> {
    Err(BinfiddleError::UnsupportedOperation(
        "--force-writable for cross-process writes is only supported on Linux x86_64".to_string(),
    ))
}

/// Uses ptrace to inject an `mprotect` syscall into a stopped target process.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn ptrace_inject_mprotect(
    target: nix::unistd::Pid,
    region: &MemoryRegion,
    prot: libc::c_int,
) -> Result<()> {
    use nix::sys::ptrace;
    use nix::sys::wait::{waitpid, WaitStatus};

    ptrace::attach(target).map_err(|e| {
        BinfiddleError::ProcessMemoryError(format!(
            "Failed to attach to process {}: {} (ensure ptrace access is permitted)",
            target, e
        ))
    })?;

    let attach_result = match waitpid(target, None) {
        Ok(WaitStatus::Stopped(_, _)) => Ok(()),
        Ok(other) => Err(BinfiddleError::ProcessMemoryError(format!(
            "Unexpected wait status while attaching to {}: {:?}",
            target, other
        ))),
        Err(e) => Err(BinfiddleError::ProcessMemoryError(format!(
            "waitpid failed after attach to {}: {}",
            target, e
        ))),
    };

    if let Err(e) = attach_result {
        let _ = ptrace::detach(target, None);
        return Err(e);
    }

    let inject_result = (|| {
        let mut regs = ptrace::getregs(target).map_err(|e| {
            BinfiddleError::ProcessMemoryError(format!(
                "Failed to read registers of {}: {}",
                target, e
            ))
        })?;
        let saved_regs = regs;
        let rip = regs.rip;

        let page_size = page_size();
        let protect_start = region.start & !(page_size - 1);
        let protect_end = (region.end + page_size - 1) & !(page_size - 1);
        let protect_len = protect_end - protect_start;

        // Save the two words at the injection point so we can restore them.
        let original_word1 = ptrace::read(target, rip as *mut libc::c_void).map_err(|e| {
            BinfiddleError::ProcessMemoryError(format!(
                "Failed to read injection point from {}: {}",
                target, e
            ))
        })? as u64;
        let original_word2 = ptrace::read(target, (rip + 8) as *mut libc::c_void).map_err(|e| {
            BinfiddleError::ProcessMemoryError(format!(
                "Failed to read injection point from {}: {}",
                target, e
            ))
        })? as u64;

        // Inject: syscall (0x0f 0x05); int3 (0xcc); padding
        let injected_word1 = u64::from_le_bytes([0x0f, 0x05, 0xcc, 0x90, 0x90, 0x90, 0x90, 0x90]);
        ptrace::write(target, rip as *mut libc::c_void, injected_word1 as i64).map_err(|e| {
            BinfiddleError::ProcessMemoryError(format!(
                "Failed to write injected syscall to {}: {}",
                target, e
            ))
        })?;

        // Set up mprotect syscall arguments (x86_64 syscall ABI).
        regs.rax = libc::SYS_mprotect as u64;
        regs.rdi = protect_start;
        regs.rsi = protect_len;
        regs.rdx = prot as u64;
        regs.rip = rip;

        ptrace::setregs(target, regs).map_err(|e| {
            BinfiddleError::ProcessMemoryError(format!(
                "Failed to set registers of {}: {}",
                target, e
            ))
        })?;

        ptrace::cont(target, None).map_err(|e| {
            BinfiddleError::ProcessMemoryError(format!(
                "Failed to continue {} for mprotect injection: {}",
                target, e
            ))
        })?;

        let wait_result = waitpid(target, None).map_err(|e| {
            BinfiddleError::ProcessMemoryError(format!(
                "waitpid failed during mprotect injection for {}: {}",
                target, e
            ))
        })?;

        if !matches!(
            wait_result,
            WaitStatus::Stopped(_, nix::sys::signal::Signal::SIGTRAP)
        ) {
            return Err(BinfiddleError::ProcessMemoryError(format!(
                "Unexpected wait status during mprotect injection for {}: {:?}",
                target, wait_result
            )));
        }

        let post_regs = ptrace::getregs(target).map_err(|e| {
            BinfiddleError::ProcessMemoryError(format!(
                "Failed to read post-syscall registers of {}: {}",
                target, e
            ))
        })?;

        if post_regs.rax as i64 != 0 {
            return Err(BinfiddleError::ProcessMemoryError(format!(
                "mprotect syscall in process {} returned error {}",
                target, post_regs.rax as i64
            )));
        }

        // Restore original code words.
        ptrace::write(target, rip as *mut libc::c_void, original_word1 as i64).map_err(|e| {
            BinfiddleError::ProcessMemoryError(format!(
                "Failed to restore first word at {}: {}",
                target, e
            ))
        })?;
        // Second word was never modified, but restore for completeness.
        let _ = ptrace::write(
            target,
            (rip + 8) as *mut libc::c_void,
            original_word2 as i64,
        );

        // Restore original registers.
        ptrace::setregs(target, saved_regs).map_err(|e| {
            BinfiddleError::ProcessMemoryError(format!(
                "Failed to restore registers of {}: {}",
                target, e
            ))
        })?;

        Ok(())
    })();

    let _ = ptrace::detach(target, None);
    inject_result
}

fn page_size() -> u64 {
    // Safe: sysconf is always successful for _SC_PAGE_SIZE on Linux.
    unsafe { libc::sysconf(libc::_SC_PAGE_SIZE) as u64 }
}

fn prot_from_perms(perms: &str) -> libc::c_int {
    let mut prot = libc::PROT_NONE;
    if perms.contains('r') {
        prot |= libc::PROT_READ;
    }
    if perms.contains('w') {
        prot |= libc::PROT_WRITE;
    }
    if perms.contains('x') {
        prot |= libc::PROT_EXEC;
    }
    prot
}

/// Parses `/proc/<pid>/maps` into a list of memory regions.
pub fn parse_maps(pid: u32) -> Result<Vec<MemoryRegion>> {
    let content = std::fs::read_to_string(proc_maps_path(pid)).map_err(|e| {
        BinfiddleError::ProcessMemoryError(format!(
            "Failed to read /proc/{}/maps: {}",
            pid_label(pid),
            e
        ))
    })?;

    let mut regions = Vec::new();
    for line in content.lines() {
        if line.is_empty() {
            continue;
        }

        let mut parts = line.splitn(6, ' ');
        let range = parts.next().ok_or_else(|| {
            BinfiddleError::ProcessMemoryError(format!(
                "Malformed /proc/{}/maps line: {}",
                pid_label(pid),
                line
            ))
        })?;

        let (start, end) = parse_range(range).map_err(|e| {
            BinfiddleError::ProcessMemoryError(format!(
                "Failed to parse region range in /proc/{}/maps: {}",
                pid_label(pid),
                e
            ))
        })?;

        let perms = parts.next().unwrap_or("").to_string();
        let offset = parts.next().unwrap_or("0");
        let dev = parts.next().unwrap_or("").to_string();
        let inode = parts.next().unwrap_or("0");

        // Remaining whitespace-separated tokens are the pathname (may contain spaces).
        let pathname = parts
            .next()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let offset = u64::from_str_radix(offset, 16).map_err(|e| {
            BinfiddleError::ProcessMemoryError(format!(
                "Failed to parse offset '{}' in /proc/{}/maps: {}",
                offset,
                pid_label(pid),
                e
            ))
        })?;

        let inode = inode.parse::<u64>().map_err(|e| {
            BinfiddleError::ProcessMemoryError(format!(
                "Failed to parse inode '{}' in /proc/{}/maps: {}",
                inode,
                pid_label(pid),
                e
            ))
        })?;

        regions.push(MemoryRegion {
            start,
            end,
            perms,
            offset,
            dev,
            inode,
            pathname,
        });
    }

    Ok(regions)
}

/// Finds the memory region containing `address`.
pub fn find_region(regions: &[MemoryRegion], address: u64) -> Option<&MemoryRegion> {
    regions.iter().find(|r| r.contains(address))
}

/// Helper to print a list of memory regions in a human-readable table.
pub fn format_regions(regions: &[MemoryRegion]) -> String {
    let mut output = String::new();
    output.push_str("Memory regions:\n");
    output.push_str(&format!(
        "{:<18} {:<18} {:<5} {:<10} {:<6} {:<10} {}\n",
        "Start", "End", "Perms", "Offset", "Dev", "Inode", "Pathname"
    ));

    for r in regions {
        let pathname = r.pathname.as_deref().unwrap_or("");
        output.push_str(&format!(
            "{:<18x} {:<18x} {:<5} {:<10x} {:<6} {:<10} {}\n",
            r.start, r.end, r.perms, r.offset, r.dev, r.inode, pathname
        ));
    }

    output
}

fn parse_range(range: &str) -> Result<(u64, u64)> {
    let (start, end) = range
        .split_once('-')
        .ok_or_else(|| BinfiddleError::Parse(format!("Invalid memory region range: {}", range)))?;

    let start = u64::from_str_radix(start, 16)
        .map_err(|e| BinfiddleError::Parse(format!("Invalid start address '{}': {}", start, e)))?;
    let end = u64::from_str_radix(end, 16)
        .map_err(|e| BinfiddleError::Parse(format!("Invalid end address '{}': {}", end, e)))?;

    Ok((start, end))
}

fn pid_label(pid: u32) -> String {
    if pid == 0 || pid == std::process::id() {
        "self".to_string()
    } else {
        pid.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_maps_sample() {
        let sample = "00400000-00452000 r-xp 00000000 08:02 173521      /usr/bin/dash\n\
                      00452000-00453000 r--p 00052000 08:02 173521      /usr/bin/dash\n\
                      00453000-00454000 rw-p 00053000 08:02 173521      /usr/bin/dash\n";

        let mut regions = Vec::new();
        for line in sample.lines() {
            let mut parts = line.splitn(6, ' ');
            let range = parts.next().unwrap();
            let (start, end) = parse_range(range).unwrap();
            let perms = parts.next().unwrap().to_string();
            let offset = u64::from_str_radix(parts.next().unwrap(), 16).unwrap();
            let dev = parts.next().unwrap().to_string();
            let inode = parts.next().unwrap().parse::<u64>().unwrap();
            let pathname = parts.next().map(|s| s.trim().to_string());
            regions.push(MemoryRegion {
                start,
                end,
                perms,
                offset,
                dev,
                inode,
                pathname,
            });
        }

        assert_eq!(regions.len(), 3);
        assert_eq!(regions[0].perms, "r-xp");
        assert!(!regions[0].is_writable());
        assert_eq!(regions[2].perms, "rw-p");
        assert!(regions[2].is_writable());
    }

    #[test]
    fn test_read_current_process_memory() {
        static TEST_DATA: [u8; 8] = *b"PROCMEM!";
        let address = &TEST_DATA as *const _ as u64;

        let data = read_process_memory(0, address, TEST_DATA.len() as u64).unwrap();
        assert_eq!(data, &TEST_DATA[..]);
    }

    #[test]
    fn test_find_region() {
        let regions = vec![
            MemoryRegion {
                start: 0x1000,
                end: 0x2000,
                perms: "rw-p".to_string(),
                offset: 0,
                dev: "00:00".to_string(),
                inode: 0,
                pathname: None,
            },
            MemoryRegion {
                start: 0x2000,
                end: 0x3000,
                perms: "r-xp".to_string(),
                offset: 0,
                dev: "00:00".to_string(),
                inode: 0,
                pathname: None,
            },
        ];

        assert!(find_region(&regions, 0x1500).unwrap().is_writable());
        assert!(!find_region(&regions, 0x2500).unwrap().is_writable());
        assert!(find_region(&regions, 0x500).is_none());
    }

    #[test]
    fn test_write_current_process_memory() {
        static mut TEST_BUFFER: [u8; 8] = [0u8; 8];
        let address = std::ptr::addr_of!(TEST_BUFFER) as u64;

        // Write via process memory interface.
        write_process_memory(0, address, b"WRITEBUF", false).unwrap();

        unsafe {
            assert_eq!(&TEST_BUFFER[..], b"WRITEBUF");
            // Restore to avoid side effects.
            TEST_BUFFER = [0u8; 8];
        }
    }

    #[test]
    fn test_write_current_pid_uses_cross_process_path() {
        // Using the actual pid exercises the process_vm_writev code path against
        // the current process.
        static mut TEST_BUFFER: [u8; 8] = [0u8; 8];
        let address = std::ptr::addr_of!(TEST_BUFFER) as u64;

        write_process_memory(std::process::id(), address, b"PIDWRITE", false).unwrap();

        unsafe {
            assert_eq!(&TEST_BUFFER[..], b"PIDWRITE");
            TEST_BUFFER = [0u8; 8];
        }
    }

    #[test]
    fn test_readonly_region_detected_in_maps() {
        // String literals live in .rodata, which is mapped read-only.
        static RO_STRING: &str = "READONLY";
        let address = RO_STRING.as_bytes().as_ptr() as u64;

        let regions = parse_maps(0).expect("should parse /proc/self/maps");
        let region = find_region(&regions, address).expect("address should be mapped");
        assert!(
            !region.is_writable(),
            "Expected .rodata region to be non-writable, got perms: {}",
            region.perms
        );
    }

    #[test]
    fn test_force_write_self_memory_changes_readonly_mapping() {
        // Allocate a page, make it read-only, then use --force-writable to
        // temporarily restore write permission and modify it.
        let page_size = page_size() as usize;
        let mapping = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                page_size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert!(!mapping.is_null());

        let address = mapping as u64;
        let original = b"CHANGEME";
        unsafe {
            std::ptr::copy_nonoverlapping(original.as_ptr(), mapping as *mut u8, original.len());
            assert_eq!(libc::mprotect(mapping, page_size, libc::PROT_READ), 0);
        }

        write_process_memory(0, address, b"FORCEWRT", true).unwrap();

        unsafe {
            let bytes = std::slice::from_raw_parts(mapping as *const u8, 8);
            assert_eq!(bytes, b"FORCEWRT");
            assert_eq!(libc::munmap(mapping, page_size), 0);
        }
    }
}
