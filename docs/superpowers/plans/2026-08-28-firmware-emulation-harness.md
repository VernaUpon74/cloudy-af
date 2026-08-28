# Firmware Emulation Harness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A dependency-free ARMv6-M (Cortex-M0) Thumb interpreter plus a function-level harness that runs patched render functions under a memory ACL, catching bricking-class bugs before any image is flashed.

**Architecture:** New module `src-tauri/src/firmware/emu/` with four units — `bus.rs` (flat memory + write allow-list + peripheral stubs), `thumb.rs` (16-bit Thumb decoder), `cpu.rs` (registers, flags, execute, run loop), `harness.rs` (descriptor-driven scenario runner + frame extraction). Tests: per-file unit tests for opcodes, integration tests in `src-tauri/src/firmware/tests/emu_test.rs`.

**Tech Stack:** Rust only, zero new crates (tests run `cargo test --offline` in the Flatpak GNOME SDK). serde/serde_json (already deps) for the descriptor.

**Spec:** `docs/superpowers/specs/2026-08-28-firmware-emulation-harness-design.md`

## Global Constraints

- All cargo commands run inside the Flatpak SDK, offline:
  `flatpak run --env=FLATPAK_ENABLE_SDK_EXT=rust-stable --filesystem=home --device=all --command=bash org.gnome.Sdk//49 -c 'export PATH=/usr/lib/sdk/rust-stable/bin:$PATH; cd /var/home/j/cloudy-af/src-tauri && cargo test --offline --lib ...'`
