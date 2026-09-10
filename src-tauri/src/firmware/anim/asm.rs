use std::collections::HashMap;

/// Errors that can occur while assembling Thumb bytecode.
#[derive(Debug, thiserror::Error)]
pub enum AsmError {
    #[error("branch fixup to label '{0}' is out of range")]
    BranchOutOfRange(String),
    #[error("literal load fixup to label '{0}' is out of range")]
    LiteralOutOfRange(String),
    #[error("undefined label '{0}'")]
    UndefinedLabel(String),
    #[error("duplicate label '{0}'")]
    DuplicateLabel(String),
}

/// Errors that can occur while building an animation patch.
#[derive(Debug, thiserror::Error)]
pub enum AnimError {
    #[error("assembly error: {0}")]
    Asm(#[from] AsmError),
    #[error("descriptor error: {0}")]
    Descriptor(String),
    #[error("cave overflow: body {body} bytes, cave size {cave}")]
    CaveOverflow { body: usize, cave: usize },
}

/// Minimal Thumb-1 bytecode builder.
///
/// Emits 16-bit instructions in little-endian byte order. Labels and literal
/// pool references are resolved in `finish`. The literal pool is appended
/// after the code and 4-byte aligned, matching the emulator's `LdrLit`
/// semantics (`addr = align(pc + 4, 4) + imm8 * 4`).
pub struct Asm {
    base: u32,
    code: Vec<u8>,
    labels: HashMap<String, u32>,
    fixups: Vec<Fixup>,
    pool: Vec<(String, u32)>,
}

#[derive(Debug, Clone)]
enum Fixup {
    B11 { offset: u32, label: String },
    BCond8 { offset: u32, label: String, cond: u8 },
    LdrLit8 { offset: u32, label: String, rt: u8 },
}

impl Asm {
    pub fn new(base: u32) -> Self {
        Self {
            base,
            code: Vec::new(),
            labels: HashMap::new(),
            fixups: Vec::new(),
            pool: Vec::new(),
        }
    }

    fn emit(&mut self, hw: u16) -> &mut Self {
        self.code.extend_from_slice(&hw.to_le_bytes());
        self
    }

    fn offset(&self) -> u32 {
        self.code.len() as u32
    }

    pub fn movs(&mut self, rd: u8, imm: u8) -> &mut Self {
        self.emit(0x2000 | (((rd & 0x7) as u16) << 8) | imm as u16)
    }

    /// `nop` hint (0x46C0) — also used by finish() for pool alignment.
    pub fn nop(&mut self) -> &mut Self {
        self.emit(0x46C0)
    }

    /// `ldr rt, [pc, #imm8*4]` — reference to a literal pool entry labelled
    /// with `label`. The pool entry must be declared with `pool_word` before
    /// `finish` is called.
    pub fn ldr_lit(&mut self, rt: u8, label: &str) -> &mut Self {
        self.fixups.push(Fixup::LdrLit8 {
            offset: self.offset(),
            label: label.to_string(),
            rt: rt & 0x7,
        });
        self.emit(0x4800 | (((rt & 0x7) as u16) << 8)) // imm8 placeholder
    }

    /// `ldr rt, [rn, #imm5*4]` — word load.
    pub fn ldr_imm(&mut self, rt: u8, rn: u8, imm5: u8) -> &mut Self {
        self.emit(0x6800 | (((imm5 & 0x1F) as u16) << 6) | (((rn & 0x7) as u16) << 3) | (rt & 0x7) as u16)
    }

    /// `str rt, [rn, #imm5*4]` — word store.
    pub fn str_imm(&mut self, rt: u8, rn: u8, imm5: u8) -> &mut Self {
        self.emit(0x6000 | (((imm5 & 0x1F) as u16) << 6) | (((rn & 0x7) as u16) << 3) | (rt & 0x7) as u16)
    }

    /// `strb rt, [rn, #imm5]` — byte store.
    pub fn strb_imm(&mut self, rt: u8, rn: u8, imm5: u8) -> &mut Self {
        self.emit(0x7000 | (((imm5 & 0x1F) as u16) << 6) | (((rn & 0x7) as u16) << 3) | (rt & 0x7) as u16)
    }

    /// `ldrb rt, [rn, #imm5]` — byte load.
    pub fn ldrb_imm(&mut self, rt: u8, rn: u8, imm5: u8) -> &mut Self {
        self.emit(0x7800 | (((imm5 & 0x1F) as u16) << 6) | (((rn & 0x7) as u16) << 3) | (rt & 0x7) as u16)
    }

