//! Descriptor-driven test harness: builds a Bus+Cpu from a JSON descriptor,
//! runs one render-function call, and extracts the display buffer as a Frame.

use super::bus::{Bus, RAM_BASE};
use super::cpu::Cpu;
use super::EmuError;
use serde::Deserialize;

/// Branch target `bx lr` lands on when the render function returns; the run
/// loop stops there. Even address, never valid Thumb code.
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
pub struct Stub {
    #[serde(deserialize_with = "de_hex")]
    pub addr: u32,
    pub value: u32,
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
    pub ram_size: usize, // 0 -> default 0x8000
    #[serde(default)]
    pub args: [u32; 4], // r0..r3 at entry
    /// MMIO stubs: reads at `addr` return `value`, writes are dropped.
    /// Used for the animation config byte (dataflash-mapped) so patched
    /// firmware runs in the emulator without a dataflash model.
    #[serde(default)]
    pub stubs: Vec<Stub>,
}

/// Deserialize a u32 from a string: "0x20001000" hex or "536879104" decimal.
pub fn de_hex<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    let s = String::deserialize(d)?;
    let v = if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16)
    } else {
        s.parse::<u32>()
    };
    v.map_err(serde::de::Error::custom)
}

pub fn load_descriptor(json: &str) -> Result<Descriptor, EmuError> {
    serde_json::from_str(json).map_err(|e| EmuError::Descriptor(e.to_string()))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>, // 0|1, row-major
}

pub struct Harness {
    pub cpu: Cpu,
    pub bus: Bus,
    pub desc: Descriptor,
    /// Bottom of the allowed stack region: the canary lives here, and sp is
    /// never allowed to dip below it.
    stack_bottom: u32,
    /// Address of the stack canary (0xDEAD_C0DE), re-checked after every run.
    canary_addr: u32,
}

impl Harness {
    pub fn new(image: &[u8], desc: Descriptor) -> Self {
        let ram_size = if desc.ram_size == 0 { 0x8000 } else { desc.ram_size };
        let mut bus = Bus::new(image.to_vec(), ram_size);
        bus.allow_region(desc.display_buffer.range.start..desc.display_buffer.range.end);
        for g in &desc.ram_globals {
            bus.allow_region(g.start..g.end);
        }
        for s in &desc.stubs {
            bus.set_stub(s.addr, s.value);
        }
        let ram_top = RAM_BASE + ram_size as u32;
        // Real charge-screen render code uses deep call stacks; give it 8 KiB
        // instead of 4 KiB so nested push.w {r4..fp,lr} + local frames fit.
        let stack_size = 0x2000;
        bus.allow_region(ram_top - stack_size..ram_top); // stack
        let mut cpu = Cpu::new();
        cpu.sp = ram_top - 0x100; // 8-aligned, leave headroom for large pop.w frames
        // The canary sits at the BOTTOM of the stack region, not just below
        // the frame: normal prologue pushes clobber anything near the initial
        // sp, while a write reaching the region bottom means the stack was
        // effectively exhausted. (Writes past the bottom fault via the ACL.)
        let stack_bottom = ram_top - stack_size;
        let canary_addr = stack_bottom;
        bus.write_u32(canary_addr, 0xDEAD_C0DE)
            .expect("canary write lands inside the stack allow region");
        cpu.lr = RETURN_SENTINEL | 1;
        cpu.r[..4].copy_from_slice(&desc.args);
        Self { cpu, bus, desc, stack_bottom, canary_addr }
    }

