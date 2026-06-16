//! Process memory access helpers (Linux-only).
//!
//! This module provides a minimal interface to process memory via
//! `/proc/<pid>/maps`, `/proc/self/mem`, and `process_vm_readv` /
//! `process_vm_writev`. It is intentionally scoped to Linux and focuses on
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

    if pid == 0 || pid == std::process::id() {
        read_self_memory(address, size)
    } else {
        read_cross_process_memory(pid, address, size)
    }
}

fn read_self_memory(address: u64, size: u64) -> Result<Vec<u8>> {
    let size = size as usize;

    let mut file = File::open("/proc/self/mem").map_err(|e| {
        BinfiddleError::ProcessMemoryError(format!(
            "Failed to open current process memory for reading: {}",
            e
        ))
    })?;

    file.seek(SeekFrom::Start(address)).map_err(|e| {
        BinfiddleError::ProcessMemoryError(format!(
            "Failed to seek to address 0x{:x} in current process memory: {}",
            address, e
        ))
    })?;

    let mut buffer = vec![0u8; size];
    let mut total_read = 0usize;

    while total_read < size {
        match file.read(&mut buffer[total_read..]) {
            Ok(0) => {
                return Err(BinfiddleError::ProcessMemoryError(format!(
                    "Short read while reading current process memory at 0x{:x}: expected {}, got {}",
                    address, size, total_read
                )));
            }
            Ok(n) => total_read += n,
            Err(e) => {
                return Err(BinfiddleError::ProcessMemoryError(format!(
                    "Failed to read current process memory at 0x{:x}: {}",
                    address + total_read as u64,
                    e
                )));
            }
        }
    }

    Ok(buffer)
}

