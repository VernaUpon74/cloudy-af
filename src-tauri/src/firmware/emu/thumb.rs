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
    /// `wide` marks the 32-bit B<c>.W (T3) form, which advances pc by 4 when
    /// not taken (the 16-bit form advances by 2).
    BCond { cond: u8, off: i32, wide: bool },
    /// CBZ/CBNZ: branch on (non-)zero register; `off` is (i:imm5:0) from pc+4.
    Cbz { nonzero: bool, rn: u8, off: i32 },
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
    /// IT block: conditionally execute the next 1-4 instructions.
    /// `cond` is the firstcond nibble, `mask` the IT[3:0] mask field.
    It { cond: u8, mask: u8 },
    /// 32-bit Thumb-2 instructions decoded by `decode32`.
    Thumb2(Thumb2),
}

/// Thumb modified-immediate expansion (ARM ARM ThumbExpandImm).
/// imm12 = i : imm3 : imm8. With i:imm3 == 0 the four 0b00/01/10/11 patterns
/// at bits 9:8 produce plain / 0x00XY00XY / 0xXY00XY00 / 0xXYXYXYXY forms;
/// otherwise the value is 0b1xxxxxxx (0x80 | imm8[6:0]) rotated right by
/// imm12[11:7]. Capstone-verified: 0xE80 -> 0x400, 0xC00 -> 0x8000,
/// 0x780 -> 0x01000000, 0xC7F -> 0xFF00, 0x1A5 -> 0x00A500A5.
///
/// Also returns the shifter carry-out (ThumbExpandImm_C): Some(c) for the
/// rotated forms; None for the plain forms, where the caller must PRESERVE
/// the old C flag (used by logical DpImm with set_flags).
fn thumb_expand_imm_c(imm12: u16) -> (u32, Option<bool>) {
    let imm8 = (imm12 & 0xFF) as u32;
    if imm12 & 0xC00 == 0 {
        let v = match (imm12 >> 8) & 3 {
            0 => imm8,
            1 => (imm8 << 16) | imm8,
            2 => (imm8 << 24) | (imm8 << 8),
            _ => (imm8 << 24) | (imm8 << 16) | (imm8 << 8) | imm8,
        };
        (v, None)
    } else {
        let unrotated = 0x80 | (imm8 & 0x7F);
        let rot = ((imm12 >> 7) & 0x1F) as u32;
        let v = unrotated.rotate_right(rot); // rot >= 1 in this family
        (v, Some(v >> 31 != 0))
    }
}

#[cfg(test)]
fn thumb_expand_imm(imm12: u16) -> u32 {
    thumb_expand_imm_c(imm12).0
}

