//! ARMv6-M CPU core: register file, flags, fetch/decode/execute step,
//! and a budgeted run loop.

use super::bus::Bus;
use super::thumb::{decode, Instr};
use super::EmuError;
use std::collections::VecDeque;

const TRACE_CAP: usize = 32;

pub struct Cpu {
    pub r: [u32; 13],
    pub sp: u32,
    pub lr: u32,
    pub pc: u32, // address of the instruction being executed (even)
    pub n: bool,
    pub z: bool,
    pub c: bool,
    pub v: bool,
    pub trace: VecDeque<u32>, // last 32 executed PCs
}

impl Cpu {
    pub fn new() -> Self {
        Self {
            r: [0; 13],
            sp: 0,
            lr: 0,
            pc: 0,
            n: false,
            z: false,
            c: false,
            v: false,
            trace: VecDeque::new(),
        }
    }

    pub fn reg(&self, i: u8) -> u32 {
        match i {
            13 => self.sp,
            14 => self.lr,
            15 => self.pc.wrapping_add(4),
            _ => self.r[i as usize],
        }
    }

    pub fn set_reg(&mut self, i: u8, v: u32) {
        match i {
            13 => self.sp = v,
            14 => self.lr = v,
            15 => self.pc = v & !1,
            _ => self.r[i as usize] = v,
        }
    }

    /// Fetch/decode/execute one instruction at `pc`.
    pub fn step(&mut self, bus: &mut Bus) -> Result<(), EmuError> {
        self.trace.push_back(self.pc);
        if self.trace.len() > TRACE_CAP {
            self.trace.pop_front();
        }
        let pc = self.pc;
        let hw = bus.read_u16(pc)?;
        let instr = decode(hw).ok_or(EmuError::Undefined { pc, instr: hw })?;
        self.execute(bus, instr)
    }

    /// Step until `pc == stop_pc` (returns instructions executed) or the
    /// budget is exhausted.
    pub fn run_until(
        &mut self,
        bus: &mut Bus,
        stop_pc: u32,
        budget: u64,
    ) -> Result<u64, EmuError> {
        let mut n: u64 = 0;
        loop {
            if self.pc == stop_pc {
                return Ok(n);
            }
            self.step(bus)?;
            n += 1;
            if n >= budget {
                return Err(EmuError::BudgetExceeded { executed: n });
            }
        }
    }

    /// Registers + flags + trace ring, for failure reports.
    pub fn debug_dump(&self) -> String {
        let mut s = String::new();
        for i in 0..13u8 {
            s.push_str(&format!("r{i:<2} = {:#010x}\n", self.r[i as usize]));
        }
        s.push_str(&format!("sp  = {:#010x}\n", self.sp));
        s.push_str(&format!("lr  = {:#010x}\n", self.lr));
        s.push_str(&format!("pc  = {:#010x}\n", self.pc));
        s.push_str(&format!(
            "n={} z={} c={} v={}\n",
            self.n as u8, self.z as u8, self.c as u8, self.v as u8
        ));
        s.push_str("trace:");
        for pc in &self.trace {
            s.push_str(&format!(" {pc:#010x}"));
        }
        s
    }

    fn execute(&mut self, _bus: &mut Bus, instr: Instr) -> Result<(), EmuError> {
        match instr {
            Instr::LslImm { rd, rm, imm } => {
                let res = self.lsl_c(self.r[rm as usize], imm as u32);
                self.set_nz(res);
                self.r[rd as usize] = res;
                self.pc = self.pc.wrapping_add(2);
            }
            Instr::LsrImm { rd, rm, imm } => {
                let res = self.lsr_c(self.r[rm as usize], imm as u32);
                self.set_nz(res);
                self.r[rd as usize] = res;
                self.pc = self.pc.wrapping_add(2);
            }
            Instr::AsrImm { rd, rm, imm } => {
                let res = self.asr_c(self.r[rm as usize], imm as u32);
                self.set_nz(res);
                self.r[rd as usize] = res;
                self.pc = self.pc.wrapping_add(2);
            }
            Instr::AddReg { rd, rn, rm } => {
                let res = self.add_flags(self.r[rn as usize], self.r[rm as usize], false);
                self.r[rd as usize] = res;
                self.pc = self.pc.wrapping_add(2);
            }
            Instr::SubReg { rd, rn, rm } => {
                let res = self.sub_flags(self.r[rn as usize], self.r[rm as usize], true);
                self.r[rd as usize] = res;
                self.pc = self.pc.wrapping_add(2);
            }
            Instr::AddImm3 { rd, rn, imm } => {
                let res = self.add_flags(self.r[rn as usize], imm as u32, false);
                self.r[rd as usize] = res;
                self.pc = self.pc.wrapping_add(2);
            }
            Instr::SubImm3 { rd, rn, imm } => {
                let res = self.sub_flags(self.r[rn as usize], imm as u32, true);
                self.r[rd as usize] = res;
                self.pc = self.pc.wrapping_add(2);
            }
            Instr::MovImm { rd, imm } => {
                self.set_nz(imm as u32);
                self.r[rd as usize] = imm as u32; // C unchanged
                self.pc = self.pc.wrapping_add(2);
            }
            Instr::CmpImm { rn, imm } => {
                self.sub_flags(self.r[rn as usize], imm as u32, true);
                self.pc = self.pc.wrapping_add(2);
            }
            Instr::AddImm8 { rdn, imm } => {
                let res = self.add_flags(self.r[rdn as usize], imm as u32, false);
                self.r[rdn as usize] = res;
                self.pc = self.pc.wrapping_add(2);
            }
            Instr::SubImm8 { rdn, imm } => {
                let res = self.sub_flags(self.r[rdn as usize], imm as u32, true);
                self.r[rdn as usize] = res;
                self.pc = self.pc.wrapping_add(2);
            }
            Instr::B { off } => {
                self.pc = (self.pc as i32 + 4 + off) as u32;
            }
        }
        Ok(())
    }

    // ---- flag / shift helpers (shared by all decode groups) ----

    fn set_nz(&mut self, r: u32) {
        self.n = r >> 31 != 0;
        self.z = r == 0;
    }

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

    fn lsl_c(&mut self, v: u32, s: u32) -> u32 {
        // updates C (and NZ by callers)
        if s == 0 {
            v
        } else if s < 32 {
            self.c = v >> (32 - s) & 1 != 0;
            v << s
        } else if s == 32 {
            self.c = v & 1 != 0;
            0
        } else {
            self.c = false;
            0
        }
    }

    fn lsr_c(&mut self, v: u32, s: u32) -> u32 {
        // s==0 means 32 (imm encodings)
        let s = if s == 0 { 32 } else { s };
        if s < 32 {
            self.c = v >> (s - 1) & 1 != 0;
            v >> s
        } else if s == 32 {
            self.c = v >> 31 != 0;
            0
        } else {
            self.c = false;
            0
        }
    }

    fn asr_c(&mut self, v: u32, s: u32) -> u32 {
        // s==0 means 32
        let s = if s == 0 { 32 } else { s };
        if s >= 32 {
            self.c = v >> 31 != 0;
            if self.c {
                u32::MAX
            } else {
                0
            }
        } else {
            self.c = v >> (s - 1) & 1 != 0;
            ((v as i32) >> s) as u32
        }
    }
}

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
        cpu.r[0] = 3;
        cpu.r[1] = 5;
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