- **Zero new dependencies.** Everything is hand-rolled Rust; frame dumps are binary PGM (no image crate).
- Dev/test tooling only: nothing in `emu/` is called from Tauri commands or the flash path.
- The Thumb **bytecode builder does not exist yet** (it is Phase 3 work; the spec's "already in tree" note was wrong). All test code here uses hand-assembled halfwords with the encoding shown in comments. Patches are applied through the real `patch::apply_patch` with `PatchModification` lists built in the test.
- A fault (`EmuError`) anywhere in a harness run = test failure. That is the anti-brick signal.
- Existing interfaces consumed (do not modify):
  - `firmware::patch::{Patch, PatchModification, apply_patch(firmware: &mut [u8], patch: &mut Patch, rollback_log: &mut HashMap<usize, u8>) -> Result<()>}`
  - `firmware::loader::{FirmwareImage { definition, encryption, bytes }, load_firmware(path: &Path, definitions: &[FirmwareDefinition]) -> Result<FirmwareImage>}`
  - `firmware::{FirmwareError, Result}` in `src-tauri/src/firmware/mod.rs`

---

### Task 1: Module skeleton + `EmuError` + `Bus`

**Files:**
- Create: `src-tauri/src/firmware/emu/mod.rs`
- Create: `src-tauri/src/firmware/emu/bus.rs`
- Modify: `src-tauri/src/firmware/mod.rs` (add `pub mod emu;` after line 9 `pub mod stock;`)

**Interfaces:**
- Consumes: nothing outside std.
- Produces (used by every later task):
  ```rust
  // emu/mod.rs
  pub mod bus;
  pub mod thumb;
  pub mod cpu;
  pub mod harness;

  use thiserror::Error;

  #[derive(Debug, Error)]
  pub enum EmuError {
      #[error("unmapped address {addr:#010x}")]
      Unmapped { addr: u32 },
      #[error("unaligned {size}-byte access at {addr:#010x}")]
      Unaligned { addr: u32, size: u8 },
      #[error("undefined instruction {instr:#06x} at pc {pc:#010x}")]
      Undefined { pc: u32, instr: u16 },
      #[error("instruction budget exceeded after {executed} instructions")]
      BudgetExceeded { executed: u64 },
      #[error("descriptor error: {0}")]
      Descriptor(String),
  }

  // emu/bus.rs
  pub const FLASH_BASE: u32 = 0x0000_0000;
  pub const RAM_BASE: u32 = 0x2000_0000;

  #[derive(Debug, Clone, PartialEq)]
  pub struct WriteRecord { pub addr: u32, pub value: u32, pub size: u8 }

  pub struct Bus {
      flash: Vec<u8>,
      ram: Vec<u8>,
      allow: Vec<std::ops::Range<u32>>,
      stubs: std::collections::HashMap<u32, u32>,
      pub write_log: Vec<WriteRecord>,       // all stores that landed in RAM
      pub acl_violations: Vec<WriteRecord>,  // stores outside allow-list or into flash
      pub dropped_writes: Vec<WriteRecord>,  // stores to peripheral space
  }

  impl Bus {
      pub fn new(flash: Vec<u8>, ram_size: usize) -> Self;
      pub fn allow_region(&mut self, range: std::ops::Range<u32>);
      pub fn set_stub(&mut self, addr: u32, value: u32);
      pub fn read_u8(&self, addr: u32) -> Result<u8, EmuError>;
      pub fn read_u16(&self, addr: u32) -> Result<u16, EmuError>;
      pub fn read_u32(&self, addr: u32) -> Result<u32, EmuError>;
      pub fn write_u8(&mut self, addr: u32, value: u8) -> Result<(), EmuError>;
      pub fn write_u16(&mut self, addr: u32, value: u16) -> Result<(), EmuError>;
      pub fn write_u32(&mut self, addr: u32, value: u32) -> Result<(), EmuError>;
  }
  ```

**Address map rules:**
- `FLASH_BASE .. FLASH_BASE+flash.len()` → flash. Reads OK. Stores: record in `acl_violations`, do NOT apply (CPU stores cannot write flash on real hardware — a store here means runaway code).
- `RAM_BASE .. RAM_BASE+ram.len()` → RAM. Reads OK. Stores: apply; if the address is not inside any `allow` region, also record in `acl_violations`. All RAM stores go in `write_log`.
- `0x4000_0000..0x6000_0000` and `0xE000_0000..=0xFFFF_FFFF` → peripheral space. Reads return `stubs.get(&addr)` or 0. Stores are recorded in `dropped_writes`, not applied.
- Anything else → `EmuError::Unmapped`.
- `read_u16/write_u16` with `addr % 2 != 0` → `EmuError::Unaligned { size: 2 }`; `u32` with `addr % 4 != 0` → `Unaligned { size: 4 }`. All accesses little-endian.

- [ ] **Step 1: Write the failing tests** (in `bus.rs`, `#[cfg(test)] mod tests`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn bus() -> Bus {
        let mut b = Bus::new(vec![0u8; 0x1000], 0x1000);
        b.allow_region(RAM_BASE + 0x100..RAM_BASE + 0x200);
        b
    }

    #[test]
    fn test_ram_read_write_roundtrip() {
        let mut b = bus();
        b.write_u32(RAM_BASE + 0x100, 0xDEAD_BEEF).unwrap();
        assert_eq!(b.read_u32(RAM_BASE + 0x100).unwrap(), 0xDEAD_BEEF);
        assert!(b.acl_violations.is_empty());
        assert_eq!(b.write_log.len(), 1);
    }

    #[test]
    fn test_acl_violation_recorded_but_applied() {
        let mut b = bus();
        b.write_u8(RAM_BASE + 0x900, 0xAA).unwrap(); // outside allow region
        assert_eq!(b.read_u8(RAM_BASE + 0x900).unwrap(), 0xAA);
        assert_eq!(b.acl_violations.len(), 1);
        assert_eq!(b.acl_violations[0].addr, RAM_BASE + 0x900);
    }

    #[test]
    fn test_flash_store_is_violation_not_applied() {
        let mut b = bus();
        b.write_u32(FLASH_BASE + 0x40, 0xFFFF_FFFF).unwrap();
        assert_eq!(b.read_u32(FLASH_BASE + 0x40).unwrap(), 0);
        assert_eq!(b.acl_violations.len(), 1);
    }

    #[test]
    fn test_peripheral_stub_read_and_dropped_write() {
        let mut b = bus();
        b.set_stub(0x4000_0004, 0x1);
        assert_eq!(b.read_u32(0x4000_0004).unwrap(), 0x1);
        assert_eq!(b.read_u32(0x4000_0008).unwrap(), 0);
        b.write_u32(0x4000_0004, 0xFF).unwrap();
        assert_eq!(b.dropped_writes.len(), 1);
        assert_eq!(b.read_u32(0x4000_0004).unwrap(), 0x1); // unchanged
    }

    #[test]
    fn test_unmapped_and_unaligned() {
        let mut b = bus();
        assert!(matches!(b.read_u32(0x8000_0000), Err(EmuError::Unmapped { .. })));
        assert!(matches!(b.read_u32(RAM_BASE + 0x101), Err(EmuError::Unaligned { size: 4, .. })));
        assert!(matches!(b.write_u16(RAM_BASE + 0x101, 0), Err(EmuError::Unaligned { size: 2, .. })));
    }

    #[test]
    fn test_endianness() {
        let mut b = bus();
        b.write_u32(RAM_BASE + 0x100, 0x0102_0304).unwrap();
        assert_eq!(b.read_u8(RAM_BASE + 0x100).unwrap(), 0x04);
        assert_eq!(b.read_u16(RAM_BASE + 0x102).unwrap(), 0x0102);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run the Flatpak cargo command from Global Constraints with `cargo test --offline --lib firmware::emu::bus`
Expected: FAIL — `emu` module does not exist (compile error).

- [ ] **Step 3: Implement `mod.rs` and `bus.rs`**

Write `EmuError` and the four `pub mod` lines in `emu/mod.rs` exactly as in Interfaces (create empty `thumb.rs`, `cpu.rs`, `harness.rs` files so the module compiles; later tasks fill them). Add `pub mod emu;` to `firmware/mod.rs` after `pub mod stock;`. Implement `Bus` per the address-map rules above; a helper resolves an address to a region enum:

```rust
enum Region { Flash(usize), Ram(usize), Peripheral }

impl Bus {
    fn resolve(&self, addr: u32) -> Result<Region, EmuError> {
        if addr >= FLASH_BASE && (addr - FLASH_BASE) < self.flash.len() as u32 {
            Ok(Region::Flash((addr - FLASH_BASE) as usize))
        } else if addr >= RAM_BASE && (addr - RAM_BASE) < self.ram.len() as u32 {
            Ok(Region::Ram((addr - RAM_BASE) as usize))
        } else if (0x4000_0000..0x6000_0000).contains(&addr) || addr >= 0xE000_0000 {
            Ok(Region::Peripheral)
        } else {
            Err(EmuError::Unmapped { addr })
        }
    }
}
```

Each `write_uN` first checks alignment, then resolves, then applies the rules. Factor the generic body over size with small per-size wrappers (or a private `write(&mut self, addr, value: u32, size: u8)` the three public fns call after masking).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --offline --lib firmware::emu::bus`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/firmware/emu/ src-tauri/src/firmware/mod.rs
git commit -m "feat(emu): memory bus with ACL watch and peripheral stubs"
```

---

### Task 2: `Cpu` skeleton, run loop, decode group A (moves/shifts/immediate arithmetic)

**Files:**
- Create/fill: `src-tauri/src/firmware/emu/cpu.rs`
- Create/fill: `src-tauri/src/firmware/emu/thumb.rs`

**Interfaces:**
- Consumes: `bus::{Bus, EmuError}` from Task 1.
- Produces:
  ```rust
  // cpu.rs
  pub struct Cpu {
      pub r: [u32; 13],
      pub sp: u32,
      pub lr: u32,
      pub pc: u32,          // address of the instruction being executed (even)
      pub n: bool, pub z: bool, pub c: bool, pub v: bool,
      pub trace: std::collections::VecDeque<u32>, // last 32 executed PCs
  }

  impl Cpu {
      pub fn new() -> Self;
      pub fn reg(&self, i: u8) -> u32;          // 13→sp, 14→lr, 15→pc+4
      pub fn set_reg(&mut self, i: u8, v: u32); // set pc masks bit0
      pub fn step(&mut self, bus: &mut Bus) -> Result<(), EmuError>;
      pub fn run_until(&mut self, bus: &mut Bus, stop_pc: u32, budget: u64)
          -> Result<u64, EmuError>;             // Ok(instructions executed)
      pub fn debug_dump(&self) -> String;       // registers + flags + trace
  }

  // thumb.rs
  #[derive(Debug, Clone, Copy, PartialEq)]
  pub enum Instr { /* variants added per task, see Steps */ }

  pub fn decode(hw: u16) -> Option<Instr>; // None = undefined (or prefix of 32-bit)
  ```

  `step()`: push `pc` to trace (cap 32), fetch `hw = bus.read_u16(pc)?`, `decode(hw)`; `None` → `EmuError::Undefined { pc, instr: hw }`; then `self.execute(bus, instr)?`. Default advance `pc += 2` inside each arm (branch arms set pc directly).

  `run_until`: loop `{ if self.pc == stop_pc { return Ok(n); } self.step(bus)?; n += 1; if n >= budget { return Err(BudgetExceeded) } }`.

  Register access helpers:
  ```rust
  pub fn reg(&self, i: u8) -> u32 {
      match i { 13 => self.sp, 14 => self.lr, 15 => self.pc.wrapping_add(4), _ => self.r[i as usize] }
  }
  pub fn set_reg(&mut self, i: u8, v: u32) {
      match i { 13 => self.sp = v, 14 => self.lr = v, 15 => self.pc = v & !1, _ => self.r[i as usize] = v }
  }
  ```

  Flag helpers (private, used by all groups):
  ```rust
  fn set_nz(&mut self, r: u32) { self.n = r >> 31 != 0; self.z = r == 0; }
  fn add_flags(&mut self, a: u32, b: u32, cin: bool) -> u32 {
      let res = a.wrapping_add(b).wrapping_add(cin as u32);
      let wide = a as u64 + b as u64 + cin as u64;
      self.c = wide > 0xFFFF_FFFF;
      let b2 = b.wrapping_add(cin as u32);
      self.v = (!(a ^ b2) & (a ^ res)) >> 31 != 0;
      self.set_nz(res);
      res
  }
  fn sub_flags(&mut self, a: u32, b: u32, cin: bool) -> u32 {
      // computes a - b - (1 - cin); cin=true means "no borrow"
      let sub = b.wrapping_add(!cin as u32);
      let res = a.wrapping_sub(sub);
      self.c = a as u64 >= sub as u64;
      self.v = ((a ^ sub) & (a ^ res)) >> 31 != 0;
      self.set_nz(res);
      res
  }
  fn lsl_c(&mut self, v: u32, s: u32) -> u32 { // updates C (and NZ by callers)
      if s == 0 { v } else if s < 32 { self.c = v >> (32 - s) & 1 != 0; v << s }
      else if s == 32 { self.c = v & 1 != 0; 0 } else { self.c = false; 0 }
  }
  fn lsr_c(&mut self, v: u32, s: u32) -> u32 { // s==0 means 32 (imm encodings)
      let s = if s == 0 { 32 } else { s };
      if s < 32 { self.c = v >> (s - 1) & 1 != 0; v >> s }
      else if s == 32 { self.c = v >> 31 != 0; 0 } else { self.c = false; 0 }
  }
  fn asr_c(&mut self, v: u32, s: u32) -> u32 { // s==0 means 32
      let s = if s == 0 { 32 } else { s };
      if s >= 32 { self.c = v >> 31 != 0; if self.c { u32::MAX } else { 0 } }
      else { self.c = v >> (s - 1) & 1 != 0; ((v as i32) >> s) as u32 }
  }
  ```

  Decode arms for group A (insert into `decode` before the catch-all `None`):
  ```rust
  match hw >> 11 {
      0b000 => Instr::LslImm { rd: (hw & 7) as u8, rm: ((hw >> 3) & 7) as u8, imm: ((hw >> 6) & 0x1F) as u8 },
      0b001 => Instr::LsrImm { rd: (hw & 7) as u8, rm: ((hw >> 3) & 7) as u8, imm: ((hw >> 6) & 0x1F) as u8 },
      0b010 => Instr::AsrImm { rd: (hw & 7) as u8, rm: ((hw >> 3) & 7) as u8, imm: ((hw >> 6) & 0x1F) as u8 },
      0b011 => {
          let rd = (hw & 7) as u8; let rn = ((hw >> 3) & 7) as u8;
          let val = ((hw >> 6) & 7) as u8;
          match ((hw >> 10) & 1, (hw >> 9) & 1) {
              (0, 0) => Instr::AddReg { rd, rn, rm: val },
              (0, 1) => Instr::SubReg { rd, rn, rm: val },
              (1, 0) => Instr::AddImm3 { rd, rn, imm: val },
              _     => Instr::SubImm3 { rd, rn, imm: val },
          }
      }
      0b100 => Instr::MovImm { rd: ((hw >> 8) & 7) as u8, imm: (hw & 0xFF) as u8 },
      0b101 => Instr::CmpImm { rn: ((hw >> 8) & 7) as u8, imm: (hw & 0xFF) as u8 },
      0b110 => Instr::AddImm8 { rdn: ((hw >> 8) & 7) as u8, imm: (hw & 0xFF) as u8 },
      0b111 => Instr::SubImm8 { rdn: ((hw >> 8) & 7) as u8, imm: (hw & 0xFF) as u8 },
      _ => return None, // later tasks add arms above this
  }
  ```

  Execute arms (in `Cpu::execute`):
  - `LslImm/LsrImm/AsrImm { rd, rm, imm }`: `let res = self.lX_c(self.r[rm as usize], imm as u32); self.set_nz(res); self.r[rd as usize] = res; self.pc += 2;` (LSL imm 0 = MOV: `lsl_c` with s=0 leaves C alone — correct per ARM.)
  - `AddReg/SubReg { rd, rn, rm }`: `add_flags(r[rn], r[rm], false)` / `sub_flags(r[rn], r[rm], true)`.
  - `AddImm3/SubImm3 { rd, rn, imm }`: same with `imm as u32`.
  - `MovImm { rd, imm }`: `set_nz(imm); r[rd] = imm` (C unchanged).
  - `CmpImm { rn, imm }`: `sub_flags(r[rn], imm, true)` discard result.
  - `AddImm8/SubImm8 { rdn, imm }`: accumulate into `r[rdn]`.

- [ ] **Step 1: Write the failing tests** (in `cpu.rs` `#[cfg(test)] mod tests`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::firmware::emu::bus::{Bus, RAM_BASE};

    /// Run one instruction placed at flash offset 0; return the cpu.
    fn run_one(hw: u16) -> Cpu {
        let mut flash = vec![0u8; 0x100];
        flash[0..2].copy_from_slice(&hw.to_le_bytes());
        let mut bus = Bus::new(flash, 0x1000);
        let mut cpu = Cpu::new(); // pc = 0
        cpu.sp = RAM_BASE + 0x800;
        cpu.step(&mut bus).unwrap();
        cpu
    }

    #[test]
    fn test_movs_imm() {
        // 0x2242 = movs r2, #0x42
        let cpu = run_one(0x2242);
        assert_eq!(cpu.r[2], 0x42);
        assert!(!cpu.n && !cpu.z);
        assert_eq!(cpu.pc, 2);
    }

    #[test]
    fn test_movs_zero_sets_z() {
        // 0x2000 = movs r0, #0
        let cpu = run_one(0x2000);
        assert!(cpu.z);
    }

    #[test]
    fn test_lsls_imm() {
        // 0x0088 = lsls r0, r1, #2
        let mut flash = vec![0u8; 0x100];
        flash[0..2].copy_from_slice(&0x0088u16.to_le_bytes());
        let mut bus = Bus::new(flash, 0x1000);
        let mut cpu = Cpu::new();
        cpu.r[1] = 0x4000_0001;
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.r[0], 0x0000_0004);
        assert!(cpu.c); // bit 31 shifted out
    }

    #[test]
    fn test_lsrs_imm32() {
        // 0x0808 = lsrs r0, r1, #0 (encoding means shift 32)
        let mut flash = vec![0u8; 0x100];
        flash[0..2].copy_from_slice(&0x0808u16.to_le_bytes());
        let mut bus = Bus::new(flash, 0x1000);
        let mut cpu = Cpu::new();
        cpu.r[1] = 0x8000_0000;
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.r[0], 0);
        assert!(cpu.c); // old bit 31
        assert!(cpu.z);
    }

    #[test]
    fn test_adds_subs_imm8_flags() {
        // 0x3001 = adds r0, #1 ; 0x3801 = subs r0, #1
        let mut cpu = run_one(0x3001);
        assert_eq!(cpu.r[0], 1);
        assert!(!cpu.c);
        cpu.r[0] = 0xFFFF_FFFF;
        cpu.pc = 0;
        // rerun adds on 0xFFFFFFFF -> 0 with carry
        let mut flash = vec![0u8; 0x100];
        flash[0..2].copy_from_slice(&0x3001u16.to_le_bytes());
        let mut bus = Bus::new(flash, 0x1000);
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.r[0], 0);
        assert!(cpu.c && cpu.z);
    }

    #[test]
    fn test_subs_borrow_clears_c() {
        // 0x1A40 = subs r0, r0, r1
        let mut flash = vec![0u8; 0x100];
        flash[0..2].copy_from_slice(&0x1A40u16.to_le_bytes());
        let mut bus = Bus::new(flash, 0x1000);
        let mut cpu = Cpu::new();
        cpu.r[0] = 3; cpu.r[1] = 5;
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.r[0], (-2i32) as u32);
        assert!(!cpu.c); // borrow
        assert!(cpu.n);
    }

    #[test]
    fn test_cmp_imm() {
        // 0x2805 = cmp r0, #5
        let mut flash = vec![0u8; 0x100];
        flash[0..2].copy_from_slice(&0x2805u16.to_le_bytes());
        let mut bus = Bus::new(flash, 0x1000);
        let mut cpu = Cpu::new();
        cpu.r[0] = 5;
        cpu.step(&mut bus).unwrap();
        assert!(cpu.z && cpu.c);
    }

    #[test]
    fn test_run_until_budget() {
        // 0xE7FE = b . (branch to self)
        let mut flash = vec![0u8; 0x100];
        flash[0..2].copy_from_slice(&0xE7FEu16.to_le_bytes());
        let mut bus = Bus::new(flash, 0x1000);
        let mut cpu = Cpu::new();
        let err = cpu.run_until(&mut bus, 0x1000, 100).unwrap_err();
        assert!(matches!(err, EmuError::BudgetExceeded { .. }));
    }
}
```

(Note: `test_run_until_budget` needs the unconditional-branch arm — implement `B` now with group A; it's two lines: offset = sign-extend-12(imm11 << 1); `pc = pc + 4 + offset`. Decode: `hw >> 12 == 0b1110` → `Instr::B { imm: ((hw as i16 as i32) << 20 >> 20) }` storing the raw imm11; do the sign extension in decode: `let imm11 = hw & 0x7FF; let off = ((imm11 << 1) as u16 as i16 as i32) << ...` — simplest: `let off = (((hw & 0x7FF) as i32) << 21 >> 20) as i32;` then `pc = (pc as i32 + 4 + off) as u32`.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --offline --lib firmware::emu::cpu`
Expected: FAIL (compile error — `Instr` variants don't exist).

- [ ] **Step 3: Implement group A + `B`**

Write `Cpu`, the flag/shift helpers, `step`, `run_until`, `debug_dump`, the decode arms, and the execute arms exactly per Interfaces. `debug_dump` formats r0–r12/sp/lr/pc, flags, and the trace ring — free-form text is fine (tests don't parse it).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --offline --lib firmware::emu::cpu`
Expected: PASS (8 tests).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/firmware/emu/
git commit -m "feat(emu): cpu core, run loop, shifts and immediate arithmetic"
```

---

### Task 3: Decode group B (data-processing, multiply, hi-reg ops, BX/BLX)

**Files:**
- Modify: `src-tauri/src/firmware/emu/thumb.rs`
- Modify: `src-tauri/src/firmware/emu/cpu.rs`

**Interfaces:**
- Consumes: Task 2 `Cpu`, flag/shift helpers, decode/execute structure.
- Produces: new `Instr` variants:
  ```rust
  DataProc { op: u8, rdn: u8, rm: u8 },  // op 0x0..0xF, low regs only
  AddHi { rd: u8, rm: u8 },              // reg ids 0..15
  CmpHi { rn: u8, rm: u8 },
  MovHi { rd: u8, rm: u8 },              // no flags
  Bx { rm: u8 },
  Blx { rm: u8 },
  ```

  Decode arms (before the catch-all):
  ```rust
  if hw >> 10 == 0b010000 { // 0x4000..0x43FF data-processing
      return Some(Instr::DataProc { op: ((hw >> 6) & 0xF) as u8, rdn: (hw & 7) as u8, rm: ((hw >> 3) & 7) as u8 });
  }
  if hw >> 8 >= 0x44 && hw >> 8 <= 0x47 { // 010001xx hi-reg
      let op = (hw >> 8) & 3;
      let rd = ((((hw >> 7) & 1) << 3) | (hw & 7)) as u8;
      let rm = ((((hw >> 6) & 1) << 3) | ((hw >> 3) & 7)) as u8;
      return Some(match op {
          0 => Instr::AddHi { rd, rm },
          1 => Instr::CmpHi { rn: rd, rm },
          2 => Instr::MovHi { rd, rm },
          _ => if (hw >> 7) & 1 == 1 { Instr::Blx { rm } } else { Instr::Bx { rm } },
      });
  }
  ```
  (Restructure `decode` from a single `match hw >> 11` into ordered `if` guards; the group-A arms keep working — put the two guards above the match and keep group A's arms in the match.)

  Execute semantics:
  - `DataProc op`: 0=AND,1=EOR,2=LSL(reg),3=LSR(reg),4=ASR(reg),5=ADC,6=SBC,7=ROR,8=TST,9=RSB(neg),10=CMP,11=CMN,12=ORR,13=MUL,14=BIC,15=MVN. All set N,Z; logical ops set C from shifter only for shifts; MUL sets N,Z (C,V unchanged); TST/CMP/CMN discard result. ROR by register: `s = rm & 0xFF`; if `s==0` result=v, C unchanged; else `s &= 31; if s==0 { C=bit31; result=v } else { result = v.rotate_right(s); C = bit31 of result }`.
  - `Bx { rm }`: `pc = reg(rm) & !1`.
  - `Blx { rm }`: `lr = (pc + 2) | 1; pc = reg(rm) & !1;` (target must be Thumb; bit0 of target is the mode bit — masked).

- [ ] **Step 1: Write the failing tests** (append to `cpu.rs` tests)

```rust
#[test]
fn test_dataproc_mul() {
    // 0x4348 = muls r0, r1, r0
    let mut flash = vec![0u8; 0x100];
    flash[0..2].copy_from_slice(&0x4348u16.to_le_bytes());
    let mut bus = Bus::new(flash, 0x1000);
    let mut cpu = Cpu::new();
    cpu.r[0] = 7; cpu.r[1] = 6;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.r[0], 42);
}

#[test]
fn test_dataproc_rsb() {
    // 0x4248 = rsbs r0, r1, #0  (negs r0, r1)
    let mut flash = vec![0u8; 0x100];
    flash[0..2].copy_from_slice(&0x4248u16.to_le_bytes());
    let mut bus = Bus::new(flash, 0x1000);
    let mut cpu = Cpu::new();
    cpu.r[1] = 5;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.r[0], (-5i32) as u32);
    assert!(cpu.n);
}