    /// Register-offset load/store: `op` encodes the operation as in
    /// `thumb.rs::LsReg`:
    ///   0 = STR, 2 = STRB, 4 = LDR, 6 = LDRB.
    pub fn ls_reg(&mut self, op: u8, rt: u8, rn: u8, rm: u8) -> &mut Self {
        self.emit(
            0x5000
                | (((op & 0x7) as u16) << 9)
                | (((rn & 0x7) as u16) << 6)
                | (((rm & 0x7) as u16) << 3)
                | (rt & 0x7) as u16,
        )
    }

    /// `adds rd, rn, rm` — low-register add, sets flags.
    pub fn adds_reg(&mut self, rd: u8, rn: u8, rm: u8) -> &mut Self {
        self.emit(0x1800 | (((rm & 0x7) as u16) << 6) | (((rn & 0x7) as u16) << 3) | (rd & 0x7) as u16)
    }

    pub fn adds(&mut self, rdn: u8, imm8: u8) -> &mut Self {
        self.emit(0x3000 | (((rdn & 0x7) as u16) << 8) | imm8 as u16)
    }

    pub fn subs(&mut self, rdn: u8, imm8: u8) -> &mut Self {
        self.emit(0x3800 | (((rdn & 0x7) as u16) << 8) | imm8 as u16)
    }

    pub fn cmp(&mut self, rn: u8, imm8: u8) -> &mut Self {
        self.emit(0x2800 | (((rn & 0x7) as u16) << 8) | imm8 as u16)
    }

    pub fn ands(&mut self, rdn: u8, rm: u8) -> &mut Self {
        self.emit(0x4000 | (((rm & 0x7) as u16) << 3) | (rdn & 0x7) as u16)
    }

    pub fn lsls(&mut self, rd: u8, rm: u8, imm5: u8) -> &mut Self {
        self.emit((((imm5 & 0x1F) as u16) << 6) | (((rm & 0x7) as u16) << 3) | (rd & 0x7) as u16)
    }

    pub fn lsrs(&mut self, rd: u8, rm: u8, imm5: u8) -> &mut Self {
        self.emit(0x0800 | (((imm5 & 0x1F) as u16) << 6) | (((rm & 0x7) as u16) << 3) | (rd & 0x7) as u16)
    }

    pub fn muls(&mut self, rdm: u8, rn: u8) -> &mut Self {
        self.emit(0x4340 | (((rn & 0x7) as u16) << 3) | (rdm & 0x7) as u16)
    }

    pub fn push(&mut self, regs: &[u8]) -> &mut Self {
        let mut list: u16 = 0;
        for &r in regs {
            list |= 1 << (r & 0xF);
        }
        self.emit(0xB400 | list)
    }

    pub fn pop(&mut self, regs: &[u8]) -> &mut Self {
        let mut list: u16 = 0;
        for &r in regs {
            list |= 1 << (r & 0xF);
        }
        self.emit(0xBC00 | list)
    }

    /// Unconditional branch to `label`. Range: ±2048 bytes (11-bit halfword offset).
    pub fn b(&mut self, label: &str) -> &mut Self {
        self.fixups.push(Fixup::B11 {
            offset: self.offset(),
            label: label.to_string(),
        });
        self.emit(0xE000) // imm11 placeholder
    }

    /// Conditional branch to `label`. `cond` is the ARM condition code nibble.
    pub fn bcond(&mut self, cond: u8, label: &str) -> &mut Self {
        self.fixups.push(Fixup::BCond8 {
            offset: self.offset(),
            label: label.to_string(),
            cond: cond & 0xF,
        });
        self.emit(0xD000 | (((cond & 0xF) as u16) << 8)) // imm8 placeholder
    }

    /// Define a label at the current code offset.
    pub fn label(&mut self, name: &str) -> &mut Self {
        if self.labels.insert(name.to_string(), self.offset()).is_some() {
            panic!("duplicate label: {name}"); // build-time bug, not runtime error
        }
        self
    }

    /// Add a 32-bit word to the literal pool. The same `label` is used with
    /// `ldr_lit` to load the address of this word.
    pub fn pool_word(&mut self, label: &str, value: u32) -> &mut Self {
        self.pool.push((label.to_string(), value));
        self
    }

