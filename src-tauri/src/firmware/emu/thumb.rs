//! Thumb-1 instruction decode. `decode` returns `None` for undefined
//! encodings and 32-bit prefixes; later tasks prepend arms above the
//! catch-all in each `match`.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Instr {
    LslImm { rd: u8, rm: u8, imm: u8 },
    LsrImm { rd: u8, rm: u8, imm: u8 },
    AsrImm { rd: u8, rm: u8, imm: u8 },
    AddReg { rd: u8, rn: u8, rm: u8 },
    SubReg { rd: u8, rn: u8, rm: u8 },
    AddImm3 { rd: u8, rn: u8, imm: u8 },
    SubImm3 { rd: u8, rn: u8, imm: u8 },
    MovImm { rd: u8, imm: u8 },
    CmpImm { rn: u8, imm: u8 },
    AddImm8 { rdn: u8, imm: u8 },
    SubImm8 { rdn: u8, imm: u8 },
    /// 0x4000..0x43FF data-processing; op 0x0..0xF, low regs only.
    DataProc { op: u8, rdn: u8, rm: u8 },
    /// Hi-register add (010001 00); reg ids 0..15. No flags.
    AddHi { rd: u8, rm: u8 },
    /// Hi-register compare (010001 01); sets flags.
    CmpHi { rn: u8, rm: u8 },
    /// Hi-register move (010001 10); no flags.
    MovHi { rd: u8, rm: u8 },
    Bx { rm: u8 },
    Blx { rm: u8 },
    /// LDR rt, [PC, #imm8*4]; base is (pc+4) & !3. No flags.
    LdrLit { rt: u8, imm: u8 },
    /// Register-offset load/store; op: 0=STR,1=STRH,2=STRB,3=LDRSB,4=LDR,5=LDRH,6=LDRB,7=LDRSH.
    LsReg { op: u8, rt: u8, rn: u8, rm: u8 },
    /// Word/byte imm5-offset load/store. No flags.
    LsImm { load: bool, byte: bool, rt: u8, rn: u8, imm: u8 },
    /// Halfword imm5-offset load/store. No flags.
    LshImm { load: bool, rt: u8, rn: u8, imm: u8 },
    /// SP-relative load/store, imm8*4. No flags.
    LsSp { load: bool, rt: u8, imm: u8 },
    /// ADD rd, PC, #imm8*4 (ADR); base is (pc+4) & !3. No flags.
    Adr { rd: u8, imm: u8 },
    /// ADD rd, SP, #imm8*4. No flags.
    AddSpImm { rd: u8, imm: u8 },
    /// ADD/SUB SP, #imm7*4. No flags.
    AdjSp { sub: bool, imm: u8 },
    /// Unconditional branch; `off` is the sign-extended byte offset from pc+4.
    B { off: i32 },
    /// PUSH register list; `lr` = store LR too. No flags.
    Push { list: u8, lr: bool },
    /// POP register list; `pc` = load PC too (masks bit0). No flags.
    Pop { list: u8, pc: bool },
    /// STM rn!, {list}: store ascending, writeback. No flags.
    Stm { rn: u8, list: u8 },
    /// LDM rn!, {list}: load ascending, writeback unless rn in list. No flags.
    Ldm { rn: u8, list: u8 },
    /// Conditional branch; `off` is the sign-extended byte offset from pc+4.
    BCond { cond: u8, off: i32 },
    /// 32-bit branch-with-link; decoded in Cpu::step (needs the second halfword).
    Bl { off: i32 },
    /// Supervisor call; a no-op in this harness.
    Svc { imm: u8 },
    /// Breakpoint; a no-op in this harness.
    Bkpt { imm: u8 },
    /// Sign/zero extend; op: 0=SXTH,1=SXTB,2=UXTH,3=UXTB. No flags.
    Extend { op: u8, rd: u8, rm: u8 },
    /// Byte reverse; op: 0=REV,1=REV16,3=REVSH. No flags.
    Rev { op: u8, rd: u8, rm: u8 },
    /// NOP and other hints (incl. CPS). No flags.
    Nop,
}