#[test]
fn test_dataproc_adc() {
    // 0x4140 = adcs r0, r0
    let mut flash = vec![0u8; 0x100];
    flash[0..2].copy_from_slice(&0x4140u16.to_le_bytes());
    let mut bus = Bus::new(flash, 0x1000);
    let mut cpu = Cpu::new();
    cpu.r[0] = 1; cpu.c = true;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.r[0], 3);
}

#[test]
fn test_mov_hi_and_bx() {
    // 0x46F7 = mov pc, lr  -- use mov lr, r0 then bx lr instead:
    // 0x4687 = mov lr, r0 ; 0x4770 = bx lr
    let mut flash = vec![0u8; 0x100];
    flash[0..2].copy_from_slice(&0x4687u16.to_le_bytes());
    flash[2..4].copy_from_slice(&0x4770u16.to_le_bytes());
    let mut bus = Bus::new(flash, 0x1000);
    let mut cpu = Cpu::new();
    cpu.r[0] = 0x41; // bit0 set (thumb)
    cpu.step(&mut bus).unwrap(); // mov lr, r0
    assert_eq!(cpu.lr, 0x41);
    cpu.step(&mut bus).unwrap(); // bx lr
    assert_eq!(cpu.pc, 0x40); // bit0 masked
}

#[test]
fn test_blx_sets_lr() {
    // 0x4788 = blx r1
    let mut flash = vec![0u8; 0x100];
    flash[0..2].copy_from_slice(&0x4788u16.to_le_bytes());
    let mut bus = Bus::new(flash, 0x1000);
    let mut cpu = Cpu::new();
    cpu.r[1] = 0x21;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 0x20);
    assert_eq!(cpu.lr, 3); // (0 + 2) | 1
}