fn read_cross_process_memory(pid: u32, address: u64, size: u64) -> Result<Vec<u8>> {
    let size = size as usize;
    let mut buffer = vec![0u8; size];
    let mut total_read = 0usize;

    while total_read < size {
        let local_iov = libc::iovec {
            iov_base: buffer[total_read..].as_mut_ptr() as *mut libc::c_void,
            iov_len: size - total_read,
        };
        let remote_iov = libc::iovec {
            iov_base: (address + total_read as u64) as *mut libc::c_void,
            iov_len: size - total_read,
        };

        let ret =
            unsafe { libc::process_vm_readv(pid as libc::pid_t, &local_iov, 1, &remote_iov, 1, 0) };

        if ret < 0 {
            let err = std::io::Error::last_os_error();
            return Err(BinfiddleError::ProcessMemoryError(format!(
                "process_vm_readv failed for pid {}: {} (ensure ptrace access is permitted)",
                pid, err
            )));
        }

        if ret == 0 {
            return Err(BinfiddleError::ProcessMemoryError(format!(
                "Short read while reading process {} memory at 0x{:x}: expected {}, got {}",
                pid, address, size, total_read
            )));
        }

        total_read += ret as usize;
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
            write_self_memory_checked(address, data)
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

fn write_self_memory_checked(address: u64, data: &[u8]) -> Result<()> {
    let regions = parse_maps(0)?;
    let region = find_region(&regions, address).ok_or_else(|| {
        BinfiddleError::ProcessMemoryError(format!(
            "Address 0x{:x} is not in any mapped region of the current process",
            address
        ))
    })?;

    if !region.is_writable() {
        return Err(BinfiddleError::ProcessMemoryError(format!(
            "Memory region 0x{:x}-0x{:x} in the current process is not writable (use --force-writable to override)",
            region.start, region.end
        )));
    }

    check_region_bounds(region, address, data.len())?;
    write_self_memory(address, data)
}

fn force_write_self_memory(address: u64, data: &[u8]) -> Result<()> {
    let regions = parse_maps(0)?;
    let region = find_region(&regions, address).ok_or_else(|| {
        BinfiddleError::ProcessMemoryError(format!(
            "Address 0x{:x} is not in any mapped region of the current process",
            address
        ))
    })?;

    check_region_bounds(region, address, data.len())?;

    let original_prot = prot_from_perms(&region.perms);

    let page_size = page_size();
    let protect_start = region.start & !(page_size - 1);
    let protect_end = (region.end + page_size - 1) & !(page_size - 1);
    let protect_len = protect_end - protect_start;

    let mut guard = MprotectGuard::make_writable(protect_start, protect_len, original_prot)?;
    let result = write_self_memory(address, data);

    // If the write succeeded, a restore failure must be reported to the caller.
    if result.is_ok() {
        guard.restore()?;
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

    check_region_bounds(region, address, data.len())?;

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

/// RAII guard that temporarily makes a remote memory region writable via
/// ptrace-injected `mprotect` and restores the original protection on drop or
/// explicit restore.
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
struct PtraceMprotectGuard<'a> {
    target: nix::unistd::Pid,
    region: &'a MemoryRegion,
    original_prot: libc::c_int,
    restored: bool,
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
impl<'a> PtraceMprotectGuard<'a> {
    fn make_writable(
        target: nix::unistd::Pid,
        region: &'a MemoryRegion,
        original_prot: libc::c_int,
    ) -> Result<Self> {
        ptrace_inject_mprotect(target, region, libc::PROT_READ | libc::PROT_WRITE)?;
        Ok(Self {
            target,
            region,
            original_prot,
            restored: false,
        })
    }

    fn restore(&mut self) -> Result<()> {
        if self.restored {
            return Ok(());
        }
        ptrace_inject_mprotect(self.target, self.region, self.original_prot)?;
        self.restored = true;
        Ok(())
    }
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
impl<'a> Drop for PtraceMprotectGuard<'a> {
    fn drop(&mut self) {
        if !self.restored {
            let _ = ptrace_inject_mprotect(self.target, self.region, self.original_prot);
        }
    }
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
fn force_write_cross_process_memory(
    pid: u32,
    address: u64,
    data: &[u8],
    region: &MemoryRegion,
) -> Result<()> {
    use nix::unistd::Pid;

    let target = Pid::from_raw(pid as i32);
    let original_prot = prot_from_perms(&region.perms);

    let mut guard = PtraceMprotectGuard::make_writable(target, region, original_prot)?;
    let result = process_vm_writev_data(pid, address, data);

    // If the write succeeded, a restore failure must be reported to the caller.
    if result.is_ok() {
        guard.restore()?;
    }

    result
}

#[cfg(not(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
)))]
fn force_write_cross_process_memory(
    _pid: u32,
    _address: u64,
    _data: &[u8],
    _region: &MemoryRegion,
) -> Result<()> {
    Err(BinfiddleError::UnsupportedOperation(
        "--force-writable for cross-process writes is only supported on Linux x86_64 and aarch64"
            .to_string(),
    ))
}

/// Ensures the target process is detached from ptrace when this guard drops,
/// even if the injection logic returns early or panics.
#[cfg(target_os = "linux")]
struct PtraceAttachGuard {
    target: nix::unistd::Pid,
}

#[cfg(target_os = "linux")]
impl Drop for PtraceAttachGuard {
    fn drop(&mut self) {
        let _ = nix::sys::ptrace::detach(self.target, None);
    }
}

#[cfg(target_os = "linux")]
fn ptrace_scope_hint(errno: &nix::errno::Errno) -> String {
    if *errno != nix::errno::Errno::EPERM {
        return String::new();
    }
    match std::fs::read_to_string("/proc/sys/kernel/yama/ptrace_scope") {
        Ok(content) => match content.trim().parse::<i32>() {
            Ok(1) => " (Yama ptrace_scope=1: ptrace is restricted to parent-child relationships)"
                .to_string(),
            Ok(2) => {
                " (Yama ptrace_scope=2: only administrators may use ptrace; try sudo)".to_string()
            }
            Ok(3) => " (Yama ptrace_scope=3: ptrace is disabled)".to_string(),
            _ => String::new(),
        },
        Err(_) => String::new(),
    }
}

/// Uses ptrace to inject an `mprotect` syscall into a stopped target process.
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
fn ptrace_inject_mprotect(
    target: nix::unistd::Pid,
    region: &MemoryRegion,
    prot: libc::c_int,
) -> Result<()> {
    #[cfg(target_arch = "x86_64")]
    {
        ptrace_inject_mprotect_x86_64(target, region, prot)
    }
    #[cfg(target_arch = "aarch64")]
    {
        ptrace_inject_mprotect_aarch64(target, region, prot)
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn ptrace_inject_mprotect_x86_64(
    target: nix::unistd::Pid,
    region: &MemoryRegion,
    prot: libc::c_int,
) -> Result<()> {
    use nix::sys::ptrace;
    use nix::sys::wait::{waitpid, WaitStatus};

    ptrace::attach(target).map_err(|e| {
        let hint = ptrace_scope_hint(&e);
        BinfiddleError::ProcessMemoryError(format!(
            "Failed to attach to process {}: {}{}",
            target, e, hint
        ))
    })?;

    let _attach_guard = PtraceAttachGuard { target };

    match waitpid(target, None) {
        Ok(WaitStatus::Stopped(_, _)) => {}
        Ok(other) => {
            return Err(BinfiddleError::ProcessMemoryError(format!(
                "Unexpected wait status while attaching to {}: {:?}",
                target, other
            )));
        }
        Err(e) => {
            return Err(BinfiddleError::ProcessMemoryError(format!(
                "waitpid failed after attach to {}: {}",
                target, e
            )));
        }
    };

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

    // PtraceAttachGuard::drop detaches the target even if inject_result is an Err.
    inject_result
}

/// Writes a 32-bit value into a 64-bit aligned ptrace word, preserving the
/// surrounding 32 bits. Used on aarch64 to inject a single `svc #0` instruction
/// without corrupting the adjacent instruction.
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
fn ptrace_write_u32_at(target: nix::unistd::Pid, address: u64, value: u32) -> Result<()> {
    use nix::sys::ptrace;

    let aligned = address & !7;
    let shift = (address - aligned) * 8;
    let original = ptrace::read(target, aligned as *mut libc::c_void).map_err(|e| {
        BinfiddleError::ProcessMemoryError(format!(
            "Failed to read injection point from {}: {}",
            target, e
        ))
    })? as u64;

    let mask = !(0xffff_ffff_u64 << shift);
    let new_word = (original & mask) | ((value as u64) << shift);

    ptrace::write(target, aligned as *mut libc::c_void, new_word as i64).map_err(|e| {
        BinfiddleError::ProcessMemoryError(format!(
            "Failed to write injected instruction to {}: {}",
            target, e
        ))
    })
}

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
fn ptrace_inject_mprotect_aarch64(
    target: nix::unistd::Pid,
    region: &MemoryRegion,
    prot: libc::c_int,
) -> Result<()> {
    use nix::sys::ptrace;
    use nix::sys::wait::{waitpid, WaitStatus};

    const SVC_0: u32 = 0xd4000001;

    ptrace::attach(target).map_err(|e| {
        let hint = ptrace_scope_hint(&e);
        BinfiddleError::ProcessMemoryError(format!(
            "Failed to attach to process {}: {}{}",
            target, e, hint
        ))
    })?;

    let _attach_guard = PtraceAttachGuard { target };

    match waitpid(target, None) {
        Ok(WaitStatus::Stopped(_, _)) => {}
        Ok(other) => {
            return Err(BinfiddleError::ProcessMemoryError(format!(
                "Unexpected wait status while attaching to {}: {:?}",
                target, other
            )));
        }
        Err(e) => {
            return Err(BinfiddleError::ProcessMemoryError(format!(
                "waitpid failed after attach to {}: {}",
                target, e
            )));
        }
    };

    // State captured during injection so we can always restore it.
    let mut pc: Option<u64> = None;
    let mut saved_regs: Option<nix::libc::user_regs_struct> = None;
    let mut original_word: Option<u64> = None;

    let inject_result = (|| {
        let mut regs = ptrace::getregs(target).map_err(|e| {
            BinfiddleError::ProcessMemoryError(format!(
                "Failed to read registers of {}: {}",
                target, e
            ))
        })?;

        let current_pc = regs.pc;
        pc = Some(current_pc);
        saved_regs = Some(regs);

        let page_size = page_size();
        let protect_start = region.start & !(page_size - 1);
        let protect_end = (region.end + page_size - 1) & !(page_size - 1);
        let protect_len = protect_end - protect_start;

        let word = ptrace::read(target, current_pc as *mut libc::c_void).map_err(|e| {
            BinfiddleError::ProcessMemoryError(format!(
                "Failed to read injection point from {}: {}",
                target, e
            ))
        })? as u64;
        original_word = Some(word);

        ptrace_write_u32_at(target, current_pc, SVC_0)?;

        // aarch64 syscall ABI: x0-x2 args, x8 syscall number.
        regs.regs[0] = protect_start;
        regs.regs[1] = protect_len;
        regs.regs[2] = prot as u64;
        regs.regs[8] = libc::SYS_mprotect as u64;
        // pc already points at the injected svc #0.

        ptrace::setregs(target, regs).map_err(|e| {
            BinfiddleError::ProcessMemoryError(format!(
                "Failed to set registers of {}: {}",
                target, e
            ))
        })?;

        // Run to syscall entry.
        ptrace::syscall(target, None).map_err(|e| {
            BinfiddleError::ProcessMemoryError(format!(
                "Failed to continue {} to syscall entry: {}",
                target, e
            ))
        })?;
        match waitpid(target, None) {
            Ok(WaitStatus::Stopped(_, nix::sys::signal::Signal::SIGTRAP)) => {}
            Ok(other) => {
                return Err(BinfiddleError::ProcessMemoryError(format!(
                    "Unexpected wait status at syscall entry for {}: {:?}",
                    target, other
                )));
            }
            Err(e) => {
                return Err(BinfiddleError::ProcessMemoryError(format!(
                    "waitpid failed at syscall entry for {}: {}",
                    target, e
                )));
            }
        };

        // Run to syscall exit.
        ptrace::syscall(target, None).map_err(|e| {
            BinfiddleError::ProcessMemoryError(format!(
                "Failed to continue {} to syscall exit: {}",
                target, e
            ))
        })?;
        match waitpid(target, None) {
            Ok(WaitStatus::Stopped(_, nix::sys::signal::Signal::SIGTRAP)) => {}
            Ok(other) => {
                return Err(BinfiddleError::ProcessMemoryError(format!(
                    "Unexpected wait status at syscall exit for {}: {:?}",
                    target, other
                )));
            }
            Err(e) => {
                return Err(BinfiddleError::ProcessMemoryError(format!(
                    "waitpid failed at syscall exit for {}: {}",
                    target, e
                )));
            }
        };

        let post_regs = ptrace::getregs(target).map_err(|e| {
            BinfiddleError::ProcessMemoryError(format!(
                "Failed to read post-syscall registers of {}: {}",
                target, e
            ))
        })?;

        if post_regs.regs[0] as i64 != 0 {
            return Err(BinfiddleError::ProcessMemoryError(format!(
                "mprotect syscall in process {} returned error {}",
                target, post_regs.regs[0] as i64
            )));
        }

        Ok(())
    })();

    // Best-effort restoration of original code and registers.
    if let (Some(current_pc), Some(word), Some(saved)) = (pc, original_word, saved_regs) {
        let _ = ptrace::write(target, current_pc as *mut libc::c_void, word as i64);
        let _ = ptrace::setregs(target, saved);
    }

    // PtraceAttachGuard::drop detaches the target even if inject_result is an Err.
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

/// Verifies that `[address, address + len)` fits inside `region`.
fn check_region_bounds(region: &MemoryRegion, address: u64, len: usize) -> Result<()> {
    let end = address.checked_add(len as u64).ok_or_else(|| {
        BinfiddleError::ProcessMemoryError(format!(
            "Write length {} at address 0x{:x} overflows the address space",
            len, address
        ))
    })?;

    if end > region.end {
        return Err(BinfiddleError::ProcessMemoryError(format!(
            "Write range 0x{:x}-0x{:x} extends beyond mapped region 0x{:x}-0x{:x}",
            address, end, region.start, region.end
        )));
    }

    Ok(())
}

/// RAII guard that temporarily makes a page-aligned range writable and restores
/// the original protection when dropped or explicitly restored.
#[cfg(target_os = "linux")]
struct MprotectGuard {
    addr: *mut libc::c_void,
    len: usize,
    original_prot: libc::c_int,
    restored: bool,
}

#[cfg(target_os = "linux")]
impl MprotectGuard {
    fn make_writable(
        protect_start: u64,
        protect_len: u64,
        original_prot: libc::c_int,
    ) -> Result<Self> {
        let addr = protect_start as *mut libc::c_void;
        let len = protect_len as usize;

        if unsafe { libc::mprotect(addr, len, libc::PROT_READ | libc::PROT_WRITE) } != 0 {
            return Err(BinfiddleError::ProcessMemoryError(format!(
                "mprotect failed for range 0x{:x}-0x{:x}: {}",
                protect_start,
                protect_start + protect_len,
                std::io::Error::last_os_error()
            )));
        }

        Ok(Self {
            addr,
            len,
            original_prot,
            restored: false,
        })
    }

    fn restore(&mut self) -> Result<()> {
        if self.restored {
            return Ok(());
        }

        if unsafe { libc::mprotect(self.addr, self.len, self.original_prot) } != 0 {
            return Err(BinfiddleError::ProcessMemoryError(format!(
                "Failed to restore original memory protection for range {:?}: {}. Pages may still be writable!",
                self.addr, std::io::Error::last_os_error()
            )));
        }

        self.restored = true;
        Ok(())
    }
}

#[cfg(target_os = "linux")]
impl Drop for MprotectGuard {
    fn drop(&mut self) {
        if !self.restored {
            let _ = unsafe { libc::mprotect(self.addr, self.len, self.original_prot) };
        }
    }
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

            // Verify the temporary write permission was restored to read-only.
            let regions = parse_maps(0).expect("should parse /proc/self/maps");
            let region = find_region(&regions, address).expect("mapping should still exist");
            assert!(
                !region.is_writable(),
                "Expected mapping to be restored to read-only, got perms: {}",
                region.perms
            );

            assert_eq!(libc::munmap(mapping, page_size), 0);
        }
    }

    #[test]
    fn test_check_region_bounds() {
        let region = MemoryRegion {
            start: 0x1000,
            end: 0x2000,
            perms: "rw-p".to_string(),
            offset: 0,
            dev: "00:00".to_string(),
            inode: 0,
            pathname: None,
        };

        assert!(check_region_bounds(&region, 0x1000, 0x1000).is_ok());
        assert!(check_region_bounds(&region, 0x1fff, 1).is_ok());
        assert!(check_region_bounds(&region, 0x1fff, 2).is_err());
        assert!(check_region_bounds(&region, 0x2000, 1).is_err());
    }

    #[test]
    fn test_write_self_past_region_end_fails() {
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

        let address = mapping as u64 + page_size as u64 - 1;
        let result = write_process_memory(0, address, &[0u8; 2], false);
        assert!(
            result.is_err(),
            "Expected boundary check to reject writes past region end"
        );

        unsafe {
            libc::munmap(mapping, page_size);
        }
    }

    #[test]
    fn test_write_cross_process_past_region_end_fails() {
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

        let address = mapping as u64 + page_size as u64 - 1;
        let result = write_process_memory(std::process::id(), address, &[0u8; 2], false);
        assert!(
            result.is_err(),
            "Expected boundary check to reject writes past region end"
        );

        unsafe {
            libc::munmap(mapping, page_size);
        }
    }
}