pub fn decode(hw: u16) -> Option<Instr> {
    if hw >> 10 == 0b010000 {
        // 0x4000..0x43FF data-processing
        return Some(Instr::DataProc { op: ((hw >> 6) & 0xF) as u8, rdn: (hw & 7) as u8, rm: ((hw >> 3) & 7) as u8 });
    }
    if hw >> 8 >= 0x44 && hw >> 8 <= 0x47 {
        // 010001xx hi-reg ops / BX / BLX
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
    // Group C (loads/stores, ADR, SP adjust). These guards sit above the
    // group-A `match hw >> 11`; their top-bit patterns (0x4800+, 0x5000+,
    // 0x6000..0x9FFF, 0xA000+, 0xB0xx) do not overlap the group-B guards
    // (0x4000..0x47FF) checked above.
    if hw >> 11 == 0b01001 {
        // LDR literal
        return Some(Instr::LdrLit { rt: ((hw >> 8) & 7) as u8, imm: (hw & 0xFF) as u8 });
    }
    if hw >> 12 == 0b0101 {
        // load/store with register offset
        return Some(Instr::LsReg { op: ((hw >> 9) & 7) as u8, rm: ((hw >> 6) & 7) as u8, rn: ((hw >> 3) & 7) as u8, rt: (hw & 7) as u8 });
    }
    if hw >> 13 == 0b011 {
        // load/store word/byte with imm5 offset
        let op = (hw >> 11) & 3; // 0=STR,1=LDR,2=STRB,3=LDRB
        return Some(Instr::LsImm { load: op & 1 == 1, byte: op >= 2, rt: (hw & 7) as u8, rn: ((hw >> 3) & 7) as u8, imm: ((hw >> 6) & 0x1F) as u8 });
    }
    if hw >> 12 == 0b1000 {
        // load/store halfword with imm5 offset
        return Some(Instr::LshImm { load: (hw >> 11) & 1 == 1, rt: (hw & 7) as u8, rn: ((hw >> 3) & 7) as u8, imm: ((hw >> 6) & 0x1F) as u8 });
    }
    if hw >> 12 == 0b1001 {
        // SP-relative load/store
        return Some(Instr::LsSp { load: (hw >> 11) & 1 == 1, rt: ((hw >> 8) & 7) as u8, imm: (hw & 0xFF) as u8 });
    }
    if hw >> 12 == 0b1010 {
        // ADR / ADD rd, SP, #imm8*4
        let rd = ((hw >> 8) & 7) as u8; let imm = (hw & 0xFF) as u8;
        return Some(if (hw >> 11) & 1 == 0 { Instr::Adr { rd, imm } } else { Instr::AddSpImm { rd, imm } });
    }
    if hw >> 8 == 0xB0 { // 1011 0000 x imm7  SP adjust
        return Some(Instr::AdjSp { sub: (hw >> 7) & 1 == 1, imm: (hw & 0x7F) as u8 });
    }
    // Group D (stack ops, multi load/store, branches, misc). Same placement
    // reasoning as group C: these top-bit patterns (0xB2xx+, 0xB4xx..0xBFxx,
    // 0xC000+, 0xD000+) do not overlap the group-B guards checked above.
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
        let off = ((hw & 0xFF) as i32) << 24 >> 23; // sign-extend imm8<<1
        return Some(Instr::BCond { cond, off });
    }
    if hw >> 11 == 0b11110 { // BL prefix; second halfword at pc+2
        // decode() is pure; BL needs the second halfword, so Cpu::step
        // intercepts this encoding before calling decode.
        unreachable!("Cpu::step intercepts the BL prefix before decode");
    }
    let instr = match hw >> 11 {
        0b000 => Instr::LslImm { rd: (hw & 7) as u8, rm: ((hw >> 3) & 7) as u8, imm: ((hw >> 6) & 0x1F) as u8 },
        0b001 => Instr::LsrImm { rd: (hw & 7) as u8, rm: ((hw >> 3) & 7) as u8, imm: ((hw >> 6) & 0x1F) as u8 },
        0b010 => Instr::AsrImm { rd: (hw & 7) as u8, rm: ((hw >> 3) & 7) as u8, imm: ((hw >> 6) & 0x1F) as u8 },
        0b011 => {
            let rd = (hw & 7) as u8;
            let rn = ((hw >> 3) & 7) as u8;
            let val = ((hw >> 6) & 7) as u8;
            match ((hw >> 10) & 1, (hw >> 9) & 1) {
                (0, 0) => Instr::AddReg { rd, rn, rm: val },
                (0, 1) => Instr::SubReg { rd, rn, rm: val },
                (1, 0) => Instr::AddImm3 { rd, rn, imm: val },
                _ => Instr::SubImm3 { rd, rn, imm: val },
            }
        }
        0b100 => Instr::MovImm { rd: ((hw >> 8) & 7) as u8, imm: (hw & 0xFF) as u8 },
        0b101 => Instr::CmpImm { rn: ((hw >> 8) & 7) as u8, imm: (hw & 0xFF) as u8 },
        0b110 => Instr::AddImm8 { rdn: ((hw >> 8) & 7) as u8, imm: (hw & 0xFF) as u8 },
        0b111 => Instr::SubImm8 { rdn: ((hw >> 8) & 7) as u8, imm: (hw & 0xFF) as u8 },
        0b11100 => {
            // b <imm11>: offset = sign-extend-12(imm11 << 1)
            let off = ((hw & 0x7FF) as i32) << 21 >> 20;
            Instr::B { off }
        }
        _ => return None, // later tasks add arms above this
    };
    Some(instr)
}