#[test]
fn test_tst_discards() {
    // 0x4208 = tst r0, r1
    let mut flash = vec![0u8; 0x100];
    flash[0..2].copy_from_slice(&0x4208u16.to_le_bytes());
    let mut bus = Bus::new(flash, 0x1000);
    let mut cpu = Cpu::new();
    cpu.r[0] = 0xF0; cpu.r[1] = 0x0F;
    cpu.step(&mut bus).unwrap();
    assert!(cpu.z);
    assert_eq!(cpu.r[0], 0xF0);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --offline --lib firmware::emu::cpu`
Expected: FAIL (compile error — variants don't exist).

- [ ] **Step 3: Implement group B**

Add the variants, decode guards, and execute arms per Interfaces. `DataProc` shift-by-register ops read the shift amount from the low byte of `r[rm]` and reuse `lsl_c`/`lsr_c`/`asr_c` (pass the amount directly; register shifts treat 0 as 0, unlike the immediate forms — pass `s: u32` straight through and only special-case LSR/ASR when the *source* is an immediate encoding, which group A already handled).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --offline --lib firmware::emu::cpu`
Expected: PASS (14 tests total).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/firmware/emu/
git commit -m "feat(emu): data-processing, multiply, hi-register and branch-exchange ops"
```

---

### Task 4: Decode group C (loads/stores, ADR, SP adjust)

**Files:**
- Modify: `src-tauri/src/firmware/emu/thumb.rs`
- Modify: `src-tauri/src/firmware/emu/cpu.rs`

**Interfaces:**
- Consumes: Tasks 1–3.
- Produces: new `Instr` variants:
  ```rust
  LdrLit { rt: u8, imm: u8 },                       // LDR rt, [PC, #imm8*4]
  LsReg { op: u8, rt: u8, rn: u8, rm: u8 },         // reg offset; op: 0=STR,1=STRH,2=STRB,3=LDRSB,4=LDR,5=LDRH,6=LDRB,7=LDRSH
  LsImm { load: bool, byte: bool, rt: u8, rn: u8, imm: u8 },  // word/byte imm5 offset
  LshImm { load: bool, rt: u8, rn: u8, imm: u8 },   // halfword imm5 offset
  LsSp { load: bool, rt: u8, imm: u8 },             // SP-relative, imm8*4
  Adr { rd: u8, imm: u8 },                          // ADD rd, PC, #imm8*4
  AddSpImm { rd: u8, imm: u8 },                     // ADD rd, SP, #imm8*4
  AdjSp { sub: bool, imm: u8 },                     // ADD/SUB SP, #imm7*4
  ```

  Decode arms (before the catch-all):
  ```rust
  if hw >> 11 == 0b01001 { return Some(Instr::LdrLit { rt: ((hw >> 8) & 7) as u8, imm: (hw & 0xFF) as u8 }); }
  if hw >> 12 == 0b0101 {
      return Some(Instr::LsReg { op: ((hw >> 9) & 7) as u8, rm: ((hw >> 6) & 7) as u8, rn: ((hw >> 3) & 7) as u8, rt: (hw & 7) as u8 });
  }
  if hw >> 13 == 0b011 {
      let op = (hw >> 11) & 3; // 0=STR,1=LDR,2=STRB,3=LDRB
      return Some(Instr::LsImm { load: op & 1 == 1, byte: op >= 2, rt: (hw & 7) as u8, rn: ((hw >> 3) & 7) as u8, imm: ((hw >> 6) & 0x1F) as u8 });
  }
  if hw >> 12 == 0b1000 {
      return Some(Instr::LshImm { load: (hw >> 11) & 1 == 1, rt: (hw & 7) as u8, rn: ((hw >> 3) & 7) as u8, imm: ((hw >> 6) & 0x1F) as u8 });
  }
  if hw >> 12 == 0b1001 {
      return Some(Instr::LsSp { load: (hw >> 11) & 1 == 1, rt: ((hw >> 8) & 7) as u8, imm: (hw & 0xFF) as u8 });
  }
  if hw >> 12 == 0b1010 {
      let rd = ((hw >> 8) & 7) as u8; let imm = (hw & 0xFF) as u8;
      return Some(if (hw >> 11) & 1 == 0 { Instr::Adr { rd, imm } } else { Instr::AddSpImm { rd, imm } });
  }
  if hw >> 8 == 0xB0 { // 1011 0000 x imm7  SP adjust
      return Some(Instr::AdjSp { sub: (hw >> 7) & 1 == 1, imm: (hw & 0x7F) as u8 });
  }
  ```

  Address computations:
  - `LdrLit`: `let base = (self.pc + 4) & !3; let addr = base + (imm as u32) * 4;` load word; flags untouched.
  - `LsReg`: `addr = r[rn] + r[rm]`; per op: 0/1/2 store u32/u16/u8 of `r[rt]`; 4/5/6 load u32/u16/u8 zero-extended; 3/7 load byte/half sign-extended (`as i8 as i32 as u32`).
  - `LsImm`: `addr = r[rn] + imm * (if byte {1} else {4})`.
  - `LshImm`: `addr = r[rn] + imm * 2`.
  - `LsSp`: `addr = sp + imm * 4`.
  - `Adr`: `r[rd] = ((pc + 4) & !3) + imm*4`. `AddSpImm`: `r[rd] = sp + imm*4`. `AdjSp`: `sp ±= imm*4`. No flags for any of these.

- [ ] **Step 1: Write the failing tests** (append to `cpu.rs` tests)

```rust
#[test]
fn test_str_ldr_imm_word() {
    // 0x6008 = str r0, [r1, #0] ; 0x680A = ldr r2, [r1, #0]
    let mut flash = vec![0u8; 0x100];
    flash[0..2].copy_from_slice(&0x6008u16.to_le_bytes());
    flash[2..4].copy_from_slice(&0x680Au16.to_le_bytes());
    let mut bus = Bus::new(flash, 0x1000);
    bus.allow_region(RAM_BASE..RAM_BASE + 0x1000);
    let mut cpu = Cpu::new();
    cpu.r[0] = 0xCAFE_BABE;
    cpu.r[1] = RAM_BASE + 0x200;
    cpu.step(&mut bus).unwrap();
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.r[2], 0xCAFE_BABE);
    assert!(bus.acl_violations.is_empty());
}

