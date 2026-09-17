//! ARMv6-M CPU core: register file, flags, fetch/decode/execute step,
//! and a budgeted run loop.

use super::bus::Bus;
use super::thumb::{decode, decode32, DpOp, DpRegOp, Instr, Thumb2};
use super::EmuError;
use std::collections::VecDeque;

const TRACE_CAP: usize = 32;

/// TEMP discovery hook (Task 6): last PC observed by any Cpu, shared with
/// the bus-side discovery traces. REMOVE with the engine-write trace.
static LAST_PC: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

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
    /// Remaining instructions of the active IT block; while > 0, each step
    /// pops one condition nibble from `it_conds` (MSB first) and skips the
    /// instruction if that condition fails. 0 = not in an IT block.
    it_left: u8,
    /// Up to 4 effective condition nibbles for the active IT block, packed
    /// MSB-first; each consumed step shifts left by 4.
    it_conds: u16,
}

impl Cpu {
    /// TEMP discovery hook (Task 6): last PC observed by any Cpu, for
    /// logging from the bus. REMOVE with the engine-write trace.
    pub fn last_pc_global() -> u32 {
        crate::firmware::emu::cpu::LAST_PC.load(std::sync::atomic::Ordering::Relaxed)
    }

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
            it_left: 0,
            it_conds: 0,
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
        crate::firmware::emu::cpu::LAST_PC.store(self.pc, std::sync::atomic::Ordering::Relaxed);
        self.trace.push_back(self.pc);
        if self.trace.len() > TRACE_CAP {
            self.trace.pop_front();
        }
        let pc = self.pc;
        let hw = bus.read_u16(pc)?;
        let (instr, size) = if hw >> 11 == 0b11110 {
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
                (Instr::Bl { off }, 4)
            } else {
                // Otherwise: 32-bit data-processing or other 32-bit Thumb-2.
                let instr = decode32(hw, hw2).ok_or(EmuError::Undefined { pc, instr: hw })?;
                (instr, 4)
            }
        } else if hw >> 11 == 0b11101 || hw >> 11 == 0b11111 {
            // Other 32-bit Thumb-2 instructions.
            let hw2 = bus.read_u16(pc.wrapping_add(2))?;
            let instr = decode32(hw, hw2).ok_or(EmuError::Undefined { pc, instr: hw })?;
            (instr, 4)
        } else {
            let instr = decode(hw).ok_or(EmuError::Undefined { pc, instr: hw })?;
            (instr, 2)
        };
        bus.advance_core_tick();
        // IT-block conditional execution: pop this instruction's condition
        // and skip (no side effects, pc still advances) when it fails.
        if self.it_left > 0 {
            let c = ((self.it_conds >> 12) & 0xF) as u8;
            self.it_conds <<= 4;
            self.it_left -= 1;
            if !self.cond_passes(c) {
                self.pc = pc.wrapping_add(size);
                return Ok(());
            }
        }
        self.execute(bus, instr)
    }

    /// ARM condition-code evaluation for IT blocks and (future) BCond arms.
    fn cond_passes(&self, c: u8) -> bool {
        let base = match c >> 1 {
            0 => self.z,                        // eq / ne
            1 => self.c,                        // cs / cc
            2 => self.n,                        // mi / pl
            3 => self.v,                        // vs / vc
            4 => self.c && !self.z,             // hi / ls
            5 => self.n == self.v,              // ge / lt
            6 => !self.z && self.n == self.v,   // gt / le
            _ => true,                          // al (0xF = nv is unpredictable)
        };
        base ^ (c & 1 == 1)
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
            Instr::Thumb2(Thumb2::LdmIA { rn, list, wb }) => {
                let mut addr = self.reg(rn);
                let mut new_pc = None;
                for i in 0..16u8 {
                    if list >> i & 1 == 1 {
                        let value = bus.read_u32(addr)?;
                        if i == 15 {
                            new_pc = Some(value & !1);
                        } else {
                            self.set_reg(i, value);
                        }
                        addr = addr.wrapping_add(4);
                    }
                }
                if wb {
                    self.set_reg(rn, addr);
                }
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
            Instr::Thumb2(Thumb2::DpImm { op, rd, rn, set_flags, imm, shifter_c }) => {
                // 32-bit data-processing with modified immediate.
                // MOV (immediate) is the ORR encoding with Rn = 1111; the
                // Rn operand is not read (no PC-relative OR); result = imm.
                let lhs = if rn == 15 && matches!(op, DpOp::Orr) { 0 } else { self.reg(rn) };
                // Logical ops take their carry from the immediate's shifter
                // carry-out: the rotated forms report it, the plain forms
                // (i:imm3 == 0) PRESERVE the old C (ARM ARM ThumbExpandImm_C)
                // — e.g. `tst.w r3, #0x40` must not clear C.
                let logical_c = shifter_c.unwrap_or(self.c);
                let (result, carry, overflow) = match op {
                    DpOp::And => (lhs & imm, logical_c, false),
                    DpOp::Orr => (lhs | imm, logical_c, false),
                    DpOp::Eor => (lhs ^ imm, logical_c, false),
                    DpOp::Bic => (lhs & !imm, logical_c, false),
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
                    self.set_nzcv(result, carry, overflow);
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
                    self.set_nzcv(result, carry, overflow);
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
            Instr::Thumb2(Thumb2::Extend { signed, half, rd, rm, rot }) => {
                // UXTB/UXTH/SXTB/SXTH: operand = Rm ROR (imm2*8), masked to
                // the byte/half lane, then zero- (UX) or sign- (SX) extended.
                // No flags.
                let v = self.reg(rm).rotate_right(rot as u32 * 8);
                let v = if half { v & 0xFFFF } else { v & 0xFF };
                let v = if signed {
                    if half { (v as i16) as i32 as u32 } else { (v as i8) as i32 as u32 }
                } else {
                    v
                };
                self.set_reg(rd, v);
                self.pc = self.pc.wrapping_add(4);
            }
            Instr::Thumb2(Thumb2::LdrStrT4 { load, rt, rn, imm, pre, sub, wb }) => {
                // 32-bit LDR/STR word with index/writeback (T4). Per ARM ARM
                // A7.7.42: offset_addr = Rn ± imm8; address = offset_addr
                // (pre) or R[n] (post); wback ⇒ R[n] = offset_addr ALWAYS
                // (both P forms), after the memory access value is read but
                // before R[t] is written — so a post-indexed load where
                // rn == rt still yields the LOADED value in rt. Rt = 15 load
                // is a branch (LoadWritePC: bit 0 kept by `& !1` masking
                // here per the established Ldmia/pop convention; aligned
                // targets required else UNPREDICTABLE).
                let base = self.reg(rn);
                let offset_addr = if sub { base.wrapping_sub(imm as u32) } else { base.wrapping_add(imm as u32) };
                let addr = if pre { offset_addr } else { base };
                let mut branch = None;
                if load {
                    let v = bus.read_u32(addr)?;
                    if rt == 15 {
                        branch = Some(v & !1); // LoadWritePC
                    } else {
                        // rn == rt with wback is UNPREDICTABLE per ARM ARM;
                        // the memory read happens first and the writeback
                        // below runs before this set, so rt ends with the
                        // LOADED value — the hardware's observable order.
                        self.set_reg(rt, v);
                    }
                } else {
                    bus.write_u32(addr, self.reg(rt))?;
                }
                if wb {
                    self.set_reg(rn, offset_addr);
                }
                self.pc = branch.unwrap_or_else(|| self.pc.wrapping_add(4));
            }
            Instr::Thumb2(Thumb2::LdrhStrhT4 { load, rt, rn, imm, pre, sub, wb }) => {
                // 32-bit LDRH/STRH with index/writeback (T4): identical
                // addressing semantics to LdrStrT4 but a zero-extended
                // halfword access. No pc-target form exists for LDRH
                // (Rt = 15 is UNPREDICTABLE per ARM ARM; not branched).
                let base = self.reg(rn);
                let offset_addr = if sub { base.wrapping_sub(imm as u32) } else { base.wrapping_add(imm as u32) };
                let addr = if pre { offset_addr } else { base };
                if load {
                    let v = bus.read_u16(addr)? as u32;
                    self.set_reg(rt, v);
                } else {
                    bus.write_u16(addr, (self.reg(rt) & 0xFFFF) as u16)?;
                }
                if wb {
                    self.set_reg(rn, offset_addr);
                }
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
            Instr::Thumb2(Thumb2::LdrdStrd { load, rt, rt2, rn, imm, pre, sub, wb }) => {
                // Load/store dual word: byte offset = imm << 2, applied
                // pre-indexed (writeback only when W=1) or post-indexed
                // (always writeback). No flags.
                let base = self.reg(rn);
                let off = (imm as u32) << 2;
                let addr = if pre {
                    if sub { base.wrapping_sub(off) } else { base.wrapping_add(off) }
                } else {
                    base
                };
                if load {
                    self.set_reg(rt, bus.read_u32(addr)?);
                    self.set_reg(rt2, bus.read_u32(addr.wrapping_add(4))?);
                } else {
                    bus.write_u32(addr, self.reg(rt))?;
                    bus.write_u32(addr.wrapping_add(4), self.reg(rt2))?;
                }
                if wb || !pre {
                    let new_base = if sub { base.wrapping_sub(off) } else { base.wrapping_add(off) };
                    self.set_reg(rn, new_base);
                }
                self.pc = self.pc.wrapping_add(4);
            }
            Instr::Thumb2(Thumb2::Clz { rd, rm }) => {
                self.set_reg(rd, self.reg(rm).leading_zeros());
                self.pc = self.pc.wrapping_add(4);
            }
            Instr::Thumb2(Thumb2::Mrs { rd, sysreg }) => {
                // M-profile: only MSP (8), PSP (9) and XPSR (3) appear in the
                // firmware. This emulator runs in Thread mode with no PSP, so
                // MSP/PSP both read the main sp; XPSR reads as 0x0100_0000
                // (Thumb T bit set, no flags).
                let v = match sysreg {
                    8 | 9 => self.sp,
                    3 => 0x0100_0000,
                    _ => 0,
                };
                self.set_reg(rd, v);
                self.pc = self.pc.wrapping_add(4);
            }
            Instr::Thumb2(Thumb2::BfiBfc { rd, rn, lsbit, width }) => {
                // Bit-field insert (Rn != 15) / clear (Rn == 15): replace the
                // `width`-bit field at `lsbit` of Rd with Rn's field / with 0.
                // No flags.
                let mask = if width >= 32 { u32::MAX } else { (1u32 << width) - 1 };
                let field = if rn == 15 { 0 } else { self.reg(rn) & mask };
                let v = (self.reg(rd) & !(mask << lsbit)) | (field << lsbit);
                self.set_reg(rd, v);
                self.pc = self.pc.wrapping_add(4);
            }
            Instr::Thumb2(Thumb2::Subw { rd, rn, imm }) => {
                // Subtract with plain 12-bit immediate (no shifter carry,
                // never sets flags — S=1 is UNPREDICTABLE).
                let r = self.reg(rn).wrapping_sub(imm);
                self.set_reg(rd, r);
                self.pc = self.pc.wrapping_add(4);
            }
            Instr::Thumb2(Thumb2::TbbTbh { half, rm }) => {
                // Table branch: base = align4(pc+4); the table entry (byte for
                // TBB, halfword for TBH) at base + Rm (*2 for TBH) is a
                // halfword-scaled offset: target = base + 2 * entry.
                let base = self.pc.wrapping_add(4) & !3;
                let idx = self.reg(rm);
                let entry = if half {
                    bus.read_u16(base.wrapping_add(idx.wrapping_mul(2)))? as u32
                } else {
                    bus.read_u8(base.wrapping_add(idx))? as u32
                };
                self.pc = base.wrapping_add(entry.wrapping_mul(2));
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
            Instr::BCond { cond, off, wide } => {
                if self.cond_true(cond) {
                    self.pc = (self.pc as i32).wrapping_add(4).wrapping_add(off) as u32;
                } else {
                    // 16-bit form occupies 2 bytes; B<c>.W (T3) occupies 4.
                    self.pc = self.pc.wrapping_add(if wide { 4 } else { 2 });
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
            Instr::Nop => {
                self.pc = self.pc.wrapping_add(2);
            }
            Instr::It { cond, mask } => {
                // IT: set up conditional execution of the next 1-4
                // instructions. Expansion rule (n = instruction count,
                // field = T/E bits, 1=T): n = 4 - trailing_zeros(mask);
                // field = !(mask >> (tz+1)) & ((1<<(n-1))-1); instruction k
                // (k >= 2) takes bit (n-1-k) of field XORed with cond[0] —
                // 1 → cond, 0 → cond^1. Verified against keystone for all
                // 15 mask shapes × even/odd cond (see tests).
                let tz = mask.trailing_zeros() as u8; // mask != 0 (decode)
                let n = 4 - tz;
                let field = (!mask >> (tz + 1)) & ((1u8 << (n - 1)) - 1);
                // Pack the effective conditions MSB-first: slot 0 (bits 15:12)
                // is the first instruction (always firstcond), slot k is the
                // (k+1)-th.
                let mut conds: u16 = (cond as u16) << 12;
                for k in 1..n {
                    let then = ((field >> (n - 1 - k)) & 1) ^ (cond & 1);
                    let eff = if then == 1 { cond } else { cond ^ 1 };
                    conds |= (eff as u16) << (12 - 4 * k);
                }
                self.it_conds = conds;
                self.it_left = n;
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

    fn set_nzcv(&mut self, r: u32, c: bool, v: bool) {
        self.set_nz(r);
        self.c = c;
        self.v = v;
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
    fn test_systick_ticks_once_per_decoded_step_including_it_skip() {
        let flash = vec![0x00, 0xBF, 0x08, 0xBF, 0x01, 0x20, 0xFF, 0xDE];
        let mut bus = Bus::new(flash, 0x1000);
        bus.write_u32(0xE000_E014, 2).unwrap();
        bus.write_u32(0xE000_E010, 5).unwrap();
        let mut cpu = Cpu::new();
        for expected in [2, 1, 0] {
            cpu.step(&mut bus).unwrap();
            assert_eq!(bus.read_u32(0xE000_E018).unwrap(), expected);
        }
        assert_eq!(cpu.r[0], 0);
        assert_eq!(cpu.pc, 6);
        assert_eq!(bus.read_u32(0xE000_E010).unwrap(), 0x0001_0005);
        assert!(cpu.step(&mut bus).is_err());
        assert_eq!(bus.read_u32(0xE000_E018).unwrap(), 0);
        assert_eq!(bus.read_u32(0xE000_E010).unwrap(), 5);
    }

    #[test]
    fn test_ldmia_w_general_base_and_writeback() {
        // Capstone/ARM T2 vectors: E895 000F = ldm.w r5, {r0-r3};
        // E8B5 000F = ldm.w r5!, {r0-r3}. Neither is a POP from SP.
        for (hw1, writeback) in [(0xE895u16, false), (0xE8B5, true)] {
            let mut flash = vec![0u8; 0x100];
            flash[..2].copy_from_slice(&hw1.to_le_bytes());
            flash[2..4].copy_from_slice(&0x000Fu16.to_le_bytes());
            let mut bus = Bus::new(flash, 0x1000);
            bus.allow_region(RAM_BASE..RAM_BASE + 0x1000);
            let base = RAM_BASE + 0x100;
            let stack = RAM_BASE + 0x800;
            let values = [11u32, 22, 33, 44];
            for (i, value) in values.iter().enumerate() {
                bus.write_u32(base + i as u32 * 4, *value).unwrap();
                bus.write_u32(stack + i as u32 * 4, 0xBAD0_0000 + i as u32).unwrap();
            }
            bus.write_log.clear();
            let mut cpu = Cpu::new();
            cpu.r[5] = base;
            cpu.sp = stack;
            cpu.n = true;
            cpu.z = false;
            cpu.c = true;
            cpu.v = true;
            cpu.step(&mut bus).unwrap();
            assert_eq!(&cpu.r[..4], &values, "load from r5, not SP ({hw1:04x})");
            assert_eq!(cpu.r[5], base + if writeback { 16 } else { 0 });
            assert_eq!(cpu.sp, stack, "general-base LDM must not change SP");
            assert_eq!((cpu.n, cpu.z, cpu.c, cpu.v), (true, false, true, true));
            assert_eq!(cpu.pc, 4);
            assert!(bus.write_log.is_empty());
        }
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
    fn test_ldrstr_t4_post_indexed_store_advances_base() {
        // str r2, [r0], #4 = F840 2B04 — THE instruction that deadlocked the
        // emulated boot copy loop (0x606): previously decoded as DpImm
        // `orr r11, r0, #4`, so the store never fired and r0 never advanced.
        // Regression: store must land at the ORIGINAL base and r0 must
        // advance by 4.
        let mut flash = vec![0u8; 0x100];
        flash[0..2].copy_from_slice(&0xF840u16.to_le_bytes());
        flash[2..4].copy_from_slice(&0x2B04u16.to_le_bytes());
        let mut bus = Bus::new(flash, 0x1000);
        bus.allow_region(RAM_BASE..RAM_BASE + 0x1000);
        let mut cpu = Cpu::new();
        cpu.r[0] = RAM_BASE + 0x200;
        cpu.r[2] = 0xCAFE_BABE;
        cpu.step(&mut bus).unwrap();
        assert_eq!(bus.read_u32(RAM_BASE + 0x200).unwrap(), 0xCAFE_BABE);
        assert_eq!(cpu.r[0], RAM_BASE + 0x204, "post-indexed writeback must advance r0");
        assert_eq!(cpu.pc, 4);
    }

    #[test]
    fn test_ldrstr_t4_pre_sub_indexed_no_wb() {
        // str r2, [r1, #-4] = F841 2C04 (af_190602 0x6ade): pre-indexed,
        // subtract, W=0 — base register must stay untouched.
        let mut flash = vec![0u8; 0x100];
        flash[0..2].copy_from_slice(&0xF841u16.to_le_bytes());
        flash[2..4].copy_from_slice(&0x2C04u16.to_le_bytes());
        let mut bus = Bus::new(flash, 0x1000);
        bus.allow_region(RAM_BASE..RAM_BASE + 0x1000);
        let mut cpu = Cpu::new();
        cpu.r[1] = RAM_BASE + 0x204;
        cpu.r[2] = 0x1234_5678;
        cpu.step(&mut bus).unwrap();
        assert_eq!(bus.read_u32(RAM_BASE + 0x200).unwrap(), 0x1234_5678);
        assert_eq!(cpu.r[1], RAM_BASE + 0x204, "W=0: no writeback");
    }

    #[test]
    fn test_ldrhstrh_t4_pre_indexed_load_advances_base() {
        // ldrh r2, [r3, #2]! = F833 2F02 — the second boot blocker (0x2414):
        // pre-indexed load from r3+2 must land in r2 AND r3 must become r3+2.
        let mut flash = vec![0u8; 0x100];
        flash[0..2].copy_from_slice(&0xF833u16.to_le_bytes());
        flash[2..4].copy_from_slice(&0x2F02u16.to_le_bytes());
        let mut bus = Bus::new(flash, 0x1000);
        bus.allow_region(RAM_BASE..RAM_BASE + 0x1000);
        bus.write_u16(RAM_BASE + 0x82, 0xBEEF).unwrap();
        let mut cpu = Cpu::new();
        cpu.r[3] = RAM_BASE + 0x80;
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.r[2], 0xBEEF, "zero-extended halfword load from r3+2");
        assert_eq!(cpu.r[3], RAM_BASE + 0x82, "pre-indexed writeback");
    }

    #[test]
    fn test_ldrstr_t4_ldr_pc_post_branches_and_advances_sp() {
        // ldr pc, [sp], #4 = F85D FB04 (exception-return epilogue): loads
        // the word into pc as a branch (Thumb bit masked), AND writes sp
        // back with the post-increment — both effects observable.
        let mut flash = vec![0u8; 0x100];
        flash[0..2].copy_from_slice(&0xF85Du16.to_le_bytes());
        flash[2..4].copy_from_slice(&0xFB04u16.to_le_bytes());
        let mut bus = Bus::new(flash, 0x1000);
        bus.allow_region(RAM_BASE..RAM_BASE + 0x1000);
        bus.write_u32(RAM_BASE + 0x300, 0x2000_0041).unwrap(); // thumb target
        let mut cpu = Cpu::new();
        cpu.sp = RAM_BASE + 0x300;
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.pc, 0x2000_0040, "LoadWritePC: branch, bit0 masked");
        assert_eq!(cpu.sp, RAM_BASE + 0x304, "post-indexed writeback must advance sp");
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
        // 0x5640 = ldrsb r0, [r0, r1]: op=3 encoding 0x5600 | rm<<6 | rn<<3 | rt
        // with rm=1, rn=0, rt=0 → 0x5600 | 0x40.
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
    fn test_unaligned_ldr_succeeds() {
        // 0x6808 = ldr r0, [r1, #0]. ARMv7-M allows unaligned LDR (the af
        // boot genuinely loads a word at 0x20000161) — the load must
        // succeed and return the little-endian byte sequence.
        let mut flash = vec![0u8; 0x100];
        flash[0..2].copy_from_slice(&0x6808u16.to_le_bytes());
        let mut bus = Bus::new(flash, 0x1000);
        bus.write_u32(RAM_BASE + 0x100, 0x1122_3344).unwrap();
        let mut cpu = Cpu::new();
        cpu.r[1] = RAM_BASE + 0x101;
        cpu.step(&mut bus).unwrap();
        // bytes at 0x101..0x105 = 33 22 11 00 → 0x00112233
        assert_eq!(cpu.r[0], 0x0011_2233, "little-endian split across the word");
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
    fn test_strd_ldrd_roundtrip() {
        // strd r12, lr, [sp, #-0x10]! = E96D CE04, then ldrd r2, r3,
        // [sp, #0] (E9DD 2300) reading the same two words back.
        let mut flash = vec![0u8; 0x100];
        flash[0..4].copy_from_slice(&[0x6d, 0xe9, 0x04, 0xce]); // strd r12, lr, [sp, #-0x10]!
        flash[4..8].copy_from_slice(&[0xdd, 0xe9, 0x00, 0x23]); // ldrd r2, r3, [sp, #0]
        let mut bus = Bus::new(flash, 0x1000);
        bus.allow_region(RAM_BASE..RAM_BASE + 0x1000);
        let mut cpu = Cpu::new();
        cpu.sp = RAM_BASE + 0x800;
        cpu.r[12] = 0x1122_3344;
        cpu.lr = 0x5566_7788;
        cpu.step(&mut bus).unwrap();
        // Pre-indexed subtract, writeback: sp = 0x7F0; words at 0x7F0/0x7F4.
        assert_eq!(cpu.sp, RAM_BASE + 0x7F0);
        assert_eq!(bus.read_u32(RAM_BASE + 0x7F0).unwrap(), 0x1122_3344);
        assert_eq!(bus.read_u32(RAM_BASE + 0x7F4).unwrap(), 0x5566_7788);
        cpu.r[2] = 0;
        cpu.r[3] = 0;
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.r[2], 0x1122_3344);
        assert_eq!(cpu.r[3], 0x5566_7788);
        // Offset 0, writeback sp += 0: sp unchanged.
        assert_eq!(cpu.sp, RAM_BASE + 0x7F0);
    }

    #[test]
    fn test_bfi_bfc_semantics() {
        // bfi r3, r2, #8, #1 = F362 2308 (af_190602 0x1d7c): set bit 8 of r3
        // to bit 0 of r2; other bits preserved.
        let mut flash = vec![0u8; 0x100];
        flash[0..4].copy_from_slice(&[0x62, 0xf3, 0x08, 0x23]); // bfi r3, r2, #8, #1
        let mut bus = Bus::new(flash, 0x1000);
        let mut cpu = Cpu::new();
        cpu.r[3] = 0xFFFF_FEFF;
        cpu.r[2] = 0x1;
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.r[3], 0xFFFF_FFFF);
        // bfc r3, #0, #1 = F36F 0300 (af_190602 0x27f2): clear bit 0.
        let mut flash = vec![0u8; 0x100];
        flash[0..4].copy_from_slice(&[0x6f, 0xf3, 0x00, 0x03]); // bfc r3, #0, #1
        let mut bus = Bus::new(flash, 0x1000);
        let mut cpu = Cpu::new();
        cpu.r[3] = 0xFFFF_FFFF;
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.r[3], 0xFFFF_FFFE);
    }

    #[test]
    fn test_subw_plain_imm() {
        // subw r1, r1, #0x726 = F2A1 7126 (af_190602 0x1e08): plain imm12.
        let mut flash = vec![0u8; 0x100];
        flash[0..4].copy_from_slice(&[0xa1, 0xf2, 0x26, 0x71]); // subw r1, r1, #0x726
        let mut bus = Bus::new(flash, 0x1000);
        let mut cpu = Cpu::new();
        cpu.r[1] = 0x800;
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.r[1], 0x800 - 0x726);
    }

    #[test]
    fn test_tbh_branch_target() {
        // tbh [pc, r2] at 0 with base = align4(0+4) = 4; halfwords at
        // 4 + 2*r2 scale the target: target = 4 + 2 * entry.
        let mut flash = vec![0u8; 0x100];
        flash[0..4].copy_from_slice(&[0xdf, 0xe8, 0x12, 0xf0]); // tbh [pc, r2]
        flash[4..6].copy_from_slice(&0x0010u16.to_le_bytes()); // entry 0 -> 4 + 32 = 36
        flash[8..10].copy_from_slice(&0x0002u16.to_le_bytes()); // entry 1 (r2=2) -> 4 + 4 = 8
        let mut bus = Bus::new(flash, 0x1000);
        let mut cpu = Cpu::new();
        cpu.r[2] = 2;
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.pc, 8);
    }

    #[test]
    fn test_clz_and_mrs() {
        // clz lr, r2 = FAB2 FE82 (af_190602 0x19b6).
        let mut flash = vec![0u8; 0x100];
        flash[0..4].copy_from_slice(&[0xb2, 0xfa, 0x82, 0xfe]); // clz lr, r2
        let mut bus = Bus::new(flash, 0x1000);
        let mut cpu = Cpu::new();
        cpu.r[2] = 0x00F0_0000;
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.lr, 8);
        // mrs r4, xpsr = F3EF 8403 (af_190602 0x1cfe): reads 0x0100_0000.
        let mut flash = vec![0u8; 0x100];
        flash[0..4].copy_from_slice(&[0xef, 0xf3, 0x03, 0x84]); // mrs r4, xpsr
        let mut bus = Bus::new(flash, 0x1000);
        let mut cpu = Cpu::new();
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.r[4], 0x0100_0000);
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
        // add to 2. With ARMv7-M-legal unaligned loads, address 2 resolves to
        // flash (offset 2 < image len) — the step must succeed with the value
        // read from there (zeros in this fixture), not panic in debug.
        let mut flash = vec![0u8; 0x100];
        flash[0..2].copy_from_slice(&0x6848u16.to_le_bytes());
        let mut bus = Bus::new(flash, 0x1000);
        let mut cpu = Cpu::new();
        cpu.r[1] = 0xFFFF_FFFE;
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.r[0], 0);
        assert_eq!(cpu.pc, 2); // 16-bit ldr
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

#[cfg(test)]
mod it_tests {
    use super::*;
    use crate::firmware::emu::bus::{Bus, RAM_BASE};

    fn run(hws: &[u16], setup: impl Fn(&mut Cpu)) -> Cpu {
        let mut flash = vec![0u8; 0x100];
        for (i, hw) in hws.iter().enumerate() {
            flash[i * 2..i * 2 + 2].copy_from_slice(&hw.to_le_bytes());
        }
        let mut bus = Bus::new(flash, 0x1000);
        bus.allow_region(RAM_BASE..RAM_BASE + 0x1000);
        let mut cpu = Cpu::new();
        setup(&mut cpu);
        while cpu.pc < (hws.len() as u32) * 2 {
            cpu.step(&mut bus).unwrap();
        }
        cpu
    }

    #[test]
    fn test_ite_ne_executes_one_arm() {
        // The exact block from af_190602 0x8c8e (Capstone-verified):
        //   ite ne ; movne r2, #0xb7 ; moveq r2, #0x87
        // Before IT support both arms ran and 0x87 always won.
        let hws = [0xBF14, 0x22B7, 0x2287];
        let mut cpu = run(&hws, |c| c.z = false); // ne passes
        assert_eq!(cpu.r[2], 0xB7);
        cpu = run(&hws, |c| c.z = true); // eq passes
        assert_eq!(cpu.r[2], 0x87);
    }

    #[test]
    fn test_it_skip_has_no_side_effects_and_no_fault() {
        // it ne ; ldrb r0, [r1]  — with Z set (eq), the load is skipped:
        // no fault on the unmapped address, r0 untouched.
        let hws = [0xBF18, 0x7808]; // it ne ; ldrb r0, [r1]
        let cpu = run(&hws, |c| {
            c.z = true;
            c.r[1] = 0xDEAD_0000;
        });
        assert_eq!(cpu.r[0], 0);
        assert_eq!(cpu.pc, 4);
    }

    #[test]
    fn test_it_block_with_32bit_instruction() {
        // it ne ; mov.w r0, #7 (F04F 0007: imm8 = 7, Rd = r0).
        let hws = [0xBF18, 0xF04F, 0x0007];
        let mut cpu = run(&hws, |c| c.z = false);
        assert_eq!(cpu.r[0], 7);
        cpu = run(&hws, |c| c.z = true);
        assert_eq!(cpu.r[0], 0); // skipped
    }

    #[test]
    fn test_cond_passes_table() {
        let mut cpu = Cpu::new();
        cpu.n = true; cpu.z = true; cpu.c = false; cpu.v = true;
        let cases = [
            (0x0, true), (0x1, false),          // eq / ne
            (0x2, false), (0x3, true),          // cs / cc
            (0x4, true), (0x5, false),          // mi / pl
            (0x6, true), (0x7, false),          // vs / vc
            (0x8, false), (0x9, true),          // hi / ls
            (0xA, true), (0xB, false),          // ge (N==V) / lt
            (0xC, false), (0xD, true),          // gt / le
            (0xE, true),                        // al
        ];
        for (cond, want) in cases {
            assert_eq!(cpu.cond_passes(cond), want, "cond {cond:#x}");
        }
    }

    #[test]
    fn test_it_expansion_matches_keystone() {
        // (hw, expected effective conditions) — expected values derived from
        // keystone-assembled IT shapes for cond=eq (0) and cond=ne (1):
        // T keeps cond, E inverts it.
        let eq = 0u8;
        let ne = 1u8;
        let cases: &[(u16, &[u8])] = &[
            (0xBF08, &[eq]),                          // it eq
            (0xBF0C, &[eq, eq ^ 1]),                  // ite eq
            (0xBF04, &[eq, eq]),                      // itt eq
            (0xBF0A, &[eq, eq ^ 1, eq]),              // itet eq
            (0xBF02, &[eq, eq, eq]),                  // ittt eq
            (0xBF09, &[eq, eq ^ 1, eq, eq]),          // itett eq
            (0xBF01, &[eq, eq, eq, eq]),              // itttt eq
            (0xBF0D, &[eq, eq ^ 1, eq ^ 1, eq]),      // iteet eq
            (0xBF18, &[ne]),                          // it ne
            (0xBF14, &[ne, ne ^ 1]),                  // ite ne
            (0xBF1C, &[ne, ne]),                      // itt ne
            (0xBF1A, &[ne, ne, ne ^ 1]),              // itte ne
            (0xBF19, &[ne, ne, ne ^ 1, ne ^ 1]),      // ittee ne
            (0xBF1F, &[ne, ne, ne, ne]),              // itttt ne
            (0xBF13, &[ne, ne ^ 1, ne ^ 1, ne]),      // iteet ne
        ];
        for &(hw, want) in cases {
            let mut flash = vec![0u8; 0x40];
            flash[0..2].copy_from_slice(&hw.to_le_bytes());
            // Nop sled for the block body; each nop consumes one slot.
            for i in 1..=4 {
                flash[i * 2..i * 2 + 2].copy_from_slice(&0x46C0u16.to_le_bytes());
            }
            let mut bus = Bus::new(flash, 0x100);
            let mut cpu = Cpu::new();
            cpu.step(&mut bus).unwrap(); // the IT itself
            // Exposed via the block's per-instruction conditions: run the
            // nops and compare against a re-derivation from `want`.
            for (k, &cond) in want.iter().enumerate() {
                assert_eq!(cpu.it_left as usize, want.len() - k, "hw {hw:#x} slot {k}");
                let c = ((cpu.it_conds >> 12) & 0xF) as u8;
                assert_eq!(c, cond, "hw {hw:#x} slot {k}");
                cpu.step(&mut bus).unwrap();
            }
            assert_eq!(cpu.it_left, 0, "hw {hw:#x} block must end");
        }
    }

    #[test]
    fn test_dpimm_logical_preserves_c_for_plain_imm() {
        // tst.w r3, #0x40 (F013 0F40, af_190602 0x8c8a) with old C=1:
        // the plain imm8 form must PRESERVE C (ARM ARM ThumbExpandImm_C).
        // Before the fix the emulator set C = result>>31 = 0.
        let hws = [0xF013, 0x0F40];
        let cpu = run(&hws, |c| {
            c.r[3] = 0xFFFF_FFFF;
            c.c = true;
        });
        assert!(!cpu.z && cpu.c, "tst.w #0x40 must preserve C");
    }

    #[test]
    fn test_dpimm_logical_rotated_imm_reports_carry() {
        // tst.w r3, #0x400 (F413 2F80, imm12 0xA80): rotated form sets
        // C = bit31 of the expanded immediate = 0.
        let hws = [0xF413, 0x2F80];
        let cpu = run(&hws, |c| {
            c.r[3] = 0x0400_0000;
            c.c = true;
        });
        assert!(cpu.z && !cpu.c, "tst.w #0x400 must report shifter carry 0");
    }
}
