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

#[derive(Debug, Clone)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>, // 0|1, row-major
}

pub struct Harness {
    pub cpu: Cpu,
    pub bus: Bus,
    pub desc: Descriptor,
}

impl Harness {
    pub fn new(image: &[u8], desc: Descriptor) -> Self {
        let ram_size = if desc.ram_size == 0 { 0x8000 } else { desc.ram_size };
        let mut bus = Bus::new(image.to_vec(), ram_size);
        bus.allow_region(desc.display_buffer.range.start..desc.display_buffer.range.end);
        for g in &desc.ram_globals {
            bus.allow_region(g.start..g.end);
        }
        let ram_top = RAM_BASE + ram_size as u32;
        bus.allow_region(ram_top - 0x1000..ram_top); // stack
        let mut cpu = Cpu::new();
        cpu.sp = ram_top - 16; // 8-aligned
        bus.write_u32(cpu.sp - 4, 0xDEAD_C0DE)
            .expect("canary write lands inside the stack allow region");
        cpu.lr = RETURN_SENTINEL | 1;
        cpu.r[..4].copy_from_slice(&desc.args);
        Self { cpu, bus, desc }
    }

    /// Run the render entry point once and unpack the display buffer.
    /// Any ACL violation during the run is the anti-brick signal: report it
    /// as a Descriptor error (with a cpu dump) so the test fails loudly.
    pub fn run_frame(&mut self, budget: u64) -> Result<Frame, EmuError> {
        self.cpu.pc = self.desc.render_entry & !1;
        self.cpu.lr = RETURN_SENTINEL | 1;
        self.cpu.run_until(&mut self.bus, RETURN_SENTINEL, budget)?;
        if !self.bus.acl_violations.is_empty() {
            return Err(EmuError::Descriptor(format!(
                "ACL violations: {:?}\n{}",
                self.bus.acl_violations,
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

/// Block1 vertical packing: byte `x + (y/8)*width`, bit `y%8`
/// (LSB = topmost of the 8-pixel column group), set bit = pixel on.
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
        assert_eq!(frame.pixels[0], 1);       // x=0,y=0 set by the 0xFF byte
        assert_eq!(frame.pixels[7 * 64], 1);  // x=0,y=7: bit 7 of the same byte
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
