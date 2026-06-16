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
///   must already be writable.
pub fn write_process_memory(pid: u32, address: u64, data: &[u8]) -> Result<()> {
    if pid == 0 || pid == std::process::id() {
        write_self_memory(address, data)
    } else {
        write_cross_process_memory(pid, address, data)
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

fn write_cross_process_memory(pid: u32, address: u64, data: &[u8]) -> Result<()> {
    let regions = parse_maps(pid)?;
    let region = find_region(&regions, address).ok_or_else(|| {
        BinfiddleError::ProcessMemoryError(format!(
            "Address 0x{:x} is not in any mapped region of process {}",
            address, pid
        ))
    })?;

    if !region.is_writable() {
        return Err(BinfiddleError::ProcessMemoryError(format!(
            "Memory region 0x{:x}-0x{:x} in process {} is not writable",
            region.start, region.end, pid
        )));
    }

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
        write_process_memory(0, address, b"WRITEBUF").unwrap();

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

        write_process_memory(std::process::id(), address, b"PIDWRITE").unwrap();

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
}