/// The set is intentionally minimal; add variants only when the gate hits
/// an undefined 32-bit encoding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Thumb2 {
    /// STMDB sp!, {register list}: 32-bit PUSH. `list` bits 0..14 = r0..r14.
    Stmdb { list: u16 },
    /// LDMIA sp!, {register list}: 32-bit POP. `list` bits 0..14 = r0..r14.
    Ldmia { list: u16 },
    /// STMIA Rn, {register list}: 32-bit store-multiple WITHOUT writeback
    /// (hw1 = 1110_100P USW0 Rn with W=0, e.g. `stm.w sp, {r7, sl}` =
    /// E88D 0480, Capstone-verified, af_190602 0x8e02). `list` bits 0..14 =
    /// r0..r14. Words are stored ascending from Rn; Rn is NOT updated.
    StmIA { rn: u8, list: u16 },
    /// 32-bit data-processing with modified 12-bit immediate.
    /// `op` encodes the operation (AND/EOR/ORR/BIC/ADD/SUB/CMP/MOV).
    /// `shifter_c` is the immediate's shifter carry-out for logical ops with
    /// set_flags: Some(c) for the rotated forms, None = preserve old C for
    /// the plain (i:imm3 == 0) forms (ARM ARM ThumbExpandImm_C).
    DpImm { op: DpOp, rd: u8, rn: u8, set_flags: bool, imm: u32, shifter_c: Option<bool> },
    /// 32-bit unconditional branch (B.W). Same offset encoding as BL.
    B { off: i32 },
    /// 32-bit LDR (literal): LDR Rt, [PC, #imm12]. Rt bits 15:12, imm12 bits 11:0.
    LdrLitW { rt: u8, imm: u16 },
    /// 32-bit multiply / multiply-accumulate (MUL / MLA, sub = false) and
    /// multiply-subtract (MLS, sub = true). Result = Ra +/- Rn*Rm, written to
    /// Rd. Ra=15 encodes MUL (Ra ignored); otherwise MLA/MLS.
    /// Encoding: hw1 = 11111 010 11 Rn, hw2 = Ra Rd op Rm (op 0000 = MUL/MLA,
    /// 0001 = MLS; Capstone-verified: mls r7, ip, r8, r1 = FB0C 1718).
    Mul { rd: u8, rn: u8, rm: u8, ra: u8, sub: bool },
    /// SBFX.W Rd, Rn, #lsbit, #width (T1): extract `width` bits from Rn
    /// starting at `lsbit`, sign-extend to 32 bits. No flags.
    /// hw1 = 11110 11 0 110 0 Rn (0xF340|Rn, Capstone-verified),
    /// hw2 = imm3 Rd imm2 0 widthm1 (imm2 = bits 7:6, widthm1 = bits 4:0).
    Sbfx { rd: u8, rn: u8, lsbit: u8, width: u8 },
    /// UBFX.W Rd, Rn, #lsbit, #width (T1): extract `width` bits from Rn
    /// starting at `lsbit`, zero-extend to 32 bits. No flags.
    /// hw1 = 11110 11 0 111 0 Rn (0xF3C0|Rn, Capstone-verified),
    /// hw2 = imm3 Rd imm2 0 widthm1 (same layout as Sbfx).
    /// Capstone-verified: ubfx r1, r2, #0, #9 = F3C2 0108 (af_190602 0x18e).
    Ubfx { rd: u8, rn: u8, lsbit: u8, width: u8 },
    /// LDR/STR (register) T2 family, positive LSL offset, no writeback:
    /// hw1 = 11111 000 0 op Rn with op (bits 7:4) 0000=STRB, 0001=LDRB,
    /// 0010=STRH, 0011=LDRH, 0100=STR, 0101=LDR; hw2 = Rt imm2 000000 Rm.
    /// Byte offset = Rm << imm2 (LSR/ASR forms do not exist for this family).
    /// Capstone-verified: strb.w r4, [r3, r0] = F803 4000;
    /// ldr.w r3, [r3, r1, lsl #2] = F853 3021 (af_190602 0x1cc, gate hit —
    /// previously fell through into DpImm and executed garbage).
    /// Thumb-2 has no negative register-offset store; the "F0 13 ..." bytes
    /// noted during RE (at 0x8c8a) are `tst.w r3, #0x40`, a DpImm instruction.
    LdrStrReg { load: bool, size: u8, rt: u8, rn: u8, rm: u8, shift: u8 },
    /// LDRH.W Rt, [Rn, #imm12] (T3): halfword load, zero-extended. No flags.
    /// hw1 = 11111 000 0101 1 Rn (0xF8B0|Rn, Capstone-verified),
    /// hw2 = Rt 0 imm12. Byte offset = imm12 (no shift).
    LdrhImm { rt: u8, rn: u8, imm: u16 },
    /// LDR.W Rt, [Rn, #imm12] (T4): word load. No flags.
    /// hw1 = 11111 000 1 101 Rn (0xF8D0|Rn, Capstone-verified),
    /// hw2 = Rt 0 imm12. Byte offset = imm12 (no shift).
    /// (LDR literal T3, hw1 = 0xF8DF exactly, is decoded earlier.)
    LdrImm { rt: u8, rn: u8, imm: u16 },
    /// STR.W Rt, [Rn, #imm12] (T4): word store. No flags.
    /// hw1 = 11111 000 1 100 Rn (0xF8C0|Rn, Capstone-verified),
    /// hw2 = Rt 0 imm12. Byte offset = imm12 (no shift).
    StrImm { rt: u8, rn: u8, imm: u16 },
    /// LDRB.W Rt, [Rn, #imm12]: byte load, zero-extended. No flags.
    /// hw1 = 11111 000 1 001 Rn (0xF890|Rn, Capstone-verified),
    /// hw2 = Rt 0 imm12. Byte offset = imm12 (no shift).
    LdrbImm { rt: u8, rn: u8, imm: u16 },
    /// STRB.W Rt, [Rn, #imm12] (T3): byte store. No flags.
    /// hw1 = 11111 000 1000 Rn (0xF880|Rn, Capstone-verified:
    /// strb.w r3, [sp, #4] = F88D 3004, af_190602 0x160), hw2 = Rt 0 imm12.
    /// Checked before LdrStrReg/StrbT4: those require hw2 bits 11:6 patterns
    /// that T3 never produces, but T3 with imm12 < 64 looks like StrbReg.
    StrbImm { rt: u8, rn: u8, imm: u16 },
    /// STRH.W Rt, [Rn, #imm12] (T3): halfword store. No flags.
    /// hw1 = 11111 000 1010 Rn (0xF8A0|Rn, Capstone-verified:
    /// strh.w r2, [r5, #0x2a] = F8A5 202A), hw2 = Rt 0 imm12.
    StrhImm { rt: u8, rn: u8, imm: u16 },
    /// STRB.W Rt, [Rn, ±imm8] with writeback (T4): P=1 pre-indexed
    /// (`[Rn, #-imm8]!`), P=0 post-indexed (`[Rn], #±imm8`).
    /// hw1 = 11111 000 0000 Rn (0xF800|Rn), hw2 = Rt 1 PU 1 imm8
    /// (Capstone-verified: strb r7, [lr, #-1]! = F80E 7D01, af_190602 0x13d10).
    StrbT4 { rt: u8, rn: u8, imm: u8, pre: bool, sub: bool },
    /// LDRB.W Rt, [Rn, ±imm8] with writeback (T4): same shape as StrbT4 but
    /// hw1 bits 7:4 = 0001 (0xF810|Rn). Byte load, zero-extended. No flags.
    /// (Capstone-verified: ldrb r1, [r3], #-1 = F813 1901, af_190602 0x13d18.)
    LdrbT4 { rt: u8, rn: u8, imm: u8, pre: bool, sub: bool },
    /// SDIV.W Rd, Rn, Rm: signed divide, truncated toward zero. No flags.
    /// hw1 = 11111 011 1001 Rn (0xFB90|Rn), hw2 = 1111 Rd 1111 Rm
    /// (Capstone-verified: sdiv r8, r1, ip = FB91 F8FC, af_190602 0x13d04).
    /// Divide-by-zero yields 0; INT_MIN / -1 yields INT_MIN (ARMv7-M).
    Sdiv { rd: u8, rn: u8, rm: u8 },
    /// UDIV.W Rd, Rn, Rm: unsigned divide. No flags.
    /// hw1 = 11111 011 1011 Rn (0xFBB0|Rn), hw2 = 1111 Rd 1111 Rm
    /// (Capstone-verified: udiv r3, r3, r2 = FBB3 F3F2, af_190602 0x1ac).
    /// Divide-by-zero yields 0 (ARMv7-M).
    Udiv { rd: u8, rn: u8, rm: u8 },
    /// 32-bit data-processing (shifted register): logical (AND/BIC/ORR/ORN/
    /// EOR) and arithmetic (ADD/ADC/SBC/SUB) forms.
    /// hw1 = 1110 1011 0 op[3:0] S Rn,
    /// hw2 = 0 imm3 Rd imm2 stype Rm; shift amount = imm3:imm2, stype
    /// 0=LSL 1=LSR 2=ASR 3=ROR (amount 0: LSR/ASR = #32, ROR = RRX).
    /// TST/MOV/MVN are aliases: Rd=15 (TST/TEQ/CMP/CMN: flags only, no
    /// write) or Rn=15 (MOV/MVN: Rn operand not read).
    /// `lsl.w sb, sl, #0xc` = EA4F 390A,
    /// `bic.w r1, r0, r0, asr #31` = EA20 71E0 (af_190602 0x13cde),
    /// `sub.w lr, r1, r0` = EBA1 0E00 (af_190602 0x12bc0).
    DpReg { op: DpRegOp, rd: u8, rn: u8, rm: u8, stype: u8, amount: u8, set_flags: bool },
    /// 32-bit MOV (register) with register-controlled shift: LSL/LSR/ASR/ROR.W
    /// Rd, Rn, Rm. hw1 = 1111 1010 0 stype S Rn (stype bits 6:5, S bit 4);
    /// hw2 = 1 111 Rd 00 0000 Rm (imm3:imm2 = 0b11100 = register amount).
    /// Capstone-verified: lsl.w ip, r4, r5 = FA04 FC05 (af_190602 0x12bf8);
    /// asr.w r1, r2, r3 = FA42 F103.
    MovShiftReg { rd: u8, rn: u8, rm: u8, stype: u8, set_flags: bool },
    /// MOVW.W Rd, #imm16: Rd = imm16 (lower half, upper cleared). No flags.
    /// hw1 = 11110 i 100 imm4 (i = bit 10, imm4 = bits 3:0; mask 0xFBF0 =
    /// 0xF240, Capstone-verified: movw r3, #0x2327 = F242 3327,
    /// movw r0, #0xffff = F64F 70FF — i bit set for the 0xF64F form).
    Movw { rd: u8, imm16: u16 },
    /// MOVT.W Rd, #imm16: Rd[31:16] = imm16 (lower half preserved). No flags.
    /// hw1 = 11110 i 101 imm4 (mask 0xFBF0 = 0xF2C0, Capstone-verified:
    /// movt r3, #0x1234 = F2C1 2334).
    Movt { rd: u8, imm16: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DpRegOp {
    And,  // TST when rd=15
    Bic,
    Orr,  // MOV / LSL.W when rn=15
    Orn,  // MVN when rn=15
    Eor,  // TEQ when rd=15
    Add,  // CMN when rd=15
    Adc,
    Sbc,
    Sub,  // CMP when rd=15
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DpOp {
    And,  // TST when rd=15
    Orr,  // MOV when rn=15
    Eor,  // TEQ when rd=15
    Bic,
    Add,  // CMN when rd=15
    Sub,  // CMP when rd=15
    Rsb,  // reverse subtract: Rd = imm - Rn
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
    if (hw & 0xFD00) == 0xB100 || (hw & 0xFD00) == 0xB900 {
        // CBZ/CBNZ: 1011 op i 0 imm5 Rn; offset = (i:imm5:0) from pc+4.
        // Compilers emit these for short null checks — needed for real code.
        let i = ((hw >> 9) & 1) as i32;
        let imm5 = ((hw >> 3) & 0x1F) as i32;
        return Some(Instr::Cbz { nonzero: (hw >> 11) & 1 == 1, rn: (hw & 7) as u8, off: (i << 6) | (imm5 << 1) });
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
    if hw >> 8 == 0xBF {
        // 0xBFxx: hint space (NOP/WFI/..., mask == 0) and IT blocks
        // (mask != 0 → conditional execution of the next 1-4 instructions).
        // IT expansion semantics verified against keystone for all 15 mask
        // shapes × both condition parities (see cpu.rs tests).
        let mask = (hw & 0xF) as u8;
        if mask == 0 {
            return Some(Instr::Nop);
        }
        return Some(Instr::It { cond: ((hw >> 4) & 0xF) as u8, mask });
    }
    // (Halfwords with high byte 0x46 already decoded as hi-reg ops above;
    // halfwords with top five bits 11110 never reach this function — Cpu::step
    // routes them to the 32-bit decode path first.)
    if hw >> 5 == 0b1011_0110_011 && hw & 0x8 == 0 {
        // CPS (all im/A/I/F variants, e.g. CPSIE i = 0xB662, CPSID i = 0xB672).
        // PRIMASK is not modelled, so this is a NOP — but it must decode,
        // otherwise real render code faults as Undefined.
        return Some(Instr::Nop);
    }
    if hw >> 12 == 0b1100 {
        let rn = ((hw >> 8) & 7) as u8; let list = (hw & 0xFF) as u8;
        return Some(if (hw >> 11) & 1 == 0 { Instr::Stm { rn, list } } else { Instr::Ldm { rn, list } });
    }
    if hw >> 12 == 0b1101 {
        let cond = ((hw >> 8) & 0xF) as u8;
        if cond == 0b1111 { return Some(Instr::Svc { imm: (hw & 0xFF) as u8 }); }
        if cond == 0b1110 { return None; }
        let off = ((hw & 0xFF) as i32) << 24 >> 23; // sign-extend imm8<<1
        return Some(Instr::BCond { cond, off, wide: false });
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
        _ => return None, // 32-bit prefixes are handled in Cpu::step
    };
    Some(instr)
}

/// Decode a 32-bit Thumb-2 instruction from its two halfwords.
/// Returns `None` for unsupported/undefined encodings.
pub fn decode32(hw1: u16, hw2: u16) -> Option<Instr> {
    let op1 = hw1 >> 11; // full 5-bit prefix: 11101 or 11111 for non-BL 32-bit
    let op2 = (hw1 >> 9) & 0b11; // bits 10:9

    // Load/store multiple / dual / table branch (op1 == 0b11101, op2 == 0b00)
    if op1 == 0b11101 && op2 == 0b00 {
        // Encoding: 1110_100P USW L Rn, hw2 = register list.
        //   push.w = P1U0W1L0 (STMDB sp!), pop.w = P0U1W1L1 (LDMIA sp!),
        //   stm.w (no `!`) = P?U?W0L0 (STMIA, NO writeback).
        let w = (hw1 >> 5) & 1 == 1; // bit 5
        let l = (hw1 >> 4) & 1 == 1; // bit 4 = L
        let list = hw2;
        if !w && !l {
            // STMIA Rn, {list}: no writeback. Capstone-verified:
            //   stm.w sp, {r7, sl}  e88d 0480 (af_190602 0x8e02).
            // Decoding this as STMDB sp! (the old behavior) corrupted sp
            // and misaligned the stored words.
            let rn = (hw1 & 0xF) as u8;
            return Some(Instr::Thumb2(Thumb2::StmIA { rn, list }));
        }
        // Empirical: STMDB (push.w) = bits 8:7 10, LDMIA (pop.w) = 01
        let sub2 = (hw1 >> 7) & 0b11; // bits 8:7
        // Verified empirically against Capstone:
        //   push.w {r4-r11, lr}  e92d 4ff0  -> list = 0x4ff0
        //   pop.w  {r4-r9, lr}   e8bd 43f0  -> list = 0x43f0
        if sub2 == 0b10 || sub2 == 0b01 {
            return Some(Instr::Thumb2(if l {
                Thumb2::Ldmia { list }
            } else {
                Thumb2::Stmdb { list }
            }));
        }
    }

    // 32-bit data-processing (shifted register): 1110 1011 0 op S Rn | 0 imm3 Rd imm2 stype Rm.
    // Capstone-verified vectors: bic.w r1, r0, r0, asr #31 = EA20 71E0;
    // lsl.w sb, sl, #0xc = EA4F 390A; tst.w r5, r6, lsl #2 = EA15 0F86;
    // sub.w lr, r1, r0 = EBA1 0E00; add.w r4, r1, r3 = EB01 0403.
    if (hw1 & 0xFE00) == 0xEA00 && hw2 & 0x8000 == 0 {
        let op = match (hw1 >> 5) & 0xF {
            0b0000 => DpRegOp::And, // TST if rd=15
            0b0001 => DpRegOp::Bic,
            0b0010 => DpRegOp::Orr, // MOV if rn=15
            0b0011 => DpRegOp::Orn, // MVN if rn=15
            0b0100 => DpRegOp::Eor, // TEQ if rd=15
            0b1000 => DpRegOp::Add, // CMN if rd=15
            0b1010 => DpRegOp::Adc,
            0b1011 => DpRegOp::Sbc,
            0b1101 => DpRegOp::Sub, // CMP if rd=15
            _ => return None,
        };
        let rn = (hw1 & 0xF) as u8;
        let set_flags = (hw1 >> 4) & 1 == 1;
        let imm3 = (hw2 >> 12) & 7;
        let rd = ((hw2 >> 8) & 0xF) as u8;
        let amount = ((imm3 << 2) | ((hw2 >> 6) & 3)) as u8;
        let stype = ((hw2 >> 4) & 3) as u8;
        let rm = (hw2 & 0xF) as u8;
        return Some(Instr::Thumb2(Thumb2::DpReg {
            op, rd, rn, rm, stype, amount, set_flags,
        }));
    }

    // 32-bit LDR (literal) T3: LDR.W Rt, [PC, #imm12].
    // hw1 = 11111 000 0 1 11111 (0xF8DF), hw2 = Rt imm12.
    if hw1 == 0xF8DF {
        let rt = ((hw2 >> 12) & 0xF) as u8;
        let imm = (hw2 & 0xFFF) as u16;
        return Some(Instr::Thumb2(Thumb2::LdrLitW { rt, imm }));
    }

    // 32-bit multiply / multiply-accumulate / multiply-subtract.
    // Encoding: hw1 = 11111 010 11 Rn, hw2 = Ra Rd op Rm.
    // hw2 bits 7:4 = 0000: MUL (Ra = 1111) / MLA (any other Ra);
    //              = 0001: MLS  (Rd = Ra - Rn*Rm).
    if hw1 >> 6 == 0b1111101100 {
        let rn = (hw1 & 0xF) as u8;
        let ra = ((hw2 >> 12) & 0xF) as u8;
        let rd = ((hw2 >> 8) & 0xF) as u8;
        let rm = (hw2 & 0xF) as u8;
        let sub = (hw2 & 0x00F0) == 0x0010;
        if (hw2 & 0x00F0) != 0 && !sub {
            return None; // bits 7:4 must be 0000 (MUL/MLA) or 0001 (MLS)
        }
        return Some(Instr::Thumb2(Thumb2::Mul { rd, rn, rm, ra, sub }));
    }

    // 32-bit LDRH (immediate) T3: `ldrh.w rt, [rn, #imm12]`.
    // Capstone-verified: ldrh.w lr, [sp, #0x20] = hw1 0xF8BD, hw2 0xE020
    // (af_190602 0x14100, hit by the layer-4 gate).
    if op1 == 0b11111 && (hw1 & 0x0FF0) == 0x08B0 {
        let rt = ((hw2 >> 12) & 0xF) as u8;
        let rn = (hw1 & 0xF) as u8;
        let imm = (hw2 & 0xFFF) as u16;
        return Some(Instr::Thumb2(Thumb2::LdrhImm { rt, rn, imm }));
    }

    // 32-bit LDR (immediate) T4: `ldr.w rt, [rn, #imm12]`.
    // Capstone-verified: ldr.w r2, [r8, #0] = hw1 0xF8D8, hw2 0x2000
    // (af_190602 0x8d0a, hit by the layer-4 gate).
    if op1 == 0b11111 && (hw1 & 0x0FF0) == 0x08D0 {
        let rt = ((hw2 >> 12) & 0xF) as u8;
        let rn = (hw1 & 0xF) as u8;
        let imm = (hw2 & 0xFFF) as u16;
        return Some(Instr::Thumb2(Thumb2::LdrImm { rt, rn, imm }));
    }

    // 32-bit STR (immediate) T4: `str.w rt, [rn, #imm12]`.
    // Capstone-verified: str.w r8, [sp, #0x14] = hw1 0xF8CD, hw2 0x8014
    // (af_190602 0x8d18, hit by the layer-4 gate).
    if op1 == 0b11111 && (hw1 & 0x0FF0) == 0x08C0 {
        let rt = ((hw2 >> 12) & 0xF) as u8;
        let rn = (hw1 & 0xF) as u8;
        let imm = (hw2 & 0xFFF) as u16;
        return Some(Instr::Thumb2(Thumb2::StrImm { rt, rn, imm }));
    }

    // 32-bit LDRB (immediate): `ldrb.w rt, [rn, #imm12]`.
    // Capstone-verified: ldrb.w r3, [r2, #0x27] = hw1 0xF892, hw2 0x3027
    // (af_190602 0x8d1c, hit by the layer-4 gate).
    if op1 == 0b11111 && (hw1 & 0x0FF0) == 0x0890 {
        let rt = ((hw2 >> 12) & 0xF) as u8;
        let rn = (hw1 & 0xF) as u8;
        let imm = (hw2 & 0xFFF) as u16;
        return Some(Instr::Thumb2(Thumb2::LdrbImm { rt, rn, imm }));
    }

    // 32-bit STRB (immediate) T3: `strb.w rt, [rn, #imm12]`.
    // Capstone-verified: strb.w r3, [sp, #4] = hw1 0xF88D, hw2 0x3004
    // (af_190602 0x160, hit by the layer-4 gate). hw2 bit 11 = 0.
    if op1 == 0b11111 && (hw1 & 0x0FF0) == 0x0880 && hw2 & 0x0800 == 0 {
        let rt = ((hw2 >> 12) & 0xF) as u8;
        let rn = (hw1 & 0xF) as u8;
        let imm = (hw2 & 0xFFF) as u16;
        return Some(Instr::Thumb2(Thumb2::StrbImm { rt, rn, imm }));
    }

    // 32-bit STRH (immediate) T3: `strh.w rt, [rn, #imm12]`.
    // Capstone-verified: strh.w r2, [r5, #0x2a] = hw1 0xF8A5, hw2 0x202A.
    if op1 == 0b11111 && (hw1 & 0x0FF0) == 0x08A0 && hw2 & 0x0800 == 0 {
        let rt = ((hw2 >> 12) & 0xF) as u8;
        let rn = (hw1 & 0xF) as u8;
        let imm = (hw2 & 0xFFF) as u16;
        return Some(Instr::Thumb2(Thumb2::StrhImm { rt, rn, imm }));
    }

    // 32-bit STRB / LDRB (immediate) T4 with writeback: `[rn, #-imm8]!` /
    // `[rn], #±imm8`. hw1 bits 7:4 = 0000 (STRB) / 0001 (LDRB); hw2 = Rt 1 P U 1 imm8.
    // Capstone-verified: strb r7, [lr, #-1]! = F80E 7D01 (0x13d10);
    // ldrb r1, [r3], #-1 = F813 1901 (0x13d18). The hw2 bits 11:8 = 1PU1
    // distinguish this from StrbReg (hw2 bits 11:6 = 0) at the same hw1.
    if op1 == 0b11111 && (hw1 & 0x0FF0) == 0x0800 && (hw2 & 0x0900) == 0x0900 {
        let rt = ((hw2 >> 12) & 0xF) as u8;
        let rn = (hw1 & 0xF) as u8;
        let imm = (hw2 & 0xFF) as u8;
        // hw2 = Rt 1 P U 1 imm8: P (pre-indexed) is bit 10, U (add, not
        // subtract) is bit 9. Verified: F80E 7D01 = strb r7, [lr, #-1]!
        // has bit 10 set, bit 9 clear.
        let pre = (hw2 >> 10) & 1 == 1;
        let sub = (hw2 >> 9) & 1 == 0;
        return Some(Instr::Thumb2(Thumb2::StrbT4 { rt, rn, imm, pre, sub }));
    }
    if op1 == 0b11111 && (hw1 & 0x0FF0) == 0x0810 && (hw2 & 0x0900) == 0x0900 {
        let rt = ((hw2 >> 12) & 0xF) as u8;
        let rn = (hw1 & 0xF) as u8;
        let imm = (hw2 & 0xFF) as u8;
        // Same field layout as StrbT4: P = bit 10, U = bit 9.
        let pre = (hw2 >> 10) & 1 == 1;
        let sub = (hw2 >> 9) & 1 == 0;
        return Some(Instr::Thumb2(Thumb2::LdrbT4 { rt, rn, imm, pre, sub }));
    }

    // 32-bit SDIV: `sdiv.w rd, rn, rm`.
    // Capstone-verified: sdiv r8, r1, ip = hw1 0xFB91, hw2 0xF8FC
    // (af_190602 0x13d04, hit by the layer-4 gate).
    if op1 == 0b11111 && (hw1 & 0xFFF0) == 0xFB90 && (hw2 & 0xF0F0) == 0xF0F0 {
        let rn = (hw1 & 0xF) as u8;
        let rd = ((hw2 >> 8) & 0xF) as u8;
        let rm = (hw2 & 0xF) as u8;
        return Some(Instr::Thumb2(Thumb2::Sdiv { rd, rn, rm }));
    }

    // 32-bit UDIV: `udiv.w rd, rn, rm`. Same hw2 shape as SDIV.
    // Capstone-verified: udiv r3, r3, r2 = hw1 0xFBB3, hw2 0xF3F2
    // (af_190602 0x1ac, hit by the layer-4 gate).
    if op1 == 0b11111 && (hw1 & 0xFFF0) == 0xFBB0 && (hw2 & 0xF0F0) == 0xF0F0 {
        let rn = (hw1 & 0xF) as u8;
        let rd = ((hw2 >> 8) & 0xF) as u8;
        let rm = (hw2 & 0xF) as u8;
        return Some(Instr::Thumb2(Thumb2::Udiv { rd, rn, rm }));
    }

    // 32-bit SBFX (signed bit-field extract) T1. Capstone-verified:
    // sbfx r6, r6, #0x12, #1 = hw1 0xF346, hw2 0x4680 (af_190602 0x8ce8).
    // Neighbors differ only in hw1 bits 7:4: ssat 0xF32x, ubfx 0xF3Cx, so
    // the full 0xF340 mask is required. Checked before DpImm: this family
    // shares the op1=0b11110 prefix and hw2[15]=0 shape.
    if op1 == 0b11110 && (hw1 & 0xFFF0) == 0xF340 {
        let rn = (hw1 & 0xF) as u8;
        let imm3 = (hw2 >> 12) & 0b111;
        let rd = ((hw2 >> 8) & 0xF) as u8;
        // Capstone-verified field layout: imm2 sits at hw2 bits 7:6 (not the
        // ARM ARM diagram position), widthm1 at bits 4:0.
        let imm2 = (hw2 >> 6) & 0b11;
        let width = ((hw2 & 0x1F) + 1) as u8;
        let lsbit = ((imm3 << 2) | imm2) as u8;
        if lsbit as u32 + width as u32 > 32 {
            return None; // UNPREDICTABLE per ARM ARM
        }
        return Some(Instr::Thumb2(Thumb2::Sbfx { rd, rn, lsbit, width }));
    }

    // 32-bit UBFX (unsigned bit-field extract) T1. Same field layout as SBFX;
    // hw1 = 0xF3C0|Rn. Capstone-verified: ubfx r1, r2, #0, #9 = F3C2 0108
    // (af_190602 0x18e, hit by the layer-4 gate).
    if op1 == 0b11110 && (hw1 & 0xFFF0) == 0xF3C0 {
        let rn = (hw1 & 0xF) as u8;
        let imm3 = (hw2 >> 12) & 0b111;
        let rd = ((hw2 >> 8) & 0xF) as u8;
        let imm2 = (hw2 >> 6) & 0b11;
        let width = ((hw2 & 0x1F) + 1) as u8;
        let lsbit = ((imm3 << 2) | imm2) as u8;
        if lsbit as u32 + width as u32 > 32 {
            return None; // UNPREDICTABLE per ARM ARM
        }
        return Some(Instr::Thumb2(Thumb2::Ubfx { rd, rn, lsbit, width }));
    }

    // 32-bit LDR/STR (register) T2 family: `ldr.w rt, [rn, rm, lsl #imm2]`
    // (and STRB/LDRB/STRH/LDRH/STR siblings), positive LSL offset, no writeback.
    // Capstone-verified: ldr.w r3, [r3, r1, lsl #2] = F853 3021 (af_190602
    // 0x1cc); strb.w r4, [r3, r0] = F803 4000. Checked before DpImm because
    // the load/store-single family shares the 0b11111 prefix; the
    // (hw2 & 0x0FC0) == 0 requirement (bits 11:6 zero, imm2 at 5:4) keeps it
    // disjoint from StrbT4/LdrbT4 (hw2 = Rt 1PU1 imm8) and from the
    // immediate-T3 forms above.
    if op1 == 0b11111 && (hw1 & 0x0F80) == 0x0800 && (hw2 & 0x0FC0) == 0 {
        let (load, size) = match (hw1 >> 4) & 0x7 {
            0b000 => (false, 1),
            0b001 => (true, 1),
            0b010 => (false, 2),
            0b011 => (true, 2),
            0b100 => (false, 4),
            0b101 => (true, 4),
            _ => return None,
        };
        let rt = ((hw2 >> 12) & 0xF) as u8;
        let rn = (hw1 & 0xF) as u8;
        let shift = ((hw2 >> 4) & 3) as u8;
        let rm = (hw2 & 0xF) as u8;
        return Some(Instr::Thumb2(Thumb2::LdrStrReg { load, size, rt, rn, rm, shift }));
    }

    // 32-bit MOVW / MOVT (immediate) T3/T1: `movw rd, #imm16` / `movt rd, #imm16`.
    // Capstone-verified: movw r3, #0x2327 = F242 3327 (af_190602 0x1c418, gate
    // hit); movt r3, #0x1234 = F2C1 2334. imm16 = imm4 : imm3 : imm8.
    // Checked before DpImm (like Sbfx): this family shares the op1=0b11110
    // prefix and hw2[15]=0 shape, and DpImm's op-class map does not cover
    // op1_class=10, so it would return None first.
    if op1 == 0b11110 && ((hw1 & 0xFBF0) == 0xF240 || (hw1 & 0xFBF0) == 0xF2C0) {
        let top = (hw1 & 0xF) as u16;
        let imm3 = (hw2 >> 12) & 0b111;
        let rd = ((hw2 >> 8) & 0xF) as u8;
        let imm8 = (hw2 & 0xFF) as u16;
        let imm16 = (top << 12) | ((imm3 as u16) << 8) | imm8;
        let movt = (hw1 & 0x80) != 0; // bit 7: MOVT, clear for MOVW
        return Some(Instr::Thumb2(if movt {
            Thumb2::Movt { rd, imm16 }
        } else {
            Thumb2::Movw { rd, imm16 }
        }));
    }

    // 32-bit CONDITIONAL branch B<c>.W (T3): hw1 = 11110 S cond imm6,
    // hw2 = 1 0 J2 0 J1 imm11 (bit 15 must be 1, bit 14 = 0, bit 12 = 0 —
    // bit 12 = 1 is T4/BL space). Offset = S:J1:J2:imm6:imm11:'0'
    // sign-extended from bit 20, with J1 = hw2 bit 11 at offset bit 19 and
    // J2 = hw2 bit 13 at offset bit 18, NO EOR-with-S transforms (unlike
    // T4/BL). Verified against Capstone on all 223 instances in af_190602
    // plus synthetic S/J1/J2/imm6/imm11 probes (see
    // `test_decode32_conditional_bw`); the T3 base is pc+4 with NO 4-byte
    // align (unlike T4/BL/literals). af_190602 0x9a24 `bmi.w 0x9b48` =
    // F100 8090. Must precede the T4 arm, which matches on hw2[15:14]=10
    // alone and would otherwise swallow T3 (its imm10 field overlaps cond).
    if op1 == 0b11110 && hw2 & 0xD000 == 0x8000 {
        let cond = ((hw1 >> 6) & 0xF) as u8;
        if cond >= 0b1110 {
            return None; // undefined, like the 16-bit B<cond> encoding
        }
        let s = ((hw1 >> 10) & 1) as u32;
        let imm21 = (s << 20)
            | ((((hw2 >> 11) & 1) as u32) << 19) // J1 (hw2 bit 11)
            | ((((hw2 >> 13) & 1) as u32) << 18) // J2 (hw2 bit 13)
            | (((hw1 & 0x3F) as u32) << 12)
            | (((hw2 & 0x7FF) as u32) << 1);
        let off = ((imm21 << 11) as i32) >> 11; // sign-extend 21 bits
        return Some(Instr::BCond { cond, off, wide: true });
    }

    // 32-bit unconditional branch B.W: hw1 = 11110 S imm10, hw2 = 10 J1 1 J2 imm11.
    // (BL uses hw2 = 11 J1 1 J2 imm11 and is intercepted in Cpu::step.)
    // Must be checked before data-processing because B.W and DpImm share the
    // 11110 prefix; B.W is distinguished by hw2 bit 15 = 1.
    if op1 == 0b11110 && (hw2 >> 15) & 1 == 1 && (hw2 >> 14) & 1 == 0 {
        let s = (hw1 >> 10) & 1;
        let j1 = (hw2 >> 13) & 1;
        let j2 = (hw2 >> 11) & 1;
        let i1 = (1 - (j1 ^ s)) & 1;
        let i2 = (1 - (j2 ^ s)) & 1;
        let imm25 = ((s as u32) << 24) | ((i1 as u32) << 23) | ((i2 as u32) << 22)
            | (((hw1 & 0x3FF) as u32) << 12) | (((hw2 & 0x7FF) as u32) << 1);
        let off = ((imm25 << 7) as i32) >> 7; // sign-extend 25 bits
        return Some(Instr::Thumb2(Thumb2::B { off }));
    }

    // 32-bit data-processing (modified immediate): op1 = 11110, 11111
    // Format: hw1 = 11110 i op1[1:0] 0 10 op2[1:0] S Rn
    // Actually simpler: op1 = hw1[9:8] is the top-level class:
    //   op1=00: AND (TST if Rd=15), ORR (MOV if Rn=15), EOR, BIC
    //   op1=01: ADD (CMN if Rd=15), SUB (CMP if Rd=15), ADC, SBC, RSB
    //   op1=10: MOV / MOVT etc
    //   op1=11: ...
    // We decode the subset used by the firmware render code.
    //
    // Structural guard: the branch family (B.W / BL, hw1 = 11110 S imm10)
    // shares this prefix space and is distinguished by the SECOND halfword:
    // branches have hw2[15] = 1 (10 J1 ... for B.W, 11 J1 ... for BL, the
    // latter intercepted earlier in Cpu::step). Data-processing has hw2[15] =
    // imm3[2] at 0 only for the encodings the firmware uses — any hw2[15] = 1
    // reaches here only for non-branch bit patterns (e.g. 0xE000: BL-invalid),
    // which must fault as `Undefined` (see `test_bl_rejects_bad_second_halfword`
    // and the 0xF000-prefix test in cpu.rs).
    if (op1 == 0b11110 || op1 == 0b11111) && hw2 & 0x8000 == 0 {
        let op1_class = (hw1 >> 8) & 0b11; // bits 9:8
        let i = (hw1 >> 10) & 1;
        let s = (hw1 >> 4) & 1 == 1;
        let rn = (hw1 & 0xF) as u8;
        let imm3 = (hw2 >> 12) & 0b111;
        let rd = ((hw2 >> 8) & 0xF) as u8;
        let imm8 = (hw2 & 0xFF) as u16;
        let imm12 = (i << 11) | (imm3 << 8) | imm8;
        let (imm, shifter_c) = thumb_expand_imm_c(imm12 as u16);

        // op2 in bits 7:6 determines exact op within each class.
        let op2 = (hw1 >> 6) & 0b11;

        // op1_class=00: op2=00 AND/TST, op2=01 ORR/MOV, op2=10 EOR/TEQ, op2=11 BIC/ORN
        // op1_class=01: op2=00 ADD, op2=01 ADC, op2=10 SUB/CMP, op2=11 RSB/CMN
        // For our firmware only op2=00/01 (logical) and op2=00/10/11 (arithmetic) are used so far.
        let op = match (op1_class, op2) {
            (0b00, 0b00) => DpOp::And, // TST if rd=15
            (0b00, 0b01) => DpOp::Orr, // MOV if rn=15
            (0b01, 0b00) => DpOp::Add,
            (0b01, 0b10) => DpOp::Sub, // CMP if rd=15
            (0b01, 0b11) => DpOp::Rsb,
            _ => return None,
        };
        return Some(Instr::Thumb2(Thumb2::DpImm { op, rd, rn, set_flags: s, imm, shifter_c }));
    }

    // 32-bit MOV (register, register-controlled shift): `lsl.w rd, rn, rm`.
    // Capstone-verified: lsl.w ip, r4, r5 = FA04 FC05 (af_190602 0x12bf8,
    // hit by the layer-4 gate); asr.w r1, r2, r3 = FA42 F103. hw2 bits 14:12
    // = 111 and bits 7:4 = 0000 pin the register-amount form.
    if (hw1 & 0xFE00) == 0xFA00 && (hw2 & 0x70F0) == 0x7000 {
        let rn = (hw1 & 0xF) as u8;
        let stype = ((hw1 >> 5) & 3) as u8;
        let set_flags = (hw1 >> 4) & 1 == 1;
        let rd = ((hw2 >> 8) & 0xF) as u8;
        let rm = (hw2 & 0xF) as u8;
        return Some(Instr::Thumb2(Thumb2::MovShiftReg { rd, rn, rm, stype, set_flags }));
    }

    None
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode32_conditional_bw() {
        // B<c>.W (T3) vectors, all Capstone-verified (python capstone,
        // CS_MODE_THUMB, target = pc+4 + off with NO 4-byte align). The real
        // one: af_190602 0x9a24 `bmi.w 0x9b48` = F100 8090 -> cond MI,
        // off 0x120, wide.
        match decode32(0xF100, 0x8090).unwrap() {
            Instr::BCond { cond, off, wide } => {
                assert_eq!(cond, 0b0100);
                assert_eq!(off, 0x120);
                assert!(wide);
            }
            other => panic!("expected BCond, got {other:?}"),
        }
        // J1 = hw2 bit 11 -> offset bit 19; J2 = hw2 bit 13 -> offset bit
        // 18; imm6 -> bits 17:12; S sign-extends from bit 20. No EOR
        // transforms (unlike T4/BL).
        for (hw1, hw2, off) in [
            (0xF100, 0x8800, 0x80000),            // J1=1 (bit 11)
            (0xF100, 0xA000, 0x40000),            // J2=1 (bit 13)
            (0xF100, 0xA800, 0xC0000),            // J1+J2
            (0xF100 | 1, 0x8000, 0x1000),     // imm6=1
            (0xF100, 0x8091, 0x122),              // imm11+1
            (0xF500, 0x8090, -0xFFEE0),           // S=1 alone
            (0xF500, 0x8890, -0x7FEE0),           // S=1, J1=1
            (0xF500, 0xA090, -0xBFEE0),           // S=1, J2=1
            (0xF500, 0xA890, -0x3FEE0),           // S=1, J1+J2
        ] {
            match decode32(hw1, hw2).unwrap() {
                Instr::BCond { cond, off: o, wide } => {
                    assert_eq!(cond, 0b0100, "cond for {hw1:#x} {hw2:#x}");
                    assert_eq!(o, off, "off for {hw1:#x} {hw2:#x}");
                    assert!(wide, "wide for {hw1:#x} {hw2:#x}");
                }
                other => panic!("expected BCond, got {other:?}"),
            }
        }
        // cond 0b1110/0b1111 is undefined (like the 16-bit encoding).
        assert!(decode32(0xF100 | (0b1110 << 6), 0x8090).is_none());
    }

    #[test]
    fn test_bcond_wide_not_taken_advances_four() {
        // B<c>.W (T3) not-taken must skip BOTH halfwords; the 16-bit form
        // skips one. Regression: af_190602 0x9a24 `bmi.w 0x9b48` not taken
        // used to fall into the second halfword (0x9a26) and run garbage.
        use crate::firmware::emu::bus::Bus;
        use crate::firmware::emu::cpu::Cpu;
        // bmi.w +0x120 (00 f1 90 80) at 0, then 16-bit beq +0 (00 d0) at 4.
        let mut flash = vec![0u8; 0x100];
        flash[0..6].copy_from_slice(&[0x00, 0xf1, 0x90, 0x80, 0x00, 0xd0]);
        let mut bus = Bus::new(flash, 0x1000);
        let mut cpu = Cpu::new();
        cpu.n = false; // MI false -> not taken
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.pc, 4, "T3 not-taken must advance 4");
        // 16-bit beq at 4, Z clear -> not taken, advance 2.
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.pc, 6, "16-bit not-taken must advance 2");
    }

    #[test]
    fn test_decode32_push_w_lr() {
        let i = decode32(0xe92d, 0x4ff0).unwrap();
        match i {
            Instr::Thumb2(Thumb2::Stmdb { list }) => {
                assert_eq!(list & 0x0FF0, 0x0FF0); // r4..r11
                assert!(list & (1 << 14) != 0);    // lr
            }
            _ => panic!("expected Stmdb, got {:?}", i),
        }
    }

    #[test]
    fn test_decode32_pop_w_pc() {
        // pop.w {r4-r11, pc} = e8bd 8ff0 (Capstone-verified)
        let i = decode32(0xe8bd, 0x8ff0).unwrap();
        match i {
            Instr::Thumb2(Thumb2::Ldmia { list }) => {
                assert_eq!(list & 0x0FF0, 0x0FF0); // r4..r11
                assert!(list & (1 << 15) != 0);    // pc
            }
            _ => panic!("expected Ldmia, got {:?}", i),
        }
    }

    #[test]
    fn test_decode32_stm_w_no_writeback() {
        // `stm.w sp, {r7, sl}` = e88d 0480 (Capstone-verified, af_190602
        // 0x8e02). W=0 => STMIA with NO writeback, not STMDB sp!.
        let i = decode32(0xe88d, 0x0480).unwrap();
        match i {
            Instr::Thumb2(Thumb2::StmIA { rn, list }) => {
                assert_eq!(rn, 13);           // sp
                assert_eq!(list, 0x0480);     // r7 + r10 (sl)
            }
            _ => panic!("expected StmIA, got {:?}", i),
        }
    }

    #[test]
    fn test_decode32_strb_t4_pre_sub_fields() {
        // `strb r7, [lr, #-1]!` = F80E 7D01 (Capstone-verified): hw2 bits
        // 11:8 = 1101, so P (bit 10) = 1 = pre-indexed, U (bit 9) = 0 =
        // subtract. The pre/sub fields were originally read from swapped
        // bits, decoding this as a post-indexed add.
        let i = decode32(0xF80E, 0x7D01).unwrap();
        match i {
            Instr::Thumb2(Thumb2::StrbT4 { rt, rn, imm, pre, sub }) => {
                assert_eq!((rt, rn, imm), (7, 14, 1));
                assert!(pre && sub);
            }
            _ => panic!("expected StrbT4, got {:?}", i),
        }
    }

    #[test]
    fn test_decode32_tst_w() {
        // `tst.w r3, #0x40` at 0x8c8a of af_190602 (the gate's first DpImm hit).
        // Capstone-verified: bytes 13 f0 40 0f -> tst.w r3, #0x40.
        let i = decode32(0xF013, 0x0F40).unwrap();
        match i {
            Instr::Thumb2(Thumb2::DpImm { op, rd, rn, set_flags, imm, shifter_c }) => {
                assert_eq!(op, DpOp::And);
                assert_eq!(rd, 15); // TST: rd = 15
                assert_eq!(rn, 3);
                assert!(set_flags);
                assert_eq!(imm, 0x40);
                assert_eq!(shifter_c, None); // plain imm8 form preserves old C
            }
            _ => panic!("expected DpImm TST, got {:?}", i),
        }
    }

    #[test]
    fn test_decode32_ldrh_imm() {
        // Capstone-verified: ldrh.w lr, [sp, #0x20] = hw1 0xF8BD, hw2 0xE020
        // (af_190602 0x14100).
        let i = decode32(0xF8BD, 0xE020).unwrap();
        match i {
            Instr::Thumb2(Thumb2::LdrhImm { rt, rn, imm }) => {
                assert_eq!((rt, rn, imm), (14, 13, 0x20));
            }
            _ => panic!("expected LdrhImm, got {:?}", i),
        }
    }

    #[test]
    fn test_decode32_ldr_imm() {
        // Capstone-verified: ldr.w r2, [r8, #0] = hw1 0xF8D8, hw2 0x2000
        // (af_190602 0x8d0a).
        let i = decode32(0xF8D8, 0x2000).unwrap();
        match i {
            Instr::Thumb2(Thumb2::LdrImm { rt, rn, imm }) => {
                assert_eq!((rt, rn, imm), (2, 8, 0));
            }
            _ => panic!("expected LdrImm, got {:?}", i),
        }
    }

    #[test]
    fn test_decode32_sbfx() {
        // Capstone-verified: sbfx r6, r6, #0x12, #1 = hw1 0xF346, hw2 0x4680
        // (af_190602 0x8ce8).
        let i = decode32(0xF346, 0x4680).unwrap();
        match i {
            Instr::Thumb2(Thumb2::Sbfx { rd, rn, lsbit, width }) => {
                assert_eq!((rd, rn, lsbit, width), (6, 6, 0x12, 1));
            }
            _ => panic!("expected Sbfx, got {:?}", i),
        }
    }

    #[test]
    fn test_decode32_ubfx() {
        // Capstone-verified: ubfx r1, r2, #0, #9 = hw1 0xF3C2, hw2 0x0108
        // (af_190602 0x18e).
        let i = decode32(0xF3C2, 0x0108).unwrap();
        match i {
            Instr::Thumb2(Thumb2::Ubfx { rd, rn, lsbit, width }) => {
                assert_eq!((rd, rn, lsbit, width), (1, 2, 0, 9));
            }
            _ => panic!("expected Ubfx, got {:?}", i),
        }
    }

    #[test]
    fn test_decode32_strb_w_reg() {
        // Capstone-verified: strb.w r4, [r3, r0] = hw1 0xF803, hw2 0x4000.
        let i = decode32(0xF803, 0x4000).unwrap();
        match i {
            Instr::Thumb2(Thumb2::LdrStrReg { load, size, rt, rn, rm, shift }) => {
                assert_eq!((load, size, rt, rn, rm, shift), (false, 1, 4, 3, 0, 0));
            }
            _ => panic!("expected LdrStrReg STRB, got {:?}", i),
        }
    }

    #[test]
    fn test_decode32_ldr_w_reg_shifted() {
        // Capstone-verified: ldr.w r3, [r3, r1, lsl #2] = hw1 0xF853, hw2 0x3021
        // (af_190602 0x1cc).
        let i = decode32(0xF853, 0x3021).unwrap();
        match i {
            Instr::Thumb2(Thumb2::LdrStrReg { load, size, rt, rn, rm, shift }) => {
                assert_eq!((load, size, rt, rn, rm, shift), (true, 4, 3, 3, 1, 2));
            }
            _ => panic!("expected LdrStrReg LDR, got {:?}", i),
        }
    }

    #[test]
    fn test_decode32_sub_w_reg() {
        // Capstone-verified: sub.w lr, r1, r0 = hw1 0xEBA1, hw2 0x0E00
        // (af_190602 0x12bc0).
        let i = decode32(0xEBA1, 0x0E00).unwrap();
        match i {
            Instr::Thumb2(Thumb2::DpReg { op, rd, rn, rm, stype, amount, set_flags }) => {
                assert_eq!((op, rd, rn, rm, stype, amount, set_flags),
                           (DpRegOp::Sub, 14, 1, 0, 0, 0, false));
            }
            _ => panic!("expected DpReg SUB, got {:?}", i),
        }
    }

    #[test]
    fn test_decode32_lsl_w_reg_shift() {
        // Capstone-verified: lsl.w ip, r4, r5 = hw1 0xFA04, hw2 0xFC05
        // (af_190602 0x12bf8).
        let i = decode32(0xFA04, 0xFC05).unwrap();
        match i {
            Instr::Thumb2(Thumb2::MovShiftReg { rd, rn, rm, stype, set_flags }) => {
                assert_eq!((rd, rn, rm, stype, set_flags), (12, 4, 5, 0, false));
            }
            _ => panic!("expected MovShiftReg, got {:?}", i),
        }
    }

    #[test]
    fn test_decode32_movw_movt() {
        // Capstone-verified: movw r3, #0x2327 = hw1 0xF242, hw2 0x3327
        // (af_190602 0x1c418); movt r3, #0x1234 = hw1 0xF2C1, hw2 0x2334.
        let i = decode32(0xF242, 0x3327).unwrap();
        match i {
            Instr::Thumb2(Thumb2::Movw { rd, imm16 }) => {
                assert_eq!((rd, imm16), (3, 0x2327));
            }
            _ => panic!("expected Movw, got {:?}", i),
        }
        let i = decode32(0xF2C1, 0x2334).unwrap();
        match i {
            Instr::Thumb2(Thumb2::Movt { rd, imm16 }) => {
                assert_eq!((rd, imm16), (3, 0x1234));
            }
            _ => panic!("expected Movt, got {:?}", i),
        }
    }

    #[test]
    fn test_thumb_expand_imm_rotated_forms() {
        // Capstone-verified vectors (keystone-assembled cmp.w r2, #imm):
        // the rotated form is ROR(0x80 | imm8[6:0], imm12[11:7]); the earlier
        // implementation rotated raw imm8 by 2*bits[9:8] and decoded
        // cmp.w lr, #0x400 (F5BE 6F80, af_190602 0x12c0c) as 0x00800080,
        // which broke the display-buffer bound check and caused ACL
        // violation write cascades past 0x20002b58.
        assert_eq!(thumb_expand_imm(0xE80), 0x400);
        assert_eq!(thumb_expand_imm(0xC00), 0x8000);
        assert_eq!(thumb_expand_imm(0x780), 0x0100_0000);
        assert_eq!(thumb_expand_imm(0xC7F), 0xFF00);
        assert_eq!(thumb_expand_imm(0x1A5), 0x00A5_00A5);
        assert_eq!(thumb_expand_imm(0x040), 0x40);
        // cmp.w lr, #0x400 = F5BE 6F80 (af_190602 0x12c0c).
        let i = decode32(0xF5BE, 0x6F80).unwrap();
        match i {
            Instr::Thumb2(Thumb2::DpImm { op, rd, rn, set_flags, imm, shifter_c }) => {
                assert_eq!((op, rd, rn, set_flags, imm), (DpOp::Sub, 15, 14, true, 0x400));
                assert_eq!(shifter_c, Some(false)); // rotated form, bit31 = 0
            }
            _ => panic!("expected DpImm CMP, got {:?}", i),
        }
    }

    #[test]
    fn test_decode32_bic_w_asr31() {
        // Capstone-verified: bic.w r1, r0, r0, asr #31 = hw1 0xEA20, hw2 0x71E0
        // (af_190602 0x13cde).
        let i = decode32(0xEA20, 0x71E0).unwrap();
        match i {
            Instr::Thumb2(Thumb2::DpReg { op, rd, rn, rm, stype, amount, set_flags }) => {
                assert_eq!((op, rd, rn, rm, stype, amount, set_flags),
                           (DpRegOp::Bic, 1, 0, 0, 2, 31, false));
            }
            _ => panic!("expected DpReg BIC, got {:?}", i),
        }
    }

    #[test]
    fn test_decode32_lsl_w_mov_alias() {
        // Capstone-verified: lsl.w sb, sl, #0xc = hw1 0xEA4F, hw2 0x390A
        // (MOV alias: rn = 15, op = ORR).
        let i = decode32(0xEA4F, 0x390A).unwrap();
        match i {
            Instr::Thumb2(Thumb2::DpReg { op, rd, rn, rm, stype, amount, set_flags }) => {
                assert_eq!((op, rd, rn, rm, stype, amount, set_flags),
                           (DpRegOp::Orr, 9, 15, 10, 0, 12, false));
            }
            _ => panic!("expected DpReg MOV alias, got {:?}", i),
        }
    }

    #[test]
    fn test_decode32_tst_w_reg() {
        // Capstone-verified: tst.w r5, r6, lsl #2 = hw1 0xEA15, hw2 0x0F86
        // (AND alias: rd = 15, S = 1).
        let i = decode32(0xEA15, 0x0F86).unwrap();
        match i {
            Instr::Thumb2(Thumb2::DpReg { op, rd, rn, rm, stype, amount, set_flags }) => {
                assert_eq!((op, rd, rn, rm, stype, amount, set_flags),
                           (DpRegOp::And, 15, 5, 6, 0, 2, true));
            }
            _ => panic!("expected DpReg TST alias, got {:?}", i),
        }
    }

    #[test]
    fn test_decode32_strb_imm() {
        // Capstone-verified: strb.w r3, [sp, #4] = hw1 0xF88D, hw2 0x3004
        // (af_190602 0x160).
        let i = decode32(0xF88D, 0x3004).unwrap();
        match i {
            Instr::Thumb2(Thumb2::StrbImm { rt, rn, imm }) => {
                assert_eq!((rt, rn, imm), (3, 13, 4));
            }
            _ => panic!("expected StrbImm, got {:?}", i),
        }
    }

    #[test]
    fn test_decode32_strh_imm() {
        // Capstone-verified: strh.w r2, [r5, #0x2a] = hw1 0xF8A5, hw2 0x202A.
        let i = decode32(0xF8A5, 0x202A).unwrap();
        match i {
            Instr::Thumb2(Thumb2::StrhImm { rt, rn, imm }) => {
                assert_eq!((rt, rn, imm), (2, 5, 0x2A));
            }
            _ => panic!("expected StrhImm, got {:?}", i),
        }
    }

    #[test]
    fn test_decode32_udiv() {
        // Capstone-verified: udiv r3, r3, r2 = hw1 0xFBB3, hw2 0xF3F2
        // (af_190602 0x1ac).
        let i = decode32(0xFBB3, 0xF3F2).unwrap();
        match i {
            Instr::Thumb2(Thumb2::Udiv { rd, rn, rm }) => {
                assert_eq!((rd, rn, rm), (3, 3, 2));
            }
            _ => panic!("expected Udiv, got {:?}", i),
        }
    }
}