    /// Resolve labels, append the literal pool with 4-byte alignment, and
    /// return the assembled bytes.
    pub fn finish(mut self) -> Result<Vec<u8>, AsmError> {
        // Pad code to 4-byte alignment so the literal pool starts on a word
        // boundary, matching the PC alignment used by LdrLit.
        while self.code.len() % 4 != 0 {
            self.code.extend_from_slice(&0x46C0u16.to_le_bytes()); // nop
        }

        // Append pool entries. Each label maps to the byte offset of its word.
        let mut pool_offsets: HashMap<String, u32> = HashMap::new();
        for (label, value) in &self.pool {
            pool_offsets.insert(label.clone(), self.code.len() as u32);
            self.code.extend_from_slice(&value.to_le_bytes());
        }

        // Resolve fixups in place.
        for fixup in &self.fixups {
            match fixup {
                Fixup::B11 { offset, label } => {
                    let target = *self.labels.get(label).ok_or_else(|| AsmError::UndefinedLabel(label.clone()))?;
                    let pc = offset + 4; // branch offset is from pc+4
                    let off = (target as i64 - pc as i64) / 2;
                    if !(-2048..=2047).contains(&off) {
                        return Err(AsmError::BranchOutOfRange(label.clone()));
                    }
                    let imm11 = (off as i32 as u16) & 0x07FF;
                    let hw = 0xE000 | imm11;
                    let base = *offset as usize;
                    self.code[base] = hw as u8;
                    self.code[base + 1] = (hw >> 8) as u8;
                }
                Fixup::BCond8 { offset, label, cond } => {
                    let target = *self.labels.get(label).ok_or_else(|| AsmError::UndefinedLabel(label.clone()))?;
                    let pc = offset + 4;
                    let off = (target as i64 - pc as i64) / 2;
                    // Conditional branch imm8 is a halfword offset; range ±128 halfwords.
                    if !(-128..=127).contains(&off) {
                        return Err(AsmError::BranchOutOfRange(label.clone()));
                    }
                    let imm8 = (off as i32 as u16) & 0x00FF;
                    let hw = 0xD000 | ((*cond as u16) << 8) | imm8;
                    let base = *offset as usize;
                    self.code[base] = hw as u8;
                    self.code[base + 1] = (hw >> 8) as u8;
                }
                Fixup::LdrLit8 { offset, label, rt } => {
                    let pool_off = *pool_offsets.get(label).ok_or_else(|| AsmError::UndefinedLabel(label.clone()))?;
                    // `pc` is already InstrAddr + 4 (the ARM pipeline
                    // adjustment); the literal base is that value aligned
                    // DOWN to 4 bytes. Verified against emu_test.rs's
                    // hand-assembled cave: `ldr r1,[pc,#8]` at buffer offset
                    // 2 with the pool at offset 12 must encode imm8 = 2.
                    let pc = offset + 4;
                    let base = pc & !3;
                    let imm8 = (pool_off as i64 - base as i64) / 4;
                    if !(0..=255).contains(&imm8) {
                        return Err(AsmError::LiteralOutOfRange(label.clone()));
                    }
                    let hw = 0x4800 | ((*rt as u16) << 8) | (imm8 as u16 & 0xFF);
                    let base = *offset as usize;
                    self.code[base] = hw as u8;
                    self.code[base + 1] = (hw >> 8) as u8;
                }
            }
        }

        Ok(self.code)
    }

    /// The base address used for branch/literal calculations.
    pub fn base(&self) -> u32 {
        self.base
    }