#[test]
fn test_ldr_literal_aligns_pc() {
    // 0x4801 at pc=2 -> base = align(2+4,4)=4, +4 -> addr 8
    // 0x4801 = ldr r0, [pc, #4]
    let mut flash = vec![0u8; 0x100];
    flash[2..4].copy_from_slice(&0x4801u16.to_le_bytes());
    flash[8..12].copy_from_slice(&0x1234_5678u32.to_le_bytes());
    let mut bus = Bus::new(flash, 0x1000);
    let mut cpu = Cpu::new();
    cpu.pc = 2;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.r[0], 0x1234_5678);
}

#[test]
fn test_ldrsb_sign_extend() {
    // 0x5608 = ldrsb r0, [r0, r1] -- wait: ldrsb rt,[rn,rm]: op=3: 0x5600|rm<<6|rn<<3|rt
    // rm=1,rn=0,rt=0 -> 0x5600 + 0x40 = 0x5640? compute: 0101 011 0 001 000 000
    //   = 0x5640. Use that.
    let mut flash = vec![0u8; 0x100];
    flash[0..2].copy_from_slice(&0x5640u16.to_le_bytes());
    let mut bus = Bus::new(flash, 0x1000);
    bus.allow_region(RAM_BASE..RAM_BASE + 0x1000);
    bus.write_u8(RAM_BASE + 0x20, 0x80).unwrap();
    let mut cpu = Cpu::new();
    cpu.r[0] = RAM_BASE + 0x10;
    cpu.r[1] = 0x10;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.r[0], 0xFFFF_FF80);
}

#[test]
fn test_sp_relative_and_adjust() {
    // 0xB002 = add sp, #8 ; 0x9801 = ldr r0, [sp, #4] ; 0x9001 = str r0, [sp, #4]
    let mut flash = vec![0u8; 0x100];
    flash[0..2].copy_from_slice(&0xB002u16.to_le_bytes());
    let mut bus = Bus::new(flash, 0x1000);
    let mut cpu = Cpu::new();
    cpu.sp = RAM_BASE + 0x800;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.sp, RAM_BASE + 0x808);
}

#[test]
fn test_str_acl_violation_through_cpu() {
    // 0x6008 = str r0, [r1, #0] into a non-allowed RAM region
    let mut flash = vec![0u8; 0x100];
    flash[0..2].copy_from_slice(&0x6008u16.to_le_bytes());
    let mut bus = Bus::new(flash, 0x1000); // nothing allowed
    let mut cpu = Cpu::new();
    cpu.r[0] = 0xAA;
    cpu.r[1] = RAM_BASE + 0x200;
    cpu.step(&mut bus).unwrap();
    assert_eq!(bus.acl_violations.len(), 1);
}

#[test]
fn test_unaligned_ldr_faults() {
    // 0x6808 = ldr r0, [r1, #0] at an odd address
    let mut flash = vec![0u8; 0x100];
    flash[0..2].copy_from_slice(&0x6808u16.to_le_bytes());
    let mut bus = Bus::new(flash, 0x1000);
    let mut cpu = Cpu::new();
    cpu.r[1] = RAM_BASE + 0x101;
    let err = cpu.step(&mut bus).unwrap_err();
    assert!(matches!(err, EmuError::Unaligned { size: 4, .. }));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --offline --lib firmware::emu::cpu`
Expected: FAIL (compile error).

- [ ] **Step 3: Implement group C** per Interfaces.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --offline --lib firmware::emu::cpu`
Expected: PASS (20 tests total).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/firmware/emu/
git commit -m "feat(emu): load/store addressing modes, ADR and SP arithmetic"
```

---

### Task 5: Decode group D (stack ops, multi load/store, BL, misc)

**Files:**
- Modify: `src-tauri/src/firmware/emu/thumb.rs`
- Modify: `src-tauri/src/firmware/emu/cpu.rs`

**Interfaces:**
- Consumes: Tasks 1–4.
- Produces: new `Instr` variants:
  ```rust
  Push { list: u8, lr: bool },
  Pop { list: u8, pc: bool },
  Stm { rn: u8, list: u8 },
  Ldm { rn: u8, list: u8 },
  BCond { cond: u8, off: i32 },   // off already sign-extended, byte offset
  B { off: i32 },                 // (moved here from Task 2's quick version; keep same semantics)
  Bl { off: i32 },                // 32-bit; decode reads the second halfword
  Svc { imm: u8 },
  Bkpt { imm: u8 },
  Extend { op: u8, rd: u8, rm: u8 }, // 0=SXTH,1=SXTB,2=UXTH,3=UXTB
  Rev { op: u8, rd: u8, rm: u8 },    // 0=REV,1=REV16,3=REVSH
  Nop,
  ```

  Decode arms:
  ```rust
  if hw >> 9 == 0b1011_010 { // 0xB400/0xB500 PUSH: 1011 0 10 1 list
      return Some(Instr::Push { list: (hw & 0xFF) as u8, lr: (hw >> 8) & 1 == 1 });
  }
  if hw >> 9 == 0b1011_110 { // POP: 1011 1 10 1 list
      return Some(Instr::Pop { list: (hw & 0xFF) as u8, pc: (hw >> 8) & 1 == 1 });
  }
  if hw >> 8 == 0xBA && (hw >> 6) & 3 != 2 { // REV family
      return Some(Instr::Rev { op: ((hw >> 6) & 3) as u8, rd: (hw & 7) as u8, rm: ((hw >> 3) & 7) as u8 });
  }
  if hw >> 8 == 0xB2 { // extend family
      return Some(Instr::Extend { op: ((hw >> 6) & 3) as u8, rd: (hw & 7) as u8, rm: ((hw >> 3) & 7) as u8 });
  }
  if hw >> 8 == 0xBE { return Some(Instr::Bkpt { imm: (hw & 0xFF) as u8 }); }
  if hw >> 8 == 0xBF || hw == 0xB660 { return Some(Instr::Nop); } // hints + CPS
  if hw >> 12 == 0b1100 {
      let rn = ((hw >> 8) & 7) as u8; let list = (hw & 0xFF) as u8;
      return Some(if (hw >> 11) & 1 == 0 { Instr::Stm { rn, list } } else { Instr::Ldm { rn, list } });
  }
  if hw >> 12 == 0b1101 {
      let cond = ((hw >> 8) & 0xF) as u8;
      if cond == 0b1111 { return Some(Instr::Svc { imm: (hw & 0xFF) as u8 }); }
      if cond == 0b1110 { return None; }
      let off = (((hw & 0xFF) as i32) << 24 >> 23); // sign-extend imm8<<1
      return Some(Instr::BCond { cond, off });
  }
  if hw >> 11 == 0b11110 { // BL prefix; second halfword at pc+2 — see note
      // decode() is pure; BL needs the second halfword, so handle in Cpu::step:
      unreachable!(); // step() intercepts BL before calling decode
  }
  ```

  **BL handling:** `decode` cannot see the second halfword, so `Cpu::step` intercepts: if `hw >> 11 == 0b11110`, fetch `hw2 = bus.read_u16(pc + 2)?`, compute the offset per the ARM formula, set `lr = (pc + 4) | 1`, `pc = pc + 4 + off`, and skip the `decode` call:
  ```rust
  let s = (hw >> 10) & 1;
  let j1 = (hw2 >> 13) & 1; let j2 = (hw2 >> 11) & 1;
  let i1 = (!(j1 ^ s)) & 1; let i2 = (!(j2 ^ s)) & 1;
  let imm25 = ((s as u32) << 24) | ((i1 as u32) << 23) | ((i2 as u32) << 22)
      | (((hw & 0x3FF) as u32) << 12) | (((hw2 & 0x7FF) as u32) << 1);
  let off = ((imm25 << 7) as i32) >> 7; // sign-extend 25 bits
  self.lr = self.pc.wrapping_add(4) | 1;
  self.pc = (self.pc as i32).wrapping_add(4).wrapping_add(off) as u32;
  ```

  Condition evaluation:
  ```rust
  fn cond_true(&self, cond: u8) -> bool {
      match cond {
          0 => self.z, 1 => !self.z, 2 => self.c, 3 => !self.c,
          4 => self.n, 5 => !self.n, 6 => self.v, 7 => !self.v,
          8 => self.c && !self.z, 9 => !self.c || self.z,
          10 => self.n == self.v, 11 => self.n != self.v,
          12 => !self.z && self.n == self.v, 13 => self.z || self.n != self.v,
          _ => false,
      }
  }
  ```

  Execute semantics:
  - `Push { list, lr }`: count set bits (+ lr), `sp -= 4*count`; store r0..r7 (ascending address) then lr at the end. `Pop { list, pc }`: load r0..r7 ascending; if pc: `pc = value & !1`; `sp += 4*count`.
  - `Stm { rn, list }`: store listed regs ascending from `r[rn]`, then `r[rn] += 4*count` (writeback). `Ldm`: load ascending; writeback unless `rn` is in the list.
  - `BCond`: if `cond_true(cond) { pc = pc + 4 + off } else { pc += 2 }`.
  - `Svc`/`Bkpt`: no-op, `pc += 2` (supervisor/breakpoint have no meaning in this harness; a render function will not execute them in practice).
  - `Extend`: 0 = `r[rd] = r[rm] as i16 as i32 as u32`; 1 = `as i8`; 2 = `r[rm] & 0xFFFF`; 3 = `r[rm] & 0xFF`.
  - `Rev`: 0 = `swap_bytes`; 1 = swap bytes within each halfword; 3 = `(REV16 low half) sign-extended`: `(v as u16).swap_bytes() as i16 as i32 as u32`.
  - `Nop`: `pc += 2`.

- [ ] **Step 1: Write the failing tests** (append to `cpu.rs` tests)

```rust
#[test]
fn test_push_pop_roundtrip() {
    // 0xB510 = push {r4, lr} ; 0xBD10 = pop {r4, pc}
    let mut flash = vec![0u8; 0x100];
    flash[0..2].copy_from_slice(&0xB510u16.to_le_bytes());
    flash[2..4].copy_from_slice(&0xBD10u16.to_le_bytes());
    let mut bus = Bus::new(flash, 0x1000);
    bus.allow_region(RAM_BASE..RAM_BASE + 0x1000);
    let mut cpu = Cpu::new();
    cpu.sp = RAM_BASE + 0x800;
    cpu.r[4] = 0x1122_3344;
    cpu.lr = 0x61; // thumb return to 0x60
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.sp, RAM_BASE + 0x7F8);
    cpu.r[4] = 0; // clobber to prove pop restores
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.r[4], 0x1122_3344);
    assert_eq!(cpu.sp, RAM_BASE + 0x800);
    assert_eq!(cpu.pc, 0x60);
}

