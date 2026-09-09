//! ARMv6-M CPU core: register file, flags, fetch/decode/execute step,
//! and a budgeted run loop.

use super::bus::Bus;
use super::thumb::{decode, decode32, DpOp, DpRegOp, Instr, Thumb2};
use super::EmuError;
use std::collections::VecDeque;

const TRACE_CAP: usize = 32;

/// Variable-shift shifter (register-controlled amount, 0..255) with carry-out.
fn shift_reg(value: u32, stype: u8, amount: u32, carry_in: bool) -> (u32, bool) {
    match stype {
        0 => {
            // LSL
            if amount == 0 {
                (value, carry_in)
            } else if amount < 32 {
                (value << amount, (value >> (32 - amount)) & 1 == 1)
            } else if amount == 32 {
                (0, value & 1 == 1)
            } else {
                (0, false)
            }
        }
        1 => {
            // LSR
            if amount == 0 || amount == 32 {
                (0, value >> 31 != 0)
            } else if amount < 32 {
                (value >> amount, (value >> (amount - 1)) & 1 == 1)
            } else {
                (0, false)
            }
        }
        2 => {
            // ASR
            if amount >= 32 {
                (((value as i32) >> 31) as u32, value >> 31 != 0)
            } else {
                let v = ((value as i32) >> amount) as u32;
                (v, (v >> (amount - 1)) & 1 == 1)
            }
        }
        _ => {
            // ROR (amount 0 = no shift; RRX is not encodable here)
            if amount == 0 {
                (value, carry_in)
            } else {
                let v = value.rotate_right(amount & 31);
                (v, v >> 31 != 0)
            }
        }
    }
}