    /// Run the render entry point once and unpack the display buffer.
    /// Any ACL violation during the run is the anti-brick signal: report it
    /// as a Descriptor error (with a cpu dump) so the test fails loudly.
    pub fn run_frame(&mut self, budget: u64) -> Result<Frame, EmuError> {
        self.cpu.pc = self.desc.render_entry & !1;
        self.cpu.lr = RETURN_SENTINEL | 1;
        self.cpu.min_sp = self.cpu.sp;
        self.cpu.run_until(&mut self.bus, RETURN_SENTINEL, budget)?;
        if !self.bus.acl_violations.is_empty() {
            return Err(EmuError::Descriptor(format!(
                "ACL violations: {:?}\n{}",
                self.bus.acl_violations,
                self.cpu.debug_dump()
            )));
        }
        // Stack-exhaustion checks. The canary catches writes that reach the
        // region bottom (allowed by the ACL, so otherwise silent); the min-sp
        // check catches the stack pointer dipping below the region without a
        // store (e.g. `sub sp, #big` followed by a return).
        if self.cpu.min_sp < self.stack_bottom {
            return Err(EmuError::Descriptor(format!(
                "stack pointer bottomed out at {:#010x} (region bottom {:#010x})\n{}",
                self.cpu.min_sp,
                self.stack_bottom,
                self.cpu.debug_dump()
            )));
        }
        let canary = self.bus.read_u32(self.canary_addr)?;
        if canary != 0xDEAD_C0DE {
            return Err(EmuError::Descriptor(format!(
                "stack canary corrupted ({canary:#010x}) — stack usage reached the region bottom\n{}",
                self.cpu.debug_dump()
            )));
        }
        let start = self.desc.display_buffer.range.start;
        let len = (self.desc.display_buffer.range.end - start) as usize;
        let mut buf = vec![0u8; len];
        for (i, b) in buf.iter_mut().enumerate() {
            *b = self.bus.read_u8(start + i as u32)?;
        }
        Ok(unpack_block1(
            &buf,
            self.desc.display_buffer.width,
            self.desc.display_buffer.height,
        ))
    }

    pub fn frames_differ(a: &Frame, b: &Frame) -> bool {
        a.width != b.width || a.height != b.height || a.pixels != b.pixels
    }
}

/// Horizontal 1bpp packing, MSB = leftmost pixel: byte `y*(width/8) + x/8`,
/// bit `0x80 >> (x%8)` — the GDI+ `Format1bppIndexed` layout NToolbox copies
/// the 0xC1 screenshot bytes into verbatim (CreateBitmapFromBytesArray),
/// confirmed on hardware (Pico 25 capture renders readable text only under
/// this packing). The earlier "Block1 vertical packing" decode was wrong for
/// the on-device framebuffer; the on-pixel COUNT is packing-invariant
/// (popcount), so golden pixel counts stay valid.
pub fn unpack_block1(buf: &[u8], width: usize, height: usize) -> Frame {
    let stride = width / 8;
    let mut pixels = vec![0u8; width * height];
    for y in 0..height {
        for x in 0..width {
            let byte = buf[y * stride + x / 8];
            pixels[y * width + x] = (byte >> (7 - (x % 8))) & 1;
        }
    }
    Frame { width, height, pixels }
}