    /// Current code length in bytes.
    pub fn len(&self) -> usize {
        self.code.len()
    }
}

/// 4-byte Thumb-2 `B.W` pair (two halfwords) branching from `from` to `to`.
/// This is the hook-site detour primitive: overwrite 4 bytes at `from` with
/// this pair to divert execution into a code cave.
///
/// Golden test (phase-3 plan Task 4): `bw_pair(0x20, 0x60)` = `(0xF000, 0xB81E)`.
pub fn bw_pair(from: u32, to: u32) -> [u16; 2] {
    // B.W T4: hw1 = 11110 S imm10, hw2 = 10 J1 1 J2 imm11; offset is from
    // (from + 4), sign-extended 25 bits. Same math as the emulator's
    // decode32 B arm (thumb.rs), which the BL gate vectors validate.
    let off = (to.wrapping_sub(from.wrapping_add(4)) as i32) & 0x01FF_FFFF;
    let s = ((off >> 24) & 1) as u16;
    let i1 = ((off >> 23) & 1) as u16;
    let i2 = ((off >> 22) & 1) as u16;
    let j1 = s ^ (1 - i1); // invert: i1 = 1 - (j1 ^ s)
    let j2 = s ^ (1 - i2);
    let imm10 = ((off >> 12) & 0x3FF) as u16;
    let imm11 = ((off >> 1) & 0x7FF) as u16;
    let hw1 = 0xF000 | (s << 10) | imm10;
    let hw2 = 0x8000 | (j1 << 13) | 0x1000 | (j2 << 11) | imm11;
    [hw1, hw2]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_golden_movs() {
        // movs r0, #0xFF = 0x20FF. finish() pads the code to a 4-byte
        // boundary with a nop so the literal pool stays word-aligned.
        let mut a = Asm::new(0x60);
        a.movs(0, 0xFF);
        assert_eq!(a.finish().unwrap(), vec![0xFF, 0x20, 0xC0, 0x46]);
    }

    #[test]
    fn test_golden_bcond_backward() {
        // bne back to "loop": adds(2,1) = 0x3201, cmp(2,16) = 0x2A10, then
        // bcond at offset 4 -> pc 8, off = (0-8)/2 = -4 -> imm8 0xFC.
        // Capstone-verified: bytes [01 32 10 2A FC D1] at 0x08 disassemble as
        // adds r2,#1 ; cmp r2,#0x10 ; bne #0x08.
        let mut a = Asm::new(0x08);
        a.label("loop");
        a.adds(2, 1); // 0x3201
        a.cmp(2, 16); // 0x2A10
        a.bcond(1, "loop");
        assert_eq!(
            a.finish().unwrap(),
            vec![0x01, 0x32, 0x10, 0x2A, 0xFC, 0xD1, 0xC0, 0x46]
        );
    }

    #[test]
    fn test_bw_pair_golden() {
        // b.w from 0x20 to 0x60: off = 0x3C, S=0, imm10=0, J1=J2=1, imm11=0x1E
        // (phase-3 plan Task 4 golden vector).
        assert_eq!(bw_pair(0x20, 0x60), [0xF000, 0xB81E]);
    }

    #[test]
    fn test_bw_pair_roundtrip_through_emulator_decode() {
        // Every bw_pair output must decode as a B.W to `to` under the
        // emulator's own decoder (which the gate exercises).
        for (from, to) in [(0x20u32, 0x60u32), (0x9a16, 0x1d000), (0x1d000, 0x9a1a), (0x100, 0x80)] {
            let [hw1, hw2] = bw_pair(from, to);
            match crate::firmware::emu::thumb::decode32(hw1, hw2) {
                Some(crate::firmware::emu::thumb::Instr::Thumb2(
                    crate::firmware::emu::thumb::Thumb2::B { off },
                )) => assert_eq!(from.wrapping_add(4).wrapping_add(off as u32), to, "from {from:#x} to {to:#x}"),
                other => panic!("bw_pair({from:#x}, {to:#x}) decoded as {other:?}"),
            }
        }
    }

    #[test]
    fn test_cave_body_roundtrip_through_emulator() {
        // Cave body in the shape of emu_test.rs::hook_patch (shorter: no
        // branch-back/nop, so the pool sits at offset 8):
        //   movs r0, #0xFF ; ldr r1, [pc, #imm8*4] ; strb r0, [r1, #15] ;
        //   <align pad> ; pool: display buffer address
        // The real invariant is that the emulator's LdrLit semantics deliver
        // the pool word, so run it through the emu rather than asserting
        // hardcoded imm8 bytes.
        let mut a = Asm::new(0);
        a.movs(0, 0xFF);
        a.ldr_lit(1, "buf");
        a.strb_imm(0, 1, 15);
        a.pool_word("buf", 0x2000_1000);
        let bytes = a.finish().unwrap();
        assert_eq!(bytes.len() % 4, 0);

        let mut bus = crate::firmware::emu::bus::Bus::new(bytes, 0x2000);
        bus.allow_region(0x2000_0000..0x2000_1010);
        let mut cpu = crate::firmware::emu::cpu::Cpu::new();
        cpu.step(&mut bus).unwrap(); // movs
        cpu.step(&mut bus).unwrap(); // ldr r1, [pc, #...]
        assert_eq!(cpu.r[1], 0x2000_1000, "ldr_lit must deliver the pool word");
        cpu.step(&mut bus).unwrap(); // strb r0, [r1, #15]
        assert_eq!(bus.read_u8(0x2000_100F).unwrap(), 0xFF);
    }

    #[test]
    fn test_branch_out_of_range_errors() {
        let mut a = Asm::new(0);
        a.label("near");
        a.b("near");
        assert!(a.finish().is_ok());
        let mut a = Asm::new(0);
        a.label("far");
        for _ in 0..3000 {
            a.nop();
        }
        a.b("far");
        assert!(matches!(a.finish(), Err(AsmError::BranchOutOfRange(_))));
    }
}