/// Barrel shifter with carry-out (Thumb-2 DpReg forms). `stype`:
/// 0=LSL, 1=LSR, 2=ASR, 3=ROR/RRX. Encoding amount 0 means LSR/ASR #32 or RRX.
fn shift_c(value: u32, stype: u8, amount: u8, carry_in: bool) -> (u32, bool) {
    let a = amount as u32;
    match stype {
        0 => {
            if a == 0 {
                (value, carry_in)
            } else if a < 32 {
                (value << a, (value >> (32 - a)) & 1 == 1)
            } else {
                (0, false)
            }
        }
        1 => {
            let s = if a == 0 { 32 } else { a };
            if s == 32 {
                (0, value >> 31 != 0)
            } else if s < 32 {
                (value >> s, (value >> (s - 1)) & 1 == 1)
            } else {
                (0, false)
            }
        }
        2 => {
            let s = if a == 0 { 32 } else { a };
            let v = ((value as i32) >> s.min(31)) as u32;
            let c = if s >= 32 { value >> 31 != 0 } else { (v >> (s - 1)) & 1 == 1 };
            (v, c)
        }
        _ => {
            if a == 0 {
                // RRX
                ((value >> 1) | ((carry_in as u32) << 31), value & 1 == 1)
            } else {
                let v = value.rotate_right(a & 31);
                (v, v >> 31 != 0)
            }
        }
    }
}

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
    /// Deepest sp value seen; the harness compares it against the bottom of
    /// the stack region to catch stack exhaustion even when no store landed
    /// below the region (e.g. `sub sp, #big` then return).
    pub min_sp: u32,
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
            min_sp: u32::MAX,
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
        if hw >> 11 == 0b11110 {
            // 32-bit prefix: either BL (11110) or other 32-bit Thumb-2.
            // Read the second halfword to decide which family it is.
            let hw2 = bus.read_u16(pc.wrapping_add(2))?;
            // BL encoding: hw2 bits 15,14,12 are all 1 (11J1 1 J2 imm11).
            if hw2 & 0xD000 == 0xD000 {
                let s = (hw >> 10) & 1;
                let j1 = (hw2 >> 13) & 1;
                let j2 = (hw2 >> 11) & 1;
                let i1 = (!(j1 ^ s)) & 1;
                let i2 = (!(j2 ^ s)) & 1;
                let imm25 = ((s as u32) << 24) | ((i1 as u32) << 23) | ((i2 as u32) << 22)
                    | (((hw & 0x3FF) as u32) << 12) | (((hw2 & 0x7FF) as u32) << 1);
                let off = ((imm25 << 7) as i32) >> 7; // sign-extend 25 bits
                return self.execute(bus, Instr::Bl { off });
            }
            // Otherwise: 32-bit data-processing or other 32-bit Thumb-2.
            let instr = decode32(hw, hw2).ok_or(EmuError::Undefined { pc, instr: hw })?;
            return self.execute(bus, instr);
        }
        if hw >> 11 == 0b11101 || hw >> 11 == 0b11111 {
            // Other 32-bit Thumb-2 instructions.
            let hw2 = bus.read_u16(pc.wrapping_add(2))?;
            let instr = decode32(hw, hw2).ok_or(EmuError::Undefined { pc, instr: hw })?;
            return self.execute(bus, instr);
        }
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
            if self.sp < self.min_sp {
                self.min_sp = self.sp;
            }
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

    fn execute(&mut self, bus: &mut Bus, instr: Instr) -> Result<(), EmuError> {
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
            Instr::DataProc { op, rdn, rm } => {
                let a = self.r[rdn as usize];
                let b = self.r[rm as usize];
                match op {
                    0 => {
                        // AND
                        let res = a & b;
                        self.set_nz(res);
                        self.r[rdn as usize] = res;
                    }
                    1 => {
                        // EOR
                        let res = a ^ b;
                        self.set_nz(res);
                        self.r[rdn as usize] = res;
                    }
                    2 => {
                        // LSL by register: low byte of rm is the amount; 0 = no shift
                        let res = self.lsl_c(a, b & 0xFF);
                        self.set_nz(res);
                        self.r[rdn as usize] = res;
                    }
                    3 => {
                        // LSR by register: 0 = no shift (unlike imm form, where 0 = 32)
                        let s = b & 0xFF;
                        let res = if s == 0 { a } else { self.lsr_c(a, s) };
                        self.set_nz(res);
                        self.r[rdn as usize] = res;
                    }
                    4 => {
                        // ASR by register: 0 = no shift
                        let s = b & 0xFF;
                        let res = if s == 0 { a } else { self.asr_c(a, s) };
                        self.set_nz(res);
                        self.r[rdn as usize] = res;
                    }
                    5 => {
                        // ADC
                        let res = self.add_flags(a, b, self.c);
                        self.r[rdn as usize] = res;
                    }
                    6 => {
                        // SBC
                        let res = self.sub_flags(a, b, self.c);
                        self.r[rdn as usize] = res;
                    }
                    7 => {
                        // ROR by register
                        let s0 = b & 0xFF;
                        let res = if s0 == 0 {
                            a // C unchanged
                        } else {
                            let s = s0 & 31;
                            if s == 0 {
                                self.c = a >> 31 != 0;
                                a
                            } else {
                                let res = a.rotate_right(s);
                                self.c = res >> 31 != 0;
                                res
                            }
                        };
                        self.set_nz(res);
                        self.r[rdn as usize] = res;
                    }
                    8 => {
                        // TST: discard result, C unchanged (no shifter here)
                        self.set_nz(a & b);
                    }
                    9 => {
                        // RSB (negate): rdn = 0 - rm
                        let res = self.sub_flags(0, b, true);
                        self.r[rdn as usize] = res;
                    }
                    10 => {
                        // CMP
                        self.sub_flags(a, b, true);
                    }
                    11 => {
                        // CMN
                        self.add_flags(a, b, false);
                    }
                    12 => {
                        // ORR
                        let res = a | b;
                        self.set_nz(res);
                        self.r[rdn as usize] = res;
                    }
                    13 => {
                        // MUL: sets N,Z; C,V unchanged
                        let res = a.wrapping_mul(b);
                        self.set_nz(res);
                        self.r[rdn as usize] = res;
                    }
                    14 => {
                        // BIC
                        let res = a & !b;
                        self.set_nz(res);
                        self.r[rdn as usize] = res;
                    }
                    _ => {
                        // 15: MVN
                        let res = !b;
                        self.set_nz(res);
                        self.r[rdn as usize] = res;
                    }
                }
                self.pc = self.pc.wrapping_add(2);
            }
            Instr::AddHi { rd, rm } => {
                // no flags; writes to pc branch (set_reg masks bit0)
                let res = self.reg(rd).wrapping_add(self.reg(rm));
                self.set_reg(rd, res);
                if rd != 15 {
                    self.pc = self.pc.wrapping_add(2);
                }
            }
            Instr::CmpHi { rn, rm } => {
                self.sub_flags(self.reg(rn), self.reg(rm), true);
                self.pc = self.pc.wrapping_add(2);
            }
            Instr::MovHi { rd, rm } => {
                // no flags
                let v = self.reg(rm);
                self.set_reg(rd, v);
                if rd != 15 {
                    self.pc = self.pc.wrapping_add(2);
                }
            }
            Instr::Bx { rm } => {
                self.pc = self.reg(rm) & !1;
            }
            Instr::Blx { rm } => {
                let target = self.reg(rm) & !1; // bit0 is the Thumb mode bit
                self.lr = self.pc.wrapping_add(2) | 1;
                self.pc = target;
            }
            Instr::LdrLit { rt, imm } => {
                // no flags; base is the aligned pc+4
                let base = self.pc.wrapping_add(4) & !3;
                let addr = base.wrapping_add((imm as u32) * 4);
                let v = bus.read_u32(addr)?;
                self.r[rt as usize] = v;
                self.pc = self.pc.wrapping_add(2);
            }
            Instr::LsReg { op, rt, rn, rm } => {
                // no flags
                let addr = self.r[rn as usize].wrapping_add(self.r[rm as usize]);
                match op {
                    0 => bus.write_u32(addr, self.r[rt as usize])?,
                    1 => bus.write_u16(addr, self.r[rt as usize] as u16)?,
                    2 => bus.write_u8(addr, self.r[rt as usize] as u8)?,
                    3 => self.r[rt as usize] = bus.read_u8(addr)? as i8 as i32 as u32,
                    4 => self.r[rt as usize] = bus.read_u32(addr)?,
                    5 => self.r[rt as usize] = bus.read_u16(addr)? as u32,
                    6 => self.r[rt as usize] = bus.read_u8(addr)? as u32,
                    _ => self.r[rt as usize] = bus.read_u16(addr)? as i16 as i32 as u32, // 7: LDRSH
                }
                self.pc = self.pc.wrapping_add(2);
            }
            Instr::LsImm { load, byte, rt, rn, imm } => {
                // no flags
                let addr = self.r[rn as usize].wrapping_add((imm as u32) * (if byte { 1 } else { 4 }));
                match (load, byte) {
                    (false, false) => bus.write_u32(addr, self.r[rt as usize])?,
                    (false, true) => bus.write_u8(addr, self.r[rt as usize] as u8)?,
                    (true, false) => self.r[rt as usize] = bus.read_u32(addr)?,
                    (true, true) => self.r[rt as usize] = bus.read_u8(addr)? as u32,
                }
                self.pc = self.pc.wrapping_add(2);
            }
            Instr::LshImm { load, rt, rn, imm } => {
                // no flags
                let addr = self.r[rn as usize].wrapping_add((imm as u32) * 2);
                if load {
                    self.r[rt as usize] = bus.read_u16(addr)? as u32;
                } else {
                    bus.write_u16(addr, self.r[rt as usize] as u16)?;
                }
                self.pc = self.pc.wrapping_add(2);
            }
            Instr::LsSp { load, rt, imm } => {
                // no flags
                let addr = self.sp.wrapping_add((imm as u32) * 4);
                if load {
                    self.r[rt as usize] = bus.read_u32(addr)?;
                } else {
                    bus.write_u32(addr, self.r[rt as usize])?;
                }
                self.pc = self.pc.wrapping_add(2);
            }
            Instr::Adr { rd, imm } => {
                // no flags; base is the aligned pc+4
                self.r[rd as usize] = (self.pc.wrapping_add(4) & !3).wrapping_add((imm as u32) * 4);
                self.pc = self.pc.wrapping_add(2);
            }
            Instr::AddSpImm { rd, imm } => {
                // no flags
                self.r[rd as usize] = self.sp.wrapping_add((imm as u32) * 4);
                self.pc = self.pc.wrapping_add(2);
            }
            Instr::AdjSp { sub, imm } => {
                // no flags
                let off = (imm as u32) * 4;
                if sub {
                    self.sp = self.sp.wrapping_sub(off);
                } else {
                    self.sp = self.sp.wrapping_add(off);
                }
                self.pc = self.pc.wrapping_add(2);
            }
            Instr::B { off } => {
                self.pc = (self.pc as i32).wrapping_add(4).wrapping_add(off) as u32;
            }
            Instr::Push { list, lr } => {
                // no flags; full descending stack: decrement first, store ascending
                let count = list.count_ones() + lr as u32;
                let mut addr = self.sp.wrapping_sub(4 * count);
                self.sp = addr;
                for i in 0..8u8 {
                    if list >> i & 1 == 1 {
                        bus.write_u32(addr, self.r[i as usize])?;
                        addr = addr.wrapping_add(4);
                    }
                }
                if lr {
                    bus.write_u32(addr, self.lr)?;
                }
                self.pc = self.pc.wrapping_add(2);
            }
            Instr::Pop { list, pc } => {
                // no flags; load ascending, then pop PC last (bit0 masked)
                let mut addr = self.sp;
                for i in 0..8u8 {
                    if list >> i & 1 == 1 {
                        self.r[i as usize] = bus.read_u32(addr)?;
                        addr = addr.wrapping_add(4);
                    }
                }
                let new_pc = if pc {
                    let v = bus.read_u32(addr)?;
                    addr = addr.wrapping_add(4);
                    Some(v & !1)
                } else {
                    None
                };
                self.sp = addr;
                self.pc = new_pc.unwrap_or_else(|| self.pc.wrapping_add(2));
            }
            Instr::Stm { rn, list } => {
                // no flags; store ascending, writeback rn += 4*count
                let mut addr = self.r[rn as usize];
                for i in 0..8u8 {
                    if list >> i & 1 == 1 {
                        bus.write_u32(addr, self.r[i as usize])?;
                        addr = addr.wrapping_add(4);
                    }
                }
                self.r[rn as usize] = addr;
                self.pc = self.pc.wrapping_add(2);
            }
            Instr::Ldm { rn, list } => {
                // no flags; load ascending, writeback unless rn is in the list
                let mut addr = self.r[rn as usize];
                for i in 0..8u8 {
                    if list >> i & 1 == 1 {
                        self.r[i as usize] = bus.read_u32(addr)?;
                        addr = addr.wrapping_add(4);
                    }
                }
                if list >> rn & 1 == 0 {
                    self.r[rn as usize] = addr;
                }
                self.pc = self.pc.wrapping_add(2);
            }
            Instr::Thumb2(Thumb2::Stmdb { list }) => {
                // 32-bit PUSH: STMDB sp!, {list}. list bits 0..14 = r0..r14.
                let count = list.count_ones();
                let mut addr = self.sp.wrapping_sub(4 * count);
                self.sp = addr;
                for i in 0..15u16 {
                    if list >> i & 1 == 1 {
                        bus.write_u32(addr, self.reg(i as u8))?;
                        addr = addr.wrapping_add(4);
                    }
                }
                self.pc = self.pc.wrapping_add(4);
            }
            Instr::Thumb2(Thumb2::Ldmia { list }) => {
                // 32-bit POP: LDMIA sp!, {list}. list bits 0..14 = r0..r14,
                // bit 15 = pc (pop.w {..., pc} returns; the word is consumed
                // from the stack either way).
                let mut addr = self.sp;
                let mut new_pc = None;
                for i in 0..16u16 {
                    if list >> i & 1 == 1 {
                        let v = bus.read_u32(addr)?;
                        if i == 15 {
                            new_pc = Some(v & !1);
                        } else {
                            self.set_reg(i as u8, v);
                        }
                        addr = addr.wrapping_add(4);
                    }
                }
                self.sp = addr;
                self.pc = new_pc.unwrap_or_else(|| self.pc.wrapping_add(4));
            }
            Instr::Thumb2(Thumb2::StmIA { rn, list }) => {
                // STMIA Rn, {list}: ascending word stores from Rn, NO
                // writeback (the W=0 store-multiple form, e.g.
                // `stm.w sp, {r7, sl}` = E88D 0480). list bits 0..14 = r0..r14.
                let mut addr = self.reg(rn);
                for i in 0..15u16 {
                    if list >> i & 1 == 1 {
                        bus.write_u32(addr, self.reg(i as u8))?;
                        addr = addr.wrapping_add(4);
                    }
                }
                self.pc = self.pc.wrapping_add(4);
            }
            Instr::Thumb2(Thumb2::DpImm { op, rd, rn, set_flags, imm }) => {
                // 32-bit data-processing with modified immediate.
                // MOV (immediate) is the ORR encoding with Rn = 1111; the
                // Rn operand is not read (no PC-relative OR); result = imm.
                let lhs = if rn == 15 && matches!(op, DpOp::Orr) { 0 } else { self.reg(rn) };
                let (result, carry, overflow) = match op {
                    DpOp::And => (lhs & imm, (lhs & imm) >> 31 != 0, false),
                    DpOp::Orr => (lhs | imm, (lhs | imm) >> 31 != 0, false),
                    DpOp::Eor => (lhs ^ imm, (lhs ^ imm) >> 31 != 0, false),
                    DpOp::Bic => (lhs & !imm, (lhs & !imm) >> 31 != 0, false),
                    DpOp::Add => {
                        let r = lhs.wrapping_add(imm);
                        let c = (lhs as u64 + imm as u64) > 0xFFFF_FFFF;
                        let v = ((lhs ^ r) & (imm ^ r)) >> 31 != 0;
                        (r, c, v)
                    }
                    DpOp::Sub => {
                        let r = lhs.wrapping_sub(imm);
                        let c = lhs >= imm;
                        let v = ((lhs ^ imm) & (lhs ^ r)) >> 31 != 0;
                        (r, c, v)
                    }
                    DpOp::Rsb => {
                        let r = imm.wrapping_sub(lhs);
                        let c = imm >= lhs;
                        let v = ((imm ^ lhs) & (imm ^ r)) >> 31 != 0;
                        (r, c, v)
                    }
                };
                if rd != 15 {
                    self.set_reg(rd, result);
                }
                if set_flags {
                    self.n = (result >> 31) != 0;
                    self.z = result == 0;
                    self.c = carry;
                    self.v = overflow;
                }
                self.pc = self.pc.wrapping_add(4);
            }
            Instr::Thumb2(Thumb2::DpReg { op, rd, rn, rm, stype, amount, set_flags }) => {
                // 32-bit data-processing (shifted register). MOV/MVN are the
                // ORR/ORN encodings with Rn = 1111: the Rn operand is not
                // read (no PC read), result = shifted Rm / its complement.
                let (shifted, sh_carry) = shift_c(self.reg(rm), stype, amount, self.c);
                // MOV/MVN (Orr/Orn with rn = 15) do not read the Rn operand.
                let lhs = if rn == 15 && matches!(op, DpRegOp::Orr | DpRegOp::Orn) { 0 } else { self.reg(rn) };
                let (result, carry, overflow) = match op {
                    DpRegOp::And => (lhs & shifted, sh_carry, false),
                    DpRegOp::Bic => (lhs & !shifted, sh_carry, false),
                    DpRegOp::Orr => (lhs | shifted, sh_carry, false),
                    DpRegOp::Orn => (lhs | !shifted, sh_carry, false),
                    DpRegOp::Eor => (lhs ^ shifted, sh_carry, false),
                    DpRegOp::Add => {
                        let lhs = self.reg(rn);
                        let s = lhs as i64 + shifted as i64;
                        (lhs.wrapping_add(shifted), s > u32::MAX as i64, s > i32::MAX as i64 || s < i32::MIN as i64)
                    }
                    DpRegOp::Adc => {
                        let lhs = self.reg(rn);
                        let cin = self.c as i64;
                        let s = lhs as i64 + shifted as i64 + cin;
                        (lhs.wrapping_add(shifted).wrapping_add(cin as u32), s > u32::MAX as i64, s > i32::MAX as i64 || s < i32::MIN as i64)
                    }
                    DpRegOp::Sub => {
                        let lhs = self.reg(rn);
                        let d = lhs as i64 - shifted as i64;
                        (lhs.wrapping_sub(shifted), d >= 0, d > i32::MAX as i64 || d < i32::MIN as i64)
                    }
                    DpRegOp::Sbc => {
                        let lhs = self.reg(rn);
                        let borrow = 1 - self.c as i64;
                        let d = lhs as i64 - shifted as i64 - borrow;
                        (lhs.wrapping_sub(shifted).wrapping_sub(borrow as u32), d >= 0, d > i32::MAX as i64 || d < i32::MIN as i64)
                    }
                };
                if rd != 15 {
                    self.set_reg(rd, result);
                }
                if set_flags {
                    self.n = (result >> 31) != 0;
                    self.z = result == 0;
                    self.c = carry;
                    self.v = overflow;
                }
                self.pc = self.pc.wrapping_add(4);
            }
            Instr::Thumb2(Thumb2::MovShiftReg { rd, rn, rm, stype, set_flags }) => {
                // 32-bit MOV (register) with register-controlled shift:
                // Rd = Rn shifted by Rm[7:0]. No V flag.
                let amount = self.reg(rm) & 0xFF;
                let (result, carry) = shift_reg(self.reg(rn), stype, amount, self.c);
                self.set_reg(rd, result);
                if set_flags {
                    self.n = (result >> 31) != 0;
                    self.z = result == 0;
                    self.c = carry;
                }
                self.pc = self.pc.wrapping_add(4);
            }
            Instr::Thumb2(Thumb2::Movw { rd, imm16 }) => {
                self.set_reg(rd, imm16 as u32);
                self.pc = self.pc.wrapping_add(4);
            }
            Instr::Thumb2(Thumb2::Movt { rd, imm16 }) => {
                let v = (self.reg(rd) & 0xFFFF) | ((imm16 as u32) << 16);
                self.set_reg(rd, v);
                self.pc = self.pc.wrapping_add(4);
            }
            Instr::Thumb2(Thumb2::B { off }) => {
                // 32-bit unconditional branch B.W; same offset encoding as BL.
                self.pc = (self.pc as i32).wrapping_add(4).wrapping_add(off) as u32;
            }
            Instr::Thumb2(Thumb2::LdrLitW { rt, imm }) => {
                // 32-bit LDR (literal) T3: base is (pc+4) aligned to 4.
                let base = (self.pc.wrapping_add(4)) & !3;
                let addr = base.wrapping_add(imm as u32);
                let v = bus.read_u32(addr)?;
                self.set_reg(rt, v);
                self.pc = self.pc.wrapping_add(4);
            }
            Instr::Thumb2(Thumb2::Mul { rd, rn, rm, ra, sub }) => {
                // 32-bit MUL / MLA / MLS. Result = Ra +/- Rn*Rm (Ra = 15: MUL).
                let prod = self.reg(rn).wrapping_mul(self.reg(rm));
                let acc = self.reg(ra);
                let result = if ra == 15 {
                    prod
                } else if sub {
                    acc.wrapping_sub(prod)
                } else {
                    prod.wrapping_add(acc)
                };
                self.set_reg(rd, result);
                self.pc = self.pc.wrapping_add(4);
            }
            Instr::Thumb2(Thumb2::LdrhImm { rt, rn, imm }) => {
                // 32-bit LDRH (immediate): halfword load, zero-extended. No flags.
                let addr = self.reg(rn).wrapping_add(imm as u32);
                let v = bus.read_u16(addr)?;
                self.set_reg(rt, v as u32);
                self.pc = self.pc.wrapping_add(4);
            }
            Instr::Thumb2(Thumb2::StrImm { rt, rn, imm }) => {
                // 32-bit STR (immediate): word store. No flags.
                let addr = self.reg(rn).wrapping_add(imm as u32);
                bus.write_u32(addr, self.reg(rt))?;
                self.pc = self.pc.wrapping_add(4);
            }
            Instr::Thumb2(Thumb2::StrbT4 { rt, rn, imm, pre, sub }) => {
                // 32-bit STRB with writeback: pre => mem8[Rn ± imm] = Rt, Rn ±=
                // imm; post => mem8[Rn] = Rt, Rn ±= imm. No flags.
                let base = self.reg(rn);
                let addr = if sub { base.wrapping_sub(imm as u32) } else { base.wrapping_add(imm as u32) };
                if pre {
                    bus.write_u8(addr, (self.reg(rt) & 0xFF) as u8)?;
                    self.set_reg(rn, addr);
                } else {
                    bus.write_u8(base, (self.reg(rt) & 0xFF) as u8)?;
                    self.set_reg(rn, addr);
                }
                self.pc = self.pc.wrapping_add(4);
            }
            Instr::Thumb2(Thumb2::LdrbT4 { rt, rn, imm, pre, sub }) => {
                // 32-bit LDRB with writeback: same addressing as StrbT4, byte
                // load zero-extended into Rt. No flags.
                let base = self.reg(rn);
                let addr = if sub { base.wrapping_sub(imm as u32) } else { base.wrapping_add(imm as u32) };
                let v = bus.read_u8(if pre { addr } else { base })?;
                self.set_reg(rn, addr);
                self.set_reg(rt, v as u32);
                self.pc = self.pc.wrapping_add(4);
            }
            Instr::Thumb2(Thumb2::Sdiv { rd, rn, rm }) => {
                // Signed divide, truncated toward zero. Divide-by-zero yields
                // 0; INT_MIN / -1 yields INT_MIN (ARMv7-M saturation rule).
                let a = self.reg(rn) as i32;
                let b = self.reg(rm) as i32;
                let q = if b == 0 { 0 } else { a.wrapping_div(b) };
                self.set_reg(rd, q as u32);
                self.pc = self.pc.wrapping_add(4);
            }
            Instr::Thumb2(Thumb2::Udiv { rd, rn, rm }) => {
                // Unsigned divide. Divide-by-zero yields 0 (ARMv7-M).
                let a = self.reg(rn);
                let b = self.reg(rm);
                let q = if b == 0 { 0 } else { a / b };
                self.set_reg(rd, q);
                self.pc = self.pc.wrapping_add(4);
            }
            Instr::Thumb2(Thumb2::LdrbImm { rt, rn, imm }) => {
                // 32-bit LDRB (immediate): byte load, zero-extended. No flags.
                let addr = self.reg(rn).wrapping_add(imm as u32);
                let v = bus.read_u8(addr)?;
                self.set_reg(rt, v as u32);
                self.pc = self.pc.wrapping_add(4);
            }
            Instr::Thumb2(Thumb2::StrbImm { rt, rn, imm }) => {
                // 32-bit STRB (immediate) T3: byte store. No flags.
                let addr = self.reg(rn).wrapping_add(imm as u32);
                bus.write_u8(addr, (self.reg(rt) & 0xFF) as u8)?;
                self.pc = self.pc.wrapping_add(4);
            }
            Instr::Thumb2(Thumb2::StrhImm { rt, rn, imm }) => {
                // 32-bit STRH (immediate) T3: halfword store. No flags.
                let addr = self.reg(rn).wrapping_add(imm as u32);
                bus.write_u16(addr, (self.reg(rt) & 0xFFFF) as u16)?;
                self.pc = self.pc.wrapping_add(4);
            }
            Instr::Thumb2(Thumb2::LdrImm { rt, rn, imm }) => {
                // 32-bit LDR (immediate): word load. No flags.
                let addr = self.reg(rn).wrapping_add(imm as u32);
                let v = bus.read_u32(addr)?;
                self.set_reg(rt, v);
                self.pc = self.pc.wrapping_add(4);
            }
            Instr::Thumb2(Thumb2::Sbfx { rd, rn, lsbit, width }) => {
                // Sign-extending bit-field extract: take `width` bits of Rn
                // starting at `lsbit`, sign-extend to 32 bits. No flags.
                let v = self.reg(rn);
                let mask = if width >= 32 { u32::MAX } else { (1u32 << width) - 1 };
                let field = (v >> lsbit) & mask;
                let sign = 1u32 << (width - 1);
                let result = (field ^ sign).wrapping_sub(sign);
                self.set_reg(rd, result);
                self.pc = self.pc.wrapping_add(4);
            }
            Instr::Thumb2(Thumb2::Ubfx { rd, rn, lsbit, width }) => {
                // Zero-extending bit-field extract: take `width` bits of Rn
                // starting at `lsbit`, zero-extend to 32 bits. No flags.
                let v = self.reg(rn);
                let mask = if width >= 32 { u32::MAX } else { (1u32 << width) - 1 };
                self.set_reg(rd, (v >> lsbit) & mask);
                self.pc = self.pc.wrapping_add(4);
            }
            Instr::Thumb2(Thumb2::LdrStrReg { load, size, rt, rn, rm, shift }) => {
                // 32-bit LDR/STR (register): positive LSL offset, no writeback.
                // Byte offset = Rm << shift. No flags.
                let addr = self.reg(rn).wrapping_add(self.reg(rm) << shift);
                if load {
                    let v = match size {
                        1 => bus.read_u8(addr)? as u32,
                        2 => bus.read_u16(addr)? as u32,
                        _ => bus.read_u32(addr)?,
                    };
                    self.set_reg(rt, v);
                } else {
                    match size {
                        1 => bus.write_u8(addr, (self.reg(rt) & 0xFF) as u8)?,
                        2 => bus.write_u16(addr, (self.reg(rt) & 0xFFFF) as u16)?,
                        _ => bus.write_u32(addr, self.reg(rt))?,
                    }
                }
                self.pc = self.pc.wrapping_add(4);
            }
            Instr::BCond { cond, off } => {
                if self.cond_true(cond) {
                    self.pc = (self.pc as i32).wrapping_add(4).wrapping_add(off) as u32;
                } else {
                    self.pc = self.pc.wrapping_add(2);
                }
            }
            Instr::Cbz { nonzero, rn, off } => {
                // no flags; branch when (rn != 0) == nonzero
                if (self.r[rn as usize] != 0) == nonzero {
                    self.pc = (self.pc as i32).wrapping_add(4).wrapping_add(off) as u32;
                } else {
                    self.pc = self.pc.wrapping_add(2);
                }
            }
            Instr::Bl { off } => {
                // lr/pc update lives here; step() decodes the two halfwords
                self.lr = self.pc.wrapping_add(4) | 1;
                self.pc = (self.pc as i32).wrapping_add(4).wrapping_add(off) as u32;
            }
            Instr::Svc { .. } | Instr::Bkpt { .. } => {
                // no meaning in this harness; just advance
                self.pc = self.pc.wrapping_add(2);
            }
            Instr::Extend { op, rd, rm } => {
                // no flags; 0=SXTH, 1=SXTB, 2=UXTH, 3=UXTB
                let v = self.r[rm as usize];
                self.r[rd as usize] = match op {
                    0 => v as i16 as i32 as u32,
                    1 => v as i8 as i32 as u32,
                    2 => v & 0xFFFF,
                    _ => v & 0xFF,
                };
                self.pc = self.pc.wrapping_add(2);
            }
            Instr::Rev { op, rd, rm } => {
                // no flags; 0=REV, 1=REV16, 3=REVSH
                let v = self.r[rm as usize];
                self.r[rd as usize] = match op {
                    0 => v.swap_bytes(),
                    1 => (v << 8 & 0xFF00_FF00) | (v >> 8 & 0x00FF_00FF),
                    _ => (v as u16).swap_bytes() as i16 as i32 as u32,
                };
                self.pc = self.pc.wrapping_add(2);
            }
            Instr::Pld { .. } => {
                // PLD is a hint; no-op in emulator.
                self.pc = self.pc.wrapping_add(2);
            }
            Instr::Nop => {
                self.pc = self.pc.wrapping_add(2);
            }
        }
        Ok(())
    }

    /// Evaluate a 4-bit condition code against the flags (cond 14/15 are
    /// never passed here: decode maps 15 to Svc and rejects 14).
    fn cond_true(&self, cond: u8) -> bool {
        match cond {
            0 => self.z,
            1 => !self.z,
            2 => self.c,
            3 => !self.c,
            4 => self.n,
            5 => !self.n,
            6 => self.v,
            7 => !self.v,
            8 => self.c && !self.z,
            9 => !self.c || self.z,
            10 => self.n == self.v,
            11 => self.n != self.v,
            12 => !self.z && self.n == self.v,
            13 => self.z || self.n != self.v,
            _ => false,
        }
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
        // wide signed sum: b+cin can wrap the sign bit (b = 0x7FFF_FFFF, cin)
        let wide_s = a as i32 as i64 + b as i32 as i64 + cin as i64;
        self.v = wide_s > i32::MAX as i64 || wide_s < i32::MIN as i64;
        self.set_nz(res);
        res
    }

    fn sub_flags(&mut self, a: u32, b: u32, cin: bool) -> u32 {
        // computes a - b - (1 - cin); cin=true means "no borrow"
        let res = a.wrapping_sub(b).wrapping_sub(!cin as u32);
        // borrow comparison without wrapping: b+1 wraps to 0 when b = 0xFFFF_FFFF
        self.c = a as u64 >= b as u64 + !cin as u64;
        // wide signed difference: b+!cin can wrap the sign bit (b = 0x7FFF_FFFF, !cin)
        let wide_s = a as i32 as i64 - b as i32 as i64 - (!cin as i64);
        self.v = wide_s > i32::MAX as i64 || wide_s < i32::MIN as i64;
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


    /// Run one instruction placed at flash offset 0 with register setup.
    fn run_one_with(setup: impl Fn(&mut Cpu), hws: &[u16]) -> Cpu {
        let mut flash = vec![0u8; 0x100];
        for (i, hw) in hws.iter().enumerate() {
            flash[i * 2..i * 2 + 2].copy_from_slice(&hw.to_le_bytes());
        }
        let mut bus = Bus::new(flash, 0x1000);
        let mut cpu = Cpu::new();
        cpu.sp = RAM_BASE + 0x800;
        setup(&mut cpu);
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
    fn test_dataproc_sbc_carry_wrap() {
        // 0x4188 = sbcs r0, r1 ; carried fix: sub_flags borrow comparison
        // must not wrap when b = 0xFFFF_FFFF and C is clear.
        let mut flash = vec![0u8; 0x100];
        flash[0..2].copy_from_slice(&0x4188u16.to_le_bytes());
        let mut bus = Bus::new(flash, 0x1000);
        let mut cpu = Cpu::new();
        cpu.r[0] = 5; cpu.r[1] = 0xFFFF_FFFF; cpu.c = false;
        cpu.step(&mut bus).unwrap();
        // 5 - 0xFFFF_FFFF - 1 wraps to 5, but always borrows
        assert_eq!(cpu.r[0], 5);
        assert!(!cpu.c);
    }

    #[test]
    fn test_adds_overflow_v_flag() {
        // 0x3001 = adds r0, #1
        let mut flash = vec![0u8; 0x100];
        flash[0..2].copy_from_slice(&0x3001u16.to_le_bytes());
        let mut bus = Bus::new(flash, 0x1000);
        let mut cpu = Cpu::new();
        cpu.r[0] = 0x7FFF_FFFF;
        cpu.step(&mut bus).unwrap();
        assert!(cpu.v);
        assert!(cpu.n);
        // no-overflow case
        cpu.r[0] = 1;
        cpu.pc = 0;
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.r[0], 2);
        assert!(!cpu.v);
    }

    #[test]
    fn test_mov_hi_and_bx() {
        // 0x4686 = mov lr, r0 ; 0x4770 = bx lr
        // (brief said 0x4687, which decodes to mov pc, r0; 0x4686 is mov lr, r0)
        let mut flash = vec![0u8; 0x100];
        flash[0..2].copy_from_slice(&0x4686u16.to_le_bytes());
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
    fn test_sbc_v_flag_sign_corner() {
        // 0x4188 = sbcs r0, r1 ; V must not be computed against the
        // wrapped b+!cin (0x7FFF_FFFF + 1 wraps the sign bit).
        let mut flash = vec![0u8; 0x100];
        flash[0..2].copy_from_slice(&0x4188u16.to_le_bytes());
        let mut bus = Bus::new(flash, 0x1000);
        let mut cpu = Cpu::new();
        cpu.r[0] = 0xFFFF_FFFF; cpu.r[1] = 0x7FFF_FFFF; cpu.c = false;
        cpu.step(&mut bus).unwrap();
        // -1 - 2147483647 - 1 = -2147483649, below i32::MIN
        assert_eq!(cpu.r[0], 0x7FFF_FFFF);
        assert!(cpu.v);
    }

    #[test]
    fn test_adc_v_flag_sign_corner() {
        // 0x4148 = adcs r0, r1 ; b+cin wraps the sign bit
        // (0x7FFF_FFFF + 1 = 0x8000_0000), so the XOR formula misses V.
        let mut flash = vec![0u8; 0x100];
        flash[0..2].copy_from_slice(&0x4148u16.to_le_bytes());
        let mut bus = Bus::new(flash, 0x1000);
        let mut cpu = Cpu::new();
        cpu.r[0] = 0; cpu.r[1] = 0x7FFF_FFFF; cpu.c = true;
        cpu.step(&mut bus).unwrap();
        // 0 + 0x7FFF_FFFF + 1 = 0x8000_0000, past i32::MAX
        assert_eq!(cpu.r[0], 0x8000_0000);
        assert!(cpu.v);
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
        // BL to +0x10: first hw 0xF000, second 0xF808 (S=0,imm10=0,J1=J2=1,imm11=8 -> off=16)
        // (brief said 0xF008, which has J2=0 and yields off=0x400010 under the
        // ARM formula; 0xF808 is the encoding matching the stated fields)
        let mut flash = vec![0u8; 0x100];
        flash[0..2].copy_from_slice(&0xF000u16.to_le_bytes());
        flash[2..4].copy_from_slice(&0xF808u16.to_le_bytes());
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

    #[test]
    fn test_wild_address_faults_not_panics() {
        // carried ruling: address arithmetic must wrap, not panic in debug.
        // 0x6848 = ldr r0, [r1, #4] with r1 = 0xFFFF_FFFE wraps the base+offset
        // add to 2; the step must produce a clean EmuError (unaligned here).
        let mut flash = vec![0u8; 0x100];
        flash[0..2].copy_from_slice(&0x6848u16.to_le_bytes());
        let mut bus = Bus::new(flash, 0x1000);
        let mut cpu = Cpu::new();
        cpu.r[1] = 0xFFFF_FFFE;
        let err = cpu.step(&mut bus).unwrap_err();
        assert!(matches!(err, EmuError::Unaligned { addr: 2, size: 4 }));
    }

    #[test]
    fn test_cbz_cbnz() {
        // 0xB128 = cbz r0, #10 (i=0, imm5=5 -> offset (0:00101:0) = 10)
        let cpu = run_one_with(|cpu| cpu.r[0] = 0, &[0xB128, 0x46C0, 0x46C0, 0x46C0, 0x46C0, 0x46C0, 0x46C0]);
        assert_eq!(cpu.pc, 4 + 10);
        // not taken when rn != 0
        let cpu = run_one_with(|cpu| cpu.r[0] = 7, &[0xB128]);
        assert_eq!(cpu.pc, 2);
        // 0xB911 = cbnz r1, #4 (i=0, imm5=2 -> offset 4)
        let cpu = run_one_with(|cpu| cpu.r[1] = 0x1234, &[0xB911]);
        assert_eq!(cpu.pc, 4 + 4);
        // not taken when rn == 0
        let cpu = run_one_with(|cpu| cpu.r[1] = 0, &[0xB911]);
        assert_eq!(cpu.pc, 2);
        // i bit extends the range: 0xB328 = cbz r0, #(0b1001010) = +74
        let cpu = run_one_with(|cpu| cpu.r[0] = 0, &[0xB328]);
        assert_eq!(cpu.pc, 4 + 74);
    }

    #[test]
    fn test_cps_variants_decode_as_nop() {
        // cpsie i = 0xB662, cpsid i = 0xB672 (and the A/I/F variants) must
        // decode as NOP; only bit 3 set (0xB668) stays architecturally invalid.
        for hw in [0xB660u16, 0xB661, 0xB662, 0xB663, 0xB672] {
            let cpu = run_one(hw);
            assert_eq!(cpu.pc, 2, "CPS {hw:#06x} did not advance pc");
        }
        let mut flash = vec![0u8; 0x100];
        flash[0..2].copy_from_slice(&0xB668u16.to_le_bytes());
        let mut bus = Bus::new(flash, 0x1000);
        let mut cpu = Cpu::new();
        assert!(matches!(cpu.step(&mut bus), Err(EmuError::Undefined { .. })));
    }

    #[test]
    fn test_bl_rejects_bad_second_halfword() {
        // 0xF000 prefix followed by 0xE000 (a `b` encoding, not 11 J1 1 J2)
        // must fault as Undefined instead of branching to a garbage target.
        let mut flash = vec![0u8; 0x100];
        flash[0..2].copy_from_slice(&0xF000u16.to_le_bytes());
        flash[2..4].copy_from_slice(&0xE000u16.to_le_bytes());
        let mut bus = Bus::new(flash, 0x1000);
        let mut cpu = Cpu::new();
        assert!(matches!(cpu.step(&mut bus), Err(EmuError::Undefined { pc: 0, instr: 0xF000 })));
    }

    #[test]
    fn test_tst_w_dpimm_executes() {
        // `tst.w r3, #0x40` (hw1 0xF013, hw2 0x0F40) from af_190602 0x8c8a:
        // sets Z when (r3 & 0x40) == 0, clears it otherwise. No register write.
        let mut flash = vec![0u8; 0x100];
        flash[0..2].copy_from_slice(&0xF013u16.to_le_bytes());
        flash[2..4].copy_from_slice(&0x0F40u16.to_le_bytes());
        let mut bus = Bus::new(flash, 0x1000);
        let mut cpu = Cpu::new();
        cpu.r[3] = 0x40;
        cpu.step(&mut bus).unwrap();
        assert!(!cpu.z);
        assert_eq!(cpu.pc, 4);
        cpu.pc = 0;
        cpu.r[3] = 0x00;
        cpu.step(&mut bus).unwrap();
        assert!(cpu.z);
    }

    #[test]
    fn test_mov_w_dpimm_does_not_read_pc() {
        // `mov.w sl, #3` (hw1 0xF04F, hw2 0x0A03) from af_190602 0x8df2:
        // ORR with Rn = 1111 is the MOV alias — the Rn operand is NOT read,
        // so the result is the immediate, not (pc+4) | imm.
        let mut flash = vec![0u8; 0x100];
        flash[0..2].copy_from_slice(&0xF04Fu16.to_le_bytes());
        flash[2..4].copy_from_slice(&0x0A03u16.to_le_bytes());
        let mut bus = Bus::new(flash, 0x1000);
        let mut cpu = Cpu::new();
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.r[10], 3, "mov.w sl, #3 must be 3, not pc|3");
        assert_eq!(cpu.pc, 4);
    }

    #[test]
    fn test_bic_w_asr31_executes() {
        // bic.w r1, r0, r0, asr #31 (EA20 71E0, af_190602 0x13cde):
        // r1 = r0 & ~(r0 asr 31) — i.e. r0 clamped to non-negative.
        let mut flash = vec![0u8; 0x100];
        flash[0..2].copy_from_slice(&0xEA20u16.to_le_bytes());
        flash[2..4].copy_from_slice(&0x71E0u16.to_le_bytes());
        let mut bus = Bus::new(flash, 0x1000);
        let mut cpu = Cpu::new();
        cpu.r[0] = 5;
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.r[1], 5);
        cpu.pc = 0;
        cpu.r[0] = 0x8000_0000;
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.r[1], 0);
    }

    #[test]
    fn test_lsl_w_mov_alias_executes() {
        // lsl.w sb, sl, #0xc (EA4F 390A): MOV alias, Rn = 1111 operand is
        // not read — result is the shifted Rm only.
        let mut flash = vec![0u8; 0x100];
        flash[0..2].copy_from_slice(&0xEA4Fu16.to_le_bytes());
        flash[2..4].copy_from_slice(&0x390Au16.to_le_bytes());
        let mut bus = Bus::new(flash, 0x1000);
        let mut cpu = Cpu::new();
        cpu.r[10] = 1;
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.r[9], 0x1000);
    }

    #[test]
    fn test_pop_w_pc_transfers_and_consumes_word() {
        // pop.w {r4, pc} = e8bd 8010: bit 15 of the list is the pc. The pc
        // word must be popped from the stack (sp advances past it) AND
        // execution must transfer to it — falling through would corrupt the
        // stack by 4 bytes on every function return (found by the
        // af_190602 layer-4 gate as a drift of sp up to ram_top).
        let mut flash = vec![0u8; 0x100];
        flash[0..2].copy_from_slice(&0xe8bdu16.to_le_bytes());
        flash[2..4].copy_from_slice(&0x8010u16.to_le_bytes());
        let mut bus = Bus::new(flash, 0x1000);
        bus.allow_region(RAM_BASE..RAM_BASE + 0x1000);
        let mut cpu = Cpu::new();
        cpu.sp = RAM_BASE + 0x100;
        bus.write_u32(RAM_BASE + 0x100, 0x11223344).unwrap(); // r4
        bus.write_u32(RAM_BASE + 0x104, 0x215 | 1).unwrap();  // pc (thumb bit)
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.r[4], 0x11223344);
        assert_eq!(cpu.pc, 0x214);
        assert_eq!(cpu.sp, RAM_BASE + 0x108);
    }

    #[test]
    fn test_stm_w_no_writeback_stores_ascending() {
        // `stm.w sp, {r7, sl}` = e88d 0480 (Capstone-verified, af_190602
        // 0x8e02): STMIA with NO writeback. Decoding this as STMDB sp!
        // (the old behavior) moved sp down 8 bytes and stored the words at
        // the wrong addresses, corrupting a stack slot read later as a
        // format character.
        let mut flash = vec![0u8; 0x100];
        flash[0..2].copy_from_slice(&0xe88du16.to_le_bytes());
        flash[2..4].copy_from_slice(&0x0480u16.to_le_bytes());
        let mut bus = Bus::new(flash, 0x1000);
        bus.allow_region(RAM_BASE..RAM_BASE + 0x1000);
        let mut cpu = Cpu::new();
        cpu.sp = RAM_BASE + 0x100;
        cpu.r[7] = 1;
        cpu.r[10] = 0x1f;
        cpu.step(&mut bus).unwrap();
        assert_eq!(bus.read_u32(RAM_BASE + 0x100).unwrap(), 1);   // r7
        assert_eq!(bus.read_u32(RAM_BASE + 0x104).unwrap(), 0x1f); // sl
        assert_eq!(cpu.sp, RAM_BASE + 0x100); // no writeback
        assert_eq!(cpu.pc, 4);
    }

    #[test]
    fn test_strb_t4_pre_indexed_subtract() {
        // `strb r7, [lr, #-1]!` = F80E 7D01 (Capstone-verified, af_190602
        // 0x13d10): pre-indexed byte store with negative offset and writeback.
        let mut flash = vec![0u8; 0x100];
        flash[0..2].copy_from_slice(&0xF80Eu16.to_le_bytes());
        flash[2..4].copy_from_slice(&0x7D01u16.to_le_bytes());
        let mut bus = Bus::new(flash, 0x1000);
        bus.allow_region(RAM_BASE..RAM_BASE + 0x1000);
        let mut cpu = Cpu::new();
        cpu.lr = RAM_BASE + 0x101;
        cpu.r[7] = 0xAB;
        cpu.step(&mut bus).unwrap();
        assert_eq!(bus.read_u8(RAM_BASE + 0x100).unwrap(), 0xAB);
        assert_eq!(cpu.lr, RAM_BASE + 0x100); // lr - 1 written back
    }
}
