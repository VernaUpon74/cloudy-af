use super::EmuError;

pub const FLASH_BASE: u32 = 0x0000_0000;
pub const RAM_BASE: u32 = 0x2000_0000;

#[derive(Debug, Clone, PartialEq)]
pub struct WriteRecord {
    pub addr: u32,
    pub value: u32,
    pub size: u8,
}

enum Region {
    Flash(usize),
    Ram(usize),
    Peripheral,
}

pub struct Bus {
    flash: Vec<u8>,
    ram: Vec<u8>,
    allow: Vec<std::ops::Range<u32>>,
    stubs: std::collections::HashMap<u32, u32>,
    pub write_log: Vec<WriteRecord>,      // all stores that landed in RAM
    pub acl_violations: Vec<WriteRecord>, // stores outside allow-list or into flash
    pub dropped_writes: Vec<WriteRecord>, // stores to peripheral space
}

impl Bus {
    pub fn new(flash: Vec<u8>, ram_size: usize) -> Self {
        Self {
            flash,
            ram: vec![0u8; ram_size],
            allow: Vec::new(),
            stubs: std::collections::HashMap::new(),
            write_log: Vec::new(),
            acl_violations: Vec::new(),
            dropped_writes: Vec::new(),
        }
    }

    pub fn allow_region(&mut self, range: std::ops::Range<u32>) {
        self.allow.push(range);
    }

    pub fn set_stub(&mut self, addr: u32, value: u32) {
        self.stubs.insert(addr, value);
    }

    /// Resolve the region containing the whole access `addr .. addr + size`.
    /// Any byte outside the region makes the entire access `Unmapped` — never
    /// let a multi-byte access index past the end of a backing vec.
    fn resolve(&self, addr: u32, size: u8) -> Result<Region, EmuError> {
        let size = size as u64;
        // An explicit stub defines the address: reads return the stubbed
        // value, writes are dropped (e.g. a config byte in dataflash that
        // the memory map does not otherwise model).
        if self.stubs.contains_key(&addr) {
            return Ok(Region::Peripheral);
        }
        if addr >= FLASH_BASE && (addr - FLASH_BASE) as u64 + size <= self.flash.len() as u64 {
            Ok(Region::Flash((addr - FLASH_BASE) as usize))
        } else if addr >= RAM_BASE && (addr - RAM_BASE) as u64 + size <= self.ram.len() as u64 {
            Ok(Region::Ram((addr - RAM_BASE) as usize))
        } else if (0x4000_0000..0x6000_0000).contains(&addr) || addr >= 0xE000_0000 {
            Ok(Region::Peripheral)
        } else {
            Err(EmuError::Unmapped { addr })
        }
    }

    fn check_align(&self, addr: u32, size: u8) -> Result<(), EmuError> {
        if addr % size as u32 != 0 {
            Err(EmuError::Unaligned { addr, size })
        } else {
            Ok(())
        }
    }

    fn read(&self, addr: u32, size: u8) -> Result<u32, EmuError> {
        self.check_align(addr, size)?;
        let size_us = size as usize;
        match self.resolve(addr, size)? {
            Region::Flash(off) => Ok(self.read_le(&self.flash, off, size_us)),
            Region::Ram(off) => Ok(self.read_le(&self.ram, off, size_us)),
            Region::Peripheral => Ok(self.stubs.get(&addr).copied().unwrap_or(0)),
        }
    }

    fn read_le(&self, mem: &[u8], off: usize, size: usize) -> u32 {
        let mut v: u32 = 0;
        for i in 0..size {
            v |= (mem[off + i] as u32) << (8 * i);
        }
        v
    }

    pub fn read_u8(&self, addr: u32) -> Result<u8, EmuError> {
        Ok(self.read(addr, 1)? as u8)
    }

    pub fn read_u16(&self, addr: u32) -> Result<u16, EmuError> {
        Ok(self.read(addr, 2)? as u16)
    }

    pub fn read_u32(&self, addr: u32) -> Result<u32, EmuError> {
        self.read(addr, 4)
    }

    /// Whole-range ACL: the access `addr .. addr + size` must lie within ONE
    /// allow region (matching how regions are carved) to avoid a violation.
    fn allowed(&self, addr: u32, size: u8) -> bool {
        let (start, end) = (addr as u64, addr as u64 + size as u64);
        self.allow
            .iter()
            .any(|r| r.start as u64 <= start && end <= r.end as u64)
    }

    fn write(&mut self, addr: u32, value: u32, size: u8) -> Result<(), EmuError> {
        self.check_align(addr, size)?;
        let record = WriteRecord { addr, value, size };
        match self.resolve(addr, size)? {
            Region::Flash(_) => {
                // CPU stores cannot write flash on real hardware — runaway code.
                self.acl_violations.push(record);
            }
            Region::Ram(off) => {
                for i in 0..size as usize {
                    self.ram[off + i] = (value >> (8 * i)) as u8;
                }
                if !self.allowed(addr, size) {
                    self.acl_violations.push(record.clone());
                }
                self.write_log.push(record);
            }
            Region::Peripheral => {
                self.dropped_writes.push(record);
            }
        }
        Ok(())
    }

    pub fn write_u8(&mut self, addr: u32, value: u8) -> Result<(), EmuError> {
        self.write(addr, value as u32, 1)
    }

    pub fn write_u16(&mut self, addr: u32, value: u16) -> Result<(), EmuError> {
        self.write(addr, value as u32, 2)
    }

    pub fn write_u32(&mut self, addr: u32, value: u32) -> Result<(), EmuError> {
        self.write(addr, value, 4)
    }
}

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

    #[test]
    fn test_read_straddling_ram_end_is_unmapped_not_panic() {
        // RAM size not a multiple of 4 so an aligned u32 read can straddle the end.
        let b = Bus::new(vec![0u8; 0x1000], 0x1002);
        assert!(matches!(
            b.read_u32(RAM_BASE + 0x1000), // covers 0x1000..0x1004, RAM ends at 0x1002
            Err(EmuError::Unmapped { .. })
        ));
    }

    #[test]
    fn test_write_straddling_flash_end_is_unmapped_not_panic() {
        // Flash size not a multiple of 4 so an aligned u32 write can straddle the end.
        let mut b = Bus::new(vec![0u8; 0x1002], 0x1000);
        assert!(matches!(
            b.write_u32(FLASH_BASE + 0x1000, 0xFFFF_FFFF), // covers 0x1000..0x1004
            Err(EmuError::Unmapped { .. })
        ));
        assert!(b.acl_violations.is_empty()); // error, not a recorded violation
    }

    #[test]
    fn test_write_straddling_allow_region_end_is_violation_but_applied() {
        let mut b = bus();
        b.allow_region(RAM_BASE + 0x400..RAM_BASE + 0x402); // end not 4-aligned
        b.write_u16(RAM_BASE + 0x400, 0xBEEF).unwrap(); // 0x400..0x402 fits the region
        assert!(b.acl_violations.is_empty());
        b.write_u32(RAM_BASE + 0x400, 0xAABB_CCDD).unwrap(); // 0x400..0x404 straddles region end
        assert_eq!(b.read_u32(RAM_BASE + 0x400).unwrap(), 0xAABB_CCDD); // applied
        assert_eq!(b.acl_violations.len(), 1);
        assert_eq!(b.acl_violations[0].addr, RAM_BASE + 0x400);
        assert_eq!(b.write_log.len(), 2);
    }
}
