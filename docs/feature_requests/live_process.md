# **Epic: Process Memory Inspection & Manipulation**  
**Author**: Araray Velho

**Date**: 2025-03-29

**Status:** Draft / Proposal (Open for review)

---

## **1. Overview**  
This document outlines the implementation of **process memory inspection and manipulation** for **Binfiddle**, enabling users to:  
- Read memory from running processes (privileged & unprivileged)  
- Modify process memory (privileged only)  
- Handle sparse/inaccessible memory regions safely  
- Maintain security while minimizing privilege escalation  

The feature will be implemented in **phases**, ensuring modularity and testability.  

---

## **2. Feature Breakdown**  
The implementation is divided into **three major phases**, each containing **smaller, dependent tasks**.  

### **Phase 1: Memory Reading (Read-Only, Non-Privileged First)**  
**Objective**: Enable reading memory from processes with minimal privileges.  

#### **Step 1.1: Memory Region Detection**  
- **Task**: Implement platform-specific memory region enumeration.  
  - **Linux**: Parse `/proc/[pid]/maps`  
  - **Windows**: `VirtualQueryEx`  
  - **macOS**: `vm_region`  
- **Output**: `Vec<MemoryRegion>` listing accessible ranges.  
- **Dependencies**: None (foundational).  

#### **Step 1.2: Non-Privileged Memory Reading**  
- **Task**: Attempt reading memory without elevation.  
  - **Linux**: `/proc/[pid]/mem` (if permitted)  
  - **Windows**: `ReadProcessMemory` (if same user)  
  - **macOS**: `vm_read` (if permitted)  
- **Output**: `Result<Vec<u8>>` (fallible, may fail due to permissions).  
- **Dependencies**: Step 1.1 (region detection).  

#### **Step 1.3: Privilege Escalation Handling**  
- **Task**: If read fails, prompt for elevation (if `--auto-elevate`).  
  - **Linux**: `sudo` (interactive password prompt)  
  - **Windows**: UAC (Admin prompt)  
  - **macOS**: `AuthorizationExecuteWithPrivileges`  
- **Output**: Success/failure of elevation.  
- **Dependencies**: Step 1.2 (fallback on failure).  

#### **Step 1.4: Sparse/Inaccessible Region Handling**  
- **Task**: Skip or zero-fill inaccessible pages.  
  - Detect inaccessible regions before reading.  
  - Option (`--skip-inaccessible` vs `--zero-fill`).  
- **Output**: Partial data with warnings.  
- **Dependencies**: Step 1.1 (region detection).  

---

### **Phase 2: Memory Writing (Privileged Only)**  
**Objective**: Allow modifying process memory (requires elevation).  

#### **Step 2.1: Write Operation API**  
- **Task**: Extend `BinarySource` to support writing.  
  - New `ProcessMemoryWriter` struct.  
  - Requires explicit `--allow-write` flag.  
- **Output**: `write_range(addr, data) -> Result<()>`.  
- **Dependencies**: Phase 1 (reading).  

#### **Step 2.2: Write Privilege Escalation**  
- **Task**: Auto-elevate on write attempts.  
  - Similar to Phase 1.3 but stricter (always requires elevation).  
- **Output**: Success/failure of write.  
- **Dependencies**: Step 2.1.  

#### **Step 2.3: Memory Protection Handling**  
- **Task**: Temporarily disable `PROT_READ/WRITE` guards.  
  - **Linux**: `mprotect`  
  - **Windows**: `VirtualProtectEx`  
  - **macOS**: `vm_protect`  
- **Output**: Restores original permissions after write.  
- **Dependencies**: Step 2.1.  

---

### **Phase 3: CLI & UX Integration**  
**Objective**: Seamlessly integrate into existing Binfiddle commands.  

#### **Step 3.1: New CLI Arguments**  
| Argument              | Description                  | Example               |
| --------------------- | ---------------------------- | --------------------- |
| `--pid`               | Process ID to attach         | `--pid 1234`          |
| `--address`           | Base memory address          | `--address 0x400000`  |
| `--size`              | Bytes to read/write          | `--size 0x100`        |
| `--auto-elevate`      | Attempt privilege escalation | `--auto-elevate`      |
| `--skip-inaccessible` | Skip bad memory regions      | `--skip-inaccessible` |

#### **Step 3.2: Command Integration**  
- **`read`**: Works with process memory.  
  ```sh
  binfiddle --pid 1234 read 0x400000..0x401000
  ```
- **`write`**: Requires elevation.  
  ```sh
  binfiddle --pid 1234 --auto-elevate write 0x400000 "DEADBEEF"
  ```
- **`edit`**: Supports process memory patches.  
  ```sh
  binfiddle --pid 1234 edit replace 0x400000..0x400010 "newdata"
  ```

#### **Step 3.3: Error Handling & User Feedback**  
- **Non-privileged fallback**: Warns but continues.  
- **Permission denied**: Explains why and suggests fixes.  
- **Partial reads**: Reports skipped regions.  

---

## **3. Security Considerations**  
| Risk                           | Mitigation                                 |
| ------------------------------ | ------------------------------------------ |
| **Arbitrary memory access**    | Restrict to same-user processes by default |
| **Privilege escalation abuse** | Require explicit `--auto-elevate`          |
| **Data leaks**                 | Never cache process memory                 |
| **Race conditions**            | Lock process memory during operations      |

---

## **4. Performance Optimizations**  
| Technique                  | Benefit                        |
| -------------------------- | ------------------------------ |
| **Bulk reads**             | Minimize syscalls              |
| **Cached region maps**     | Avoid repeated `/proc` parsing |
| **Lazy permissions check** | Only validate when needed      |

---

## **5. Testing Plan**  
| Test                     | Scope                  |
| ------------------------ | ---------------------- |
| **Non-privileged reads** | Same-user process      |
| **Privileged reads**     | Cross-user process     |
| **Sparse memory**        | `/proc/self/maps` test |
| **Write validation**     | Checksum verification  |

---

## **6. Roadmap & Timeline**

| Phase                         | Estimated Time |
| ----------------------------- | -------------- |
| **Phase 1 (Read-Only)**       | 2-3 weeks      |
| **Phase 2 (Writing)**         | 1-2 weeks      |
| **Phase 3 (CLI Integration)** | 1 week         |

---

## **7. Future Extensions**  
- **Memory pattern scanning** (e.g., find `0xDEADBEEF` in a process).  
- **Cross-architecture support** (32-bit vs 64-bit).  
- **Process freezing** during writes for stability.  

---

## **8. Final**  
This Epic enables **secure, cross-platform process memory manipulation** in Binfiddle, with:  
✅ **Minimal privilege escalation**  
✅ **Sparse memory handling**  
✅ **Seamless CLI integration**  