#[test]
fn test_bcond_taken_and_not() {
    // 0xD101 = bne +2
    let mut flash = vec![0u8; 0x100];
    flash[0..2].copy_from_slice(&0xD101u16.to_le_bytes());
    let mut bus = Bus::new(flash, 0x1000);
    let mut cpu = Cpu::new();
    cpu.z = false;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 6); // 0 + 4 + 2
    let mut cpu2 = Cpu::new();
    cpu2.z = true;
    cpu2.step(&mut bus).unwrap();
    assert_eq!(cpu2.pc, 2);
}

#[test]
fn test_bl_link_and_target() {
    // BL to +0x10: first hw 0xF000, second 0xF008 (S=0,imm10=0,J1=J2=1,imm11=8 -> off=16)
    let mut flash = vec![0u8; 0x100];
    flash[0..2].copy_from_slice(&0xF000u16.to_le_bytes());
    flash[2..4].copy_from_slice(&0xF008u16.to_le_bytes());
    let mut bus = Bus::new(flash, 0x1000);
    let mut cpu = Cpu::new();
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 0x14); // 0 + 4 + 16
    assert_eq!(cpu.lr, 5);    // (0+4)|1
}

#[test]
fn test_stm_ldm() {
    // 0xC006 = stm r0!, {r1, r2} ; 0xC806 = ldm r0!, {r1, r2}
    let mut flash = vec![0u8; 0x100];
    flash[0..2].copy_from_slice(&0xC006u16.to_le_bytes());
    let mut bus = Bus::new(flash, 0x1000);
    bus.allow_region(RAM_BASE..RAM_BASE + 0x1000);
    let mut cpu = Cpu::new();
    cpu.r[0] = RAM_BASE + 0x100;
    cpu.r[1] = 0x11; cpu.r[2] = 0x22;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.r[0], RAM_BASE + 0x108);
    assert_eq!(bus.read_u32(RAM_BASE + 0x100).unwrap(), 0x11);
    assert_eq!(bus.read_u32(RAM_BASE + 0x104).unwrap(), 0x22);
}

#[test]
fn test_extend_and_rev() {
    // 0xB201 = sxth r1, r0 ; 0xBA01 = rev r1, r0
    let mut flash = vec![0u8; 0x100];
    flash[0..2].copy_from_slice(&0xB201u16.to_le_bytes());
    flash[2..4].copy_from_slice(&0xBA01u16.to_le_bytes());
    let mut bus = Bus::new(flash, 0x1000);
    let mut cpu = Cpu::new();
    cpu.r[0] = 0x1234_80FF;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.r[1], 0xFFFF_80FF);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.r[1], 0xFF80_3412);
}

