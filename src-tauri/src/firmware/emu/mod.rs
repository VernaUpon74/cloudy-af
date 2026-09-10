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