/// Binary P5 PGM, maxval 255, pixel on = 255.
pub fn dump_pgm(frame: &Frame, path: &std::path::Path) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    write!(f, "P5\n{} {}\n255\n", frame.width, frame.height)?;
    let data: Vec<u8> = frame.pixels.iter().map(|p| p * 255).collect();
    f.write_all(&data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::firmware::emu::bus::RAM_BASE;

    const DESC_JSON: &str = r#"{
        "build": "synthetic",
        "render_entry": "0x00000080",
        "display_buffer": { "start": "0x20001000", "end": "0x20001400", "width": 64, "height": 128 },
        "ram_globals": [ { "start": "0x20000000", "end": "0x20000100" } ],
        "ram_size": 32768,
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
        assert_eq!(frame.pixels[0], 1);       // x=0,y=0 set by the 0xFF byte (MSB)
        assert_eq!(frame.pixels[7], 1);       // x=7,y=0: bit 0 of the same byte
        assert_eq!(frame.pixels[8], 0);       // x=8,y=0: byte 1 untouched
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
        // store at RAM_BASE (not allowed once ram_globals is cleared):
        let mut img = synthetic_image();
        img[0x8C..0x90].copy_from_slice(&RAM_BASE.to_le_bytes()); // store target
        let mut d = load_descriptor(DESC_JSON).unwrap();
        d.ram_globals.clear(); // DESC_JSON allows RAM_BASE..+0x100; drop that allowance
        let mut h = Harness::new(&img, d);
        let err = h.run_frame(100_000).unwrap_err();
        assert!(matches!(err, EmuError::Descriptor(_)));
    }

    #[test]
    fn test_run_frame_catches_stack_canary_corruption() {
        // Store directly to the bottom of the stack region. Harness::new uses a
        // 0x2000-byte stack at the top of RAM, so with ram_size 32768 the bottom
        // and canary sit at RAM_BASE + 0x8000 - 0x2000 = 0x20006000. Allowed by
        // the ACL, but the store clobbers the canary -> must fault.
        let stack_bottom = RAM_BASE + 0x8000 - 0x2000;
        let mut img = vec![0u8; 0x100];
        let code: [u16; 4] = [
            0x4901, // 0x80: ldr r1, [pc, #4]  ; base=align(0x84,4)=0x84, +4=0x88 -> &stack_bottom
            0x2055, // 0x82: movs r0, #0x55
            0x6008, // 0x84: str r0, [r1, #0]
            0x4770, // 0x86: bx lr
        ];
        for (i, hw) in code.iter().enumerate() {
            img[0x80 + i * 2..0x80 + i * 2 + 2].copy_from_slice(&hw.to_le_bytes());
        }
        img[0x88..0x8C].copy_from_slice(&stack_bottom.to_le_bytes());
        let d = load_descriptor(DESC_JSON).unwrap();
        let mut h = Harness::new(&img, d);
        let err = h.run_frame(100_000).unwrap_err();
        match err {
            EmuError::Descriptor(msg) => assert!(msg.contains("canary"), "unexpected: {msg}"),
            other => panic!("expected Descriptor error, got {other:?}"),
        }
    }

    #[test]
    fn test_run_frame_catches_stack_pointer_underflow() {
        // Repeated `sub sp, #508` with no store at all. The 0x2000-byte stack's
        // headroom above the region bottom is 0x2000 - 0x100 (initial sp at
        // ram_top - 0x100) = 0x1F00 = 7936 bytes. 16 subs = 8128 bytes exceeds it,
        // so sp dips below the region bottom and must fault on min_sp.
        let mut img = vec![0u8; 0x100];
        for i in 0..16 {
            img[0x80 + i * 2..0x82 + i * 2].copy_from_slice(&0xB0FFu16.to_le_bytes()); // sub sp, #508
        }
        img[0xA0..0xA2].copy_from_slice(&0x4770u16.to_le_bytes()); // bx lr
        let d = load_descriptor(DESC_JSON).unwrap();
        let mut h = Harness::new(&img, d);
        let err = h.run_frame(100_000).unwrap_err();
        match err {
            EmuError::Descriptor(msg) => assert!(msg.contains("stack pointer"), "unexpected: {msg}"),
            other => panic!("expected Descriptor error, got {other:?}"),
        }
    }

    #[test]
    fn test_unpack_block1_layout() {
        let mut buf = vec![0u8; 64 * 128 / 8];
        buf[0] = 0b1000_0010; // y=0: x=0 (MSB) and x=6
        buf[8] = 0x01;        // y=1 (second row, stride 8): x=7 (LSB)
        let f = unpack_block1(&buf, 64, 128);
        assert_eq!(f.pixels[0], 1);          // (0,0)
        assert_eq!(f.pixels[6], 1);          // (6,0)
        assert_eq!(f.pixels[64 + 7], 1);     // (7,1)
        assert_eq!(f.pixels[1], 0);          // (1,0)
    }
}