#[test]
fn test_undefined_instruction_faults() {
    // 0xDE00 is architecturally undefined in v6-M
    let mut flash = vec![0u8; 0x100];
    flash[0..2].copy_from_slice(&0xDE00u16.to_le_bytes());
    let mut bus = Bus::new(flash, 0x1000);
    let mut cpu = Cpu::new();
    let err = cpu.step(&mut bus).unwrap_err();
    assert!(matches!(err, EmuError::Undefined { pc: 0, instr: 0xDE00 }));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --offline --lib firmware::emu::cpu`
Expected: FAIL (compile error).

- [ ] **Step 3: Implement group D** per Interfaces (including the `step()` BL interception).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --offline --lib firmware::emu::cpu`
Expected: PASS (26 tests total).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/firmware/emu/
git commit -m "feat(emu): stack ops, multi load/store, BL, extend/reverse, hints"
```

---

### Task 6: Harness — descriptor, setup, frame loop, Block1 unpacker, PGM dump

**Files:**
- Create/fill: `src-tauri/src/firmware/emu/harness.rs`
- Test: unit tests inside `harness.rs`

**Interfaces:**
- Consumes: `bus::Bus`, `cpu::Cpu`, `EmuError`, `patch::apply_patch` (only in Task 7), serde/serde_json (existing deps).
- Produces (Task 7 and later phases rely on these exact names):
  ```rust
  pub const RETURN_SENTINEL: u32 = 0xFFFF_FFFE;

  #[derive(Debug, serde::Deserialize)]
  pub struct AddrRange {
      #[serde(deserialize_with = "crate::firmware::emu::harness::de_hex")]
      pub start: u32,
      #[serde(deserialize_with = "crate::firmware::emu::harness::de_hex")]
      pub end: u32,
  }

  #[derive(Debug, serde::Deserialize)]
  pub struct DisplayBuffer {
      #[serde(flatten)]
      pub range: AddrRange,
      pub width: usize,
      pub height: usize,
  }

  #[derive(Debug, serde::Deserialize)]
  pub struct Descriptor {
      pub build: String,
      #[serde(deserialize_with = "de_hex")]
      pub render_entry: u32,
      pub display_buffer: DisplayBuffer,
      #[serde(default)]
      pub ram_globals: Vec<AddrRange>,
      #[serde(default)]
      pub ram_size: usize,          // 0 -> default 0x8000
      #[serde(default)]
      pub args: [u32; 4],           // r0..r3 at entry
  }

  pub fn de_hex<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u32, D::Error>;
  // accepts "0x20001000" or "536879104" strings
  pub fn load_descriptor(json: &str) -> Result<Descriptor, EmuError>;

  pub struct Frame { pub width: usize, pub height: usize, pub pixels: Vec<u8> } // 0|1, row-major

  pub struct Harness { pub cpu: Cpu, pub bus: Bus, pub desc: Descriptor }

  impl Harness {
      pub fn new(image: &[u8], desc: Descriptor) -> Self;
      pub fn run_frame(&mut self, budget: u64) -> Result<Frame, EmuError>;
      pub fn frames_differ(a: &Frame, b: &Frame) -> bool;
  }

  pub fn unpack_block1(buf: &[u8], width: usize, height: usize) -> Frame;
  pub fn dump_pgm(frame: &Frame, path: &std::path::Path) -> std::io::Result<()>;
  ```

  Setup rules (`Harness::new`):
  - `ram_size = desc.ram_size or 0x8000`; build `Bus::new(image.to_vec(), ram_size)`.
  - Allow regions: `display_buffer.range`, every `ram_globals` entry, and the stack: `RAM_BASE+ram_size-0x1000 .. RAM_BASE+ram_size`.
  - `cpu.sp = RAM_BASE + ram_size - 16` (8-aligned); write canary `0xDEAD_C0DE` at `sp-4`; `cpu.lr = RETURN_SENTINEL | 1`; `cpu.r[0..4] = desc.args`.
  - `run_frame`: `cpu.pc = desc.render_entry & !1`; `cpu.lr = RETURN_SENTINEL | 1`; `cpu.run_until(&mut bus, RETURN_SENTINEL, budget)?`; then read `display_buffer.range` bytes from the bus and `unpack_block1`. If `cpu.pc` never reaches the sentinel the budget error propagates. **If `bus.acl_violations` is non-empty after the run, return `EmuError::Descriptor(format!("ACL violations: {:?}", bus.acl_violations))`** — this is the anti-brick assertion, surfaced as an error so tests fail with full context. (Descriptor variant reused to avoid a new enum; message carries `cpu.debug_dump()`.)

  `unpack_block1`: Block1 vertical packing — byte `x + (y/8)*width`, bit `y%8` (LSB = topmost of the 8-pixel column group), set bit = pixel on:
  ```rust
  pub fn unpack_block1(buf: &[u8], width: usize, height: usize) -> Frame {
      let mut pixels = vec![0u8; width * height];
      for y in 0..height {
          for x in 0..width {
              let byte = buf[x + (y / 8) * width];
              pixels[y * width + x] = (byte >> (y % 8)) & 1;
          }
      }
      Frame { width, height, pixels }
  }
  ```

  `dump_pgm`: binary P5 PGM, maxval 255, pixel on = 255:
  ```rust
  pub fn dump_pgm(frame: &Frame, path: &std::path::Path) -> std::io::Result<()> {
      use std::io::Write;
      let mut f = std::fs::File::create(path)?;
      write!(f, "P5\n{} {}\n255\n", frame.width, frame.height)?;
      let data: Vec<u8> = frame.pixels.iter().map(|p| p * 255).collect();
      f.write_all(&data)
  }
  ```

- [ ] **Step 1: Write the failing tests** (in `harness.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::firmware::emu::bus::RAM_BASE;

    const DESC_JSON: &str = r#"{
        "build": "synthetic",
        "render_entry": "0x00000080",
        "display_buffer": { "start": "0x20001000", "end": "0x20001400", "width": 64, "height": 128 },
        "ram_globals": [ { "start": "0x20000000", "end": "0x20000100" } ],
        "ram_size": 8192,
        "args": [0, 0, 0, 0]
    }"#;

    /// Image: render function at 0x80 writes 0xFF to display_buffer[0] and returns.
    /// 0x80: 0x21FF        movs r1, #0xFF
    /// 0x82: 0x4802        ldr  r0, [pc, #8]   ; base=align(0x82+4,4)=0x84, +8=0x8C
    /// 0x84: 0x6008        str  r0, [r1, #0]  -- WRONG reg order; use:
    ///   real sequence (fixed below in the image builder):
    fn synthetic_image() -> Vec<u8> {
        let mut img = vec![0u8; 0x100];
        let code: [u16; 5] = [
            0x4902, // 0x80: ldr r1, [pc, #8]  ; base=align(0x84,4)=0x84, addr=0x8C -> &display_buffer
            0x20FF, // 0x82: movs r0, #0xFF
            0x7008, // 0x84: strb r0, [r1, #0]
            0x4770, // 0x86: bx lr
            0x46C0, // 0x88: nop (alignment pad)
        ];
        for (i, hw) in code.iter().enumerate() {
            img[0x80 + i * 2..0x80 + i * 2 + 2].copy_from_slice(&hw.to_le_bytes());
        }
        img[0x8C..0x90].copy_from_slice(&(RAM_BASE + 0x1000).to_le_bytes());
        img
    }

    #[test]
    fn test_descriptor_parses() {
        let d = load_descriptor(DESC_JSON).unwrap();
        assert_eq!(d.build, "synthetic");
        assert_eq!(d.render_entry, 0x80);
        assert_eq!(d.display_buffer.range.start, RAM_BASE + 0x1000);
        assert_eq!(d.display_buffer.width, 64);
    }

    #[test]
    fn test_run_frame_writes_pixel() {
        let d = load_descriptor(DESC_JSON).unwrap();
        let mut h = Harness::new(&synthetic_image(), d);
        let frame = h.run_frame(100_000).unwrap();
        assert_eq!(frame.pixels[0], 1);   // x=0,y=0 set by the 0xFF byte
        assert_eq!(frame.pixels[7], 1);
        assert_eq!(frame.pixels[8], 0);   // second byte untouched
    }

    #[test]
    fn test_frames_differ() {
        let mut a = Frame { width: 2, height: 2, pixels: vec![0; 4] };
        let b = a.clone();
        assert!(!Harness::frames_differ(&a, &b));
        a.pixels[3] = 1;
        assert!(Harness::frames_differ(&a, &b));
    }

    #[test]
    fn test_run_frame_catches_acl_violation() {
        // Same image but descriptor forbids the display buffer (empty allow) —
        // simulate by pointing display_buffer at flash... simpler: ram_globals
        // empty and display buffer outside any allow region is impossible since
        // harness always allows the display buffer. Instead patch the image to
        // store at RAM_BASE (not allowed):
        let mut img = synthetic_image();
        img[0x8C..0x90].copy_from_slice(&RAM_BASE.to_le_bytes()); // store target
        let d = load_descriptor(DESC_JSON).unwrap();
        let mut h = Harness::new(&img, d);
        let err = h.run_frame(100_000).unwrap_err();
        assert!(matches!(err, EmuError::Descriptor(_)));
    }

    #[test]
    fn test_unpack_block1_layout() {
        let mut buf = vec![0u8; 64 * 128 / 8];
        buf[0] = 0b0000_0101; // y=0 and y=2 at x=0
        buf[64] = 0x80;       // second byte-row group: y=15 at x=0
        let f = unpack_block1(&buf, 64, 128);
        assert_eq!(f.pixels[0], 1);          // (0,0)
        assert_eq!(f.pixels[2 * 64], 1);     // (0,2)
        assert_eq!(f.pixels[15 * 64], 1);    // (0,15)
        assert_eq!(f.pixels[64], 0);         // (0,1)
    }
}
```

(Frame needs `#[derive(Clone)]` — add it.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --offline --lib firmware::emu::harness`
Expected: FAIL (compile error — types don't exist).

- [ ] **Step 3: Implement `harness.rs`** per Interfaces.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --offline --lib firmware::emu::harness`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/firmware/emu/harness.rs
git commit -m "feat(emu): descriptor-driven harness with frame extraction"
```

---

### Task 7: Integration — synthetic render function + real patch engine + hook simulation

**Files:**
- Create: `src-tauri/src/firmware/tests/emu_test.rs`
- Modify: `src-tauri/src/firmware/tests/mod.rs` (add `mod emu_test;`)

**Interfaces:**
- Consumes: Task 6 `Harness`, `load_descriptor`, `unpack_block1`, `EmuError`; `patch::{Patch, PatchModification, apply_patch}`.
- Produces: the layer-2/3 spec tests. No new library API.

This task proves the whole flow without any RE: a synthetic two-function "firmware" (render function with a natural loop + a BL'd helper), a **hook patch applied via the real `apply_patch`** (a 4-byte branch sequence written at the hook site that detours through a code cave), and the negative test (a buggy cave write → ACL error).

Synthetic image layout (flash offsets):
- `0x00`: render function — `push {r4, lr}`; zero the 16-byte display buffer via a loop (`strb` + `adds` + `cmp` + `bne`); `bl` helper at `0x40`; helper increments the phase global at `RAM_BASE` and returns pixel value = phase & 1; render stores it into `display_buffer[0]`; cave hook site at `0x20` (two `nop`s, 4 bytes); `pop {r4, pc}`.
- `0x40`: helper — `ldr r0, [pc, #imm]` (global addr), `ldr r1, [r0]`, `adds r1, #1`, `str r1, [r0]`, `ands r1, #1`... (`ands` is DataProc op 0: `ands r1, r1, r?` — instead `movs r0, #1; ands r1, r0` wrong regs — keep it concrete below in the test's halfword table with comments), `bx lr`.
- `0x60`: code cave (32 bytes of zeros).

The exact halfword table is written in the test with per-instruction comments; each encoding follows the patterns already unit-tested in Tasks 2–5 (so no new decode surface is exercised).

The hook patch (applied with `apply_patch` to the image before `Harness::new`):
- At `0x20`: `B` to cave (`0xE00x` unconditional branch, offset = `0x60 - (0x20+4) = 0x3C` → imm11 = 0x1E → halfword `0xE01E`) plus one filler `nop`.
- In the cave at `0x60`: `movs r0, #0xFF`; `ldr r1, [pc, #imm]` → display buffer addr; `strb r0, [r1, #15]` (last byte); `B` back to `0x24` (offset = `0x24 - (0x68+4) = -0x48` → imm11 = `(-0x24) & 0x7FF`... compute concretely in the test with a small `fn b_off(from: u32, to: u32) -> u16` helper that builds the halfword: `let off = (to as i64 - (from as i64 + 4)) / 2; 0xE000 | (off as u16 & 0x7FF)`).

```rust
use std::collections::HashMap;
use crate::firmware::emu::harness::{Harness, load_descriptor};
use crate::firmware::emu::EmuError;
use crate::firmware::patch::{Patch, PatchModification, apply_patch};

fn b_off(from: u32, to: u32) -> u16 {
    let off = (to as i64 - (from as i64 + 4)) / 2;
    0xE000 | ((off as i32 as u16) & 0x7FF)
}

fn hook_patch() -> Patch {
    let mut mods = Vec::new();
    let push = |mods: &mut Vec<PatchModification>, addr: u32, hw: u16| {
        let b = hw.to_le_bytes();
        mods.push(PatchModification { offset: addr as usize, original: None, patched: b[0] });
        mods.push(PatchModification { offset: addr as usize + 1, original: None, patched: b[1] });
    };
    push(&mut mods, 0x20, b_off(0x20, 0x60)); // hook: branch into cave
    push(&mut mods, 0x22, 0x46C0);            // nop filler
    push(&mut mods, 0x60, 0x20FF);            // movs r0, #0xFF
    push(&mut mods, 0x62, 0x4902);            // ldr r1, [pc, #8] -> base align(0x66,4)=0x64, +8 = 0x6C
    push(&mut mods, 0x64, 0x73C8);            // strb r0, [r1, #15]
    push(&mut mods, 0x66, b_off(0x66, 0x24)); // branch back after hook
    // literal at 0x6C (display buffer addr) — 0x68 is a nop pad:
    let addr = crate::firmware::emu::bus::RAM_BASE + 0x1000;
    push(&mut mods, 0x68, 0x46C0);            // nop (keeps pc-relative math simple)
    mods.push(PatchModification { offset: 0x6C, original: None, patched: (addr & 0xFF) as u8 });
    mods.push(PatchModification { offset: 0x6D, original: None, patched: ((addr >> 8) & 0xFF) as u8 });
    mods.push(PatchModification { offset: 0x6E, original: None, patched: ((addr >> 16) & 0xFF) as u8 });
    mods.push(PatchModification { offset: 0x6F, original: None, patched: ((addr >> 24) & 0xFF) as u8 });
    Patch { id: "anim-hook".into(), name: "anim".into(), ..Default::default() }
}

#[test]
fn test_hooked_render_frame() {
    let d = load_descriptor(SYNTH_DESC).unwrap(); // desc matching layout above
    let mut img = synthetic_image();              // builder fn in this file
    let mut p = hook_patch();
    let mut log = HashMap::new();
    apply_patch(&mut img, &mut p, &mut log).unwrap();
    let mut h = Harness::new(&img, d);
    let frame = h.run_frame(1_000_000).unwrap();
    assert_eq!(frame.pixels[15], 1); // byte 15 = x=15, y-group 0; bit0 -> (15,0) -> 15 + 0*64
}

#[test]
fn test_bad_patch_acl_violation() {
    let d = load_descriptor(SYNTH_DESC).unwrap();
    let mut img = synthetic_image();
    let mut p = hook_patch();
    // corrupt the literal: point display buffer at unallowed RAM
    for m in &mut p.modifications {
        if m.offset == 0x6E { m.patched = 0x04; } // addr becomes 0x2004_1000ish region not allowed
    }
    let mut log = HashMap::new();
    apply_patch(&mut img, &mut p, &mut log).unwrap();
    let mut h = Harness::new(&img, d);
    assert!(h.run_frame(1_000_000).is_err());
}

#[test]
fn test_two_frames_differ_via_phase() {
    let d = load_descriptor(SYNTH_DESC).unwrap();
    let img = synthetic_image();
    let mut h = Harness::new(&img, d);
    let f1 = h.run_frame(1_000_000).unwrap();
    let f2 = h.run_frame(1_000_000).unwrap();
    assert!(Harness::frames_differ(&f1, &f2)); // helper's phase counter flips pixel 0
}
```

(The `synthetic_image()` halfword table and `SYNTH_DESC` JSON are written out fully in this test file, mirroring the layout above; the pixel-index assertion uses `x + y*width` row-major math from `unpack_block1`.)

- [ ] **Step 1: Write the failing tests** — create `emu_test.rs` with the full synthetic image builder, `SYNTH_DESC`, the three tests above, and register `mod emu_test;` in `tests/mod.rs`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --offline --lib firmware::tests::emu_test`
Expected: FAIL (compile error or wrong pixels on first attempt — iterate until the hand-assembled image is right; this is where encoding mistakes in the synthetic image get weeded out, distinct from emulator bugs because every encoding used is already unit-tested).

- [ ] **Step 3: Fix the test image until green**

Debug loop: on failure, print `h.cpu.debug_dump()` and the `write_log` from the test (temporary `println!`, removed before commit).

- [ ] **Step 4: Run full emu suite**

Run: `cargo test --offline --lib firmware::emu && cargo test --offline --lib firmware::tests::emu_test`
Expected: PASS (all).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/firmware/tests/emu_test.rs src-tauri/src/firmware/tests/mod.rs
git commit -m "test(emu): synthetic render function with hook patch through real patch engine"
```

---

### Task 8: Real-descriptor gate test (layer 4) + goals update

**Files:**
- Modify: `src-tauri/src/firmware/tests/emu_test.rs`
- Modify: `docs/goals.md`

**Interfaces:**
- Consumes: everything above; `resources/animations/af_190602.json` (does not exist yet — produced by the Phase 3 Ghidra RE); `AF_fw/decrypted/af_190602.dec.bin` (gitignored, present on dev machines only).
- Produces: `test_af_190602_render_gate` (`#[ignore]`), the pre-flash gate.

- [ ] **Step 1: Write the gate test** (append to `emu_test.rs`)

```rust
/// LAYER-4 PRE-FLASH GATE. Requires:
///   - resources/animations/af_190602.json (Phase 3 RE output)
///   - AF_fw/decrypted/af_190602.dec.bin (gitignored RE artifact)
/// Both absent in CI/other machines -> skip. Run explicitly before flashing
/// any animation image:
///   cargo test --offline --lib firmware::tests::emu_test -- --ignored
#[test]
#[ignore]
fn test_af_190602_render_gate() {
    use std::path::Path;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let desc_path = root.join("resources/animations/af_190602.json");
    let img_path = root.join("AF_fw/decrypted/af_190602.dec.bin");
    if !desc_path.exists() || !img_path.exists() {
        eprintln!("descriptor or decrypted image missing; skipping gate");
        return;
    }
    let desc = load_descriptor(&std::fs::read_to_string(&desc_path).unwrap()).unwrap();
    let img = std::fs::read(&img_path).unwrap();
    let mut h = Harness::new(&img, desc);
    let mut prev = None;
    for frame_no in 0..4 {
        let frame = h.run_frame(5_000_000)
            .unwrap_or_else(|e| panic!("frame {frame_no}: {e}\n{}", h.cpu.debug_dump()));
        if let Some(p) = &prev {
            // stock charge screen may be static; just record. Animation patches
            // assert difference in the effect tests (later phase).
            let _ = Harness::frames_differ(p, &frame);
        }
        prev = Some(frame);
    }
}
```

- [ ] **Step 2: Run it**

Run: `cargo test --offline --lib firmware::tests::emu_test::test_af_190602_render_gate -- --ignored`
Expected: PASS with "skipping gate" (descriptor absent) — proves the skip path compiles and runs.

- [ ] **Step 3: Update `docs/goals.md`**

Append a new loose end:

```markdown
- [ ] Emulation harness gate: layers 1–3 done (plan
      `docs/superpowers/plans/2026-08-28-firmware-emulation-harness.md`).
      Layer 4 (`test_af_190602_render_gate`) unblocks when the Phase 3 RE
      produces `resources/animations/af_190602.json`; layer 5 (effect golden
      frames + differential vs Rust reference math) lands with the Thumb
      bytecode builder in the animation-effects work.
```

- [ ] **Step 4: Run the complete library suite**

Run: `cargo test --offline --lib`
Expected: PASS — all pre-existing tests plus all emu tests.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/firmware/tests/emu_test.rs docs/goals.md
git commit -m "test(emu): real-descriptor pre-flash gate (ignored until RE lands)"
```

---

## Out of scope (deliberate)

- **Layer 5 effect golden frames + differential testing** — needs the Thumb bytecode builder and Gradient Fade effect (Phase 3 animation work), not this plan.
- **Boot-level emulation** (reset vector, peripheral model) — documented in the spec as a possible later upgrade.
- **The bytecode builder itself** — separate Phase 3 deliverable.

## Self-Review

**Spec coverage:** bus/ACL/stubs → Task 1; cpu/thumb interpreter → Tasks 2–5; harness/sentinel/budget/frame extraction → Task 6; layer-2 self-check → Tasks 2–5 unit tests + Task 7 synthetic functions; layer-3 synthetic-image integration → Task 7; layer-4 real-descriptor gate → Task 8; error context (debug_dump, trace, ACL detail) → Tasks 2/6; PGM frame dumps → Task 6 (`dump_pgm`); exclusions respected (no Thumb-2, no boot, no app wiring). Layer 5 deferred with reason (builder not in tree — the spec's dependency note was wrong).

**Placeholder scan:** synthetic-image halfword tables in Task 7 are specified by layout + builder helper + comments; the executor writes the exact constants following unit-tested encoding patterns — every encoding used there has a passing unit test by then. All other code blocks are complete.

**Type consistency:** `EmuError`, `Bus` (methods + public log fields), `Cpu` (`reg/set_reg/step/run_until/debug_dump/trace`), `Instr` variant names/fields, `Descriptor`/`AddrRange`/`DisplayBuffer`/`Frame`, `Harness::{new,run_frame,frames_differ}`, `RETURN_SENTINEL`, `load_descriptor`, `unpack_block1`, `dump_pgm`, `b_off` are used identically across tasks. `de_hex` is referenced in serde attributes as a plain path within the same module.
