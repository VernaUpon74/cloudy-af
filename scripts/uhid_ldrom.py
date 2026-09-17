#!/usr/bin/env python3
"""Virtual /dev/uhid double of the Nuvoton LDROM updater (validation route §5).

Serves VID 0416 / PID 5020 with a vendor-page 0xFF00 64-byte report
descriptor, so the real flasher (`flasher::flash_firmware_guarded`) connects
and runs its full protocol against a userspace fake:

- 0x35 ReadDataflash -> 2048 bytes (LE32 checksum + 2044 user bytes;
  boot flag user[9], PID "M041" at user 312, fw version 110 at user 256).
- 0xC3 WriteData(arg1=start, arg2=len) -> consume exactly arg2 payload
  bytes from subsequent OUT reports, no responses (like the real LDROM);
  when the stream completes the update is committed and the boot flag is
  cleared (the LDROM clears it after a successful update).
- 0x53 WriteDataflash -> consume the following 2048 payload bytes and apply
  them to the served user area.
- 0xB4 Restart -> reboot no-op: the device re-enumerates with the boot
  flag UNCHANGED (flag 1 boots LDROM, flag 0 boots APROM), so the
  post-flash poll sees data[4+9]==0 once the update cleared it.

Options:
  --nak-delay-ms N   sleep N ms between event reads (consumer-lag timing
                     margin; note uhid gives NO backpressure: the kernel
                     silently drops OUT reports past its 31-deep ring, so
                     host streams must stay below that per burst)
  --die-at-offset N  exit after N payload bytes of the 0xC3 stream
                     (simulates a mid-flash device death)
  --serial S         serial/uniq string (default A02015081302-uhid)

Stdlib only. Requires /dev/uhid (KERNEL=="uhid", MODE="0666" udev rule).
"""

import argparse
import os
import struct
import sys
import time

# enum uhid_event_type (linux/uhid.h)
UHID_DESTROY = 1
UHID_START = 2
UHID_STOP = 3
UHID_OPEN = 4
UHID_CLOSE = 5
UHID_OUTPUT = 6
UHID_CREATE2 = 11
UHID_INPUT2 = 12

# struct uhid_event: __u32 type + union. Largest member is uhid_create2_req.
# struct uhid_create2_req { __u16 rd_size; __u8 report_descriptor[256];
#   __u16 bus; __u16 vendor; __u16 product; __u32 version; __u32 country;
#   __u8 rd_data[4096]; }
# sizeof create2_req = 2+256+2+2+2+4+4+4096 = 4368; +4 (type) = 4372.
EVENT_SIZE = 4372

# Offsets within a flat uhid_event buffer (all little-endian, __packed__).
# struct uhid_event { __u32 type; union { struct uhid_create2_req create2; ... } u; }
# struct uhid_create2_req { __u8 name[128]; __u8 phys[64]; __u8 uniq[64];
#   __u16 rd_size; __u16 bus; __u32 vendor; __u32 product; __u32 version;
#   __u32 country; __u8 rd_data[HID_MAX_DESCRIPTOR_SIZE]; } __packed__
# sizeof = 128+64+64+2+2+4+4+4+4+4096 = 4368; +4 (type) = 4372.
EV_TYPE_OFF = 0
# uhid_create2_req fields (offsets relative to start of uhid_event buffer):
CREATE2_NAME_OFF = 4              # __u8 name[128]
CREATE2_PHYS_OFF = 4 + 128       # __u8 phys[64]
CREATE2_UNIQ_OFF = 4 + 128 + 64  # __u8 uniq[64]
CREATE2_RD_SIZE_OFF = 4 + 128 + 64 + 64   # __u16 rd_size  (offset 260)
CREATE2_BUS_OFF = CREATE2_RD_SIZE_OFF + 2 # __u16 bus       (offset 262)
CREATE2_VENDOR_OFF = CREATE2_BUS_OFF + 2  # __u32 vendor    (offset 264)
CREATE2_PRODUCT_OFF = CREATE2_VENDOR_OFF + 4  # __u32 product (offset 268)
CREATE2_VERSION_OFF = CREATE2_PRODUCT_OFF + 4  # __u32 version (offset 272)
CREATE2_COUNTRY_OFF = CREATE2_VERSION_OFF + 4  # __u32 country (offset 276)
CREATE2_RD_DATA_OFF = CREATE2_COUNTRY_OFF + 4  # __u8 rd_data[4096] (offset 280)
EVENT_SIZE = CREATE2_RD_DATA_OFF + 4096  # = 4372

# struct uhid_output_req { __u8 data[4096]; __u16 size; __u8 rtype; }
# This is a different struct used for UHID_OUTPUT events. Size = 4096+2+1 = 4099,
# but the kernel reads EVENT_SIZE bytes. We only care about data[0..size].
OUTPUT_DATA_OFF = 4
OUTPUT_SIZE_OFF = 4 + 4096

# struct uhid_input2_req { __u16 size; __u8 data[4096]; }
INPUT2_HDR = struct.Struct("<IH")

BUS_USB = 0x03
VID = 0x0416
PID = 0x5020

REPORT_SIZE = 64
DATAFLASH_SIZE = 2048

CMD_READ_DATAFLASH = 0x35
CMD_WRITE_DATAFLASH = 0x53
CMD_WRITE_DATA = 0xC3
CMD_RESTART = 0xB4

# Vendor page 0xFF00, one 64-byte input + one 64-byte output report, no
# report IDs (mirrors the Nuvoton updater; the flasher treats every returned
# byte as payload and prepends report-ID 0 on writes).
REPORT_DESCRIPTOR = bytes([
    0x06, 0x00, 0xFF,  # Usage Page (Vendor 0xFF00)
    0x09, 0x01,        # Usage (0x01)
    0xA1, 0x01,        # Collection (Application)
    0x15, 0x00,        #   Logical Minimum (0)
    0x26, 0xFF, 0x00,  #   Logical Maximum (255)
    0x75, 0x08,        #   Report Size (8)
    0x95, 0x40,        #   Report Count (64)
    0x09, 0x01,        #   Usage (0x01)
    0x81, 0x02,        #   Input (Data,Var,Abs)
    0x09, 0x01,        #   Usage (0x01)
    0x91, 0x02,        #   Output (Data,Var,Abs)
    0xC0,              # End Collection
])

RD_SIZE = len(REPORT_DESCRIPTOR)


def log(msg):
    print(f"[uhid-ldrom] {msg}", file=sys.stderr, flush=True)


def create2_event(name, phys, uniq):
    """struct uhid_event { type=UHID_CREATE2, u.create2 }.

    Matches linux/uhid.h struct uhid_create2_req (packed):
      __u8 name[128]; __u8 phys[64]; __u8 uniq[64];
      __u16 rd_size; __u16 bus; __u32 vendor; __u32 product;
      __u32 version; __u32 country;
      __u8 rd_data[HID_MAX_DESCRIPTOR_SIZE];
    The report descriptor is copied into rd_data (the kernel copies rd_size
    bytes from there into the device's descriptor). name/phys/uniq are what
    the real Nuvoton LDROM reports: iManufacturer="Nuvoton", the HID path,
    and the baked-in serial (A02015081302).
    """
    buf = bytearray(EVENT_SIZE)
    struct.pack_into("<I", buf, EV_TYPE_OFF, UHID_CREATE2)
    # name / phys / uniq (the real device's strings, null-padded).
    name_b = name.encode()[:127]
    buf[CREATE2_NAME_OFF : CREATE2_NAME_OFF + len(name_b)] = name_b
    phys_b = phys.encode()[:63]
    buf[CREATE2_PHYS_OFF : CREATE2_PHYS_OFF + len(phys_b)] = phys_b
    uniq_b = uniq.encode()[:63]
    buf[CREATE2_UNIQ_OFF : CREATE2_UNIQ_OFF + len(uniq_b)] = uniq_b
    # Descriptor + device identity.
    struct.pack_into("<H", buf, CREATE2_RD_SIZE_OFF, RD_SIZE)
    struct.pack_into("<H", buf, CREATE2_BUS_OFF, BUS_USB)
    struct.pack_into("<I", buf, CREATE2_VENDOR_OFF, VID)
    struct.pack_into("<I", buf, CREATE2_PRODUCT_OFF, PID)
    struct.pack_into("<I", buf, CREATE2_VERSION_OFF, 0x0110)  # fw ver BCD 1.10
    struct.pack_into("<I", buf, CREATE2_COUNTRY_OFF, 0)
    buf[CREATE2_RD_DATA_OFF : CREATE2_RD_DATA_OFF + RD_SIZE] = REPORT_DESCRIPTOR
    return bytes(buf)


def input2_event(payload):
    """struct uhid_event { type=UHID_INPUT2, u.input2 } — IN report to host."""
    return INPUT2_HDR.pack(UHID_INPUT2, len(payload)) + payload


class LdromDouble:
    def __init__(self, fd, nak_delay_ms, die_at_offset):
        self.fd = fd
        self.nak_delay = nak_delay_ms / 1000.0
        self.die_at_offset = die_at_offset
        self.user = bytearray(2044)
        self.user[9] = 1  # enumerate in LDROM mode
        self.user[312:316] = b"M041"
        struct.pack_into("<i", self.user, 256, 110)  # fw version 1.10
        self.stream_remaining = 0    # 0xC3 payload bytes still expected
        self.stream_received = 0
        self.df_remaining = 0        # 0x53 payload bytes still expected
        self.df_buf = bytearray()

    def dataflash(self):
        out = bytearray(DATAFLASH_SIZE)
        struct.pack_into("<I", out, 0, sum(self.user) & 0xFFFFFFFF)
        out[4:] = self.user
        return bytes(out)

    def send_dataflash(self):
        payload = self.dataflash()
        for i in range(0, len(payload), REPORT_SIZE):
            os.write(self.fd, input2_event(payload[i : i + REPORT_SIZE]))

    def handle_command(self, report):
        cmd = report[0]
        arg1, arg2 = struct.unpack_from("<ii", report, 2)
        if cmd == CMD_READ_DATAFLASH:
            self.send_dataflash()
        elif cmd == CMD_WRITE_DATA:
            self.stream_remaining = arg2
            self.stream_received = 0
            log(f"0xC3 WriteData(start={arg1:#x}, len={arg2})")
        elif cmd == CMD_WRITE_DATAFLASH:
            self.df_remaining = arg2
            self.df_buf = bytearray()
            log(f"0x53 WriteDataflash(len={arg2})")
        elif cmd == CMD_RESTART:
            # Reboot: the device re-enumerates with the boot flag unchanged
            # (flag 1 -> LDROM, flag 0 -> APROM). Nothing to do here.
            log(f"0xB4 Restart (boot flag stays {self.user[9]})")
        else:
            log(f"unknown command {cmd:#04x} — dropped")

    def handle_output(self, event):
        size = struct.unpack_from("<H", event, OUTPUT_SIZE_OFF)[0]
        report = event[OUTPUT_DATA_OFF : OUTPUT_DATA_OFF + size]
        # Unnumbered reports still arrive with the leading report-ID 0 byte
        # that hidraw requires on writes; strip it.
        if size == REPORT_SIZE + 1 and report[0] == 0:
            report = report[1:]
        if self.stream_remaining > 0:
            take = min(len(report), self.stream_remaining)
            self.stream_remaining -= take
            self.stream_received += take
            if self.stream_received % 4096 == 0 or self.stream_remaining == 0:
                log(f"stream {self.stream_received} bytes "
                    f"({self.stream_remaining} left)")
            if (self.die_at_offset is not None
                    and self.stream_received >= self.die_at_offset):
                log(f"dying at stream offset {self.stream_received}")
                sys.exit(0)
            if self.stream_remaining == 0:
                # The LDROM commits the update and clears the boot flag;
                # the next reboot (0xB4) then lands in APROM.
                self.user[9] = 0
                log("0xC3 stream complete — update committed, boot flag cleared")
        elif self.df_remaining > 0:
            take = min(len(report), self.df_remaining)
            self.df_buf.extend(report[:take])
            self.df_remaining -= take
            if self.df_remaining == 0:
                self.user[:] = self.df_buf[4 : 4 + 2044]
                log(f"0x53 applied: boot flag now {self.user[9]}")
        else:
            self.handle_command(report)

    def run(self):
        log("serving (VID 0416 PID 5020, LDROM mode)")
        while True:
            if self.nak_delay:
                time.sleep(self.nak_delay)
            event = os.read(self.fd, EVENT_SIZE)
            etype = struct.unpack_from("<I", event, 0)[0]
            if etype == UHID_OUTPUT:
                self.handle_output(event)
            elif etype in (UHID_START, UHID_OPEN, UHID_CLOSE, UHID_STOP,
                           UHID_DESTROY):
                pass
            # GET_REPORT/SET_REPORT are not used by the flasher; leave
            # unanswered like the real (interrupt-only) updater.


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--nak-delay-ms", type=int, default=0)
    ap.add_argument("--die-at-offset", type=int, default=None)
    ap.add_argument("--serial", default="A02015081302-uhid")
    args = ap.parse_args()

    fd = os.open("/dev/uhid", os.O_RDWR)
    try:
        os.write(fd, create2_event(
            name="HID Transfer", phys="uhid-ldrom-double", uniq=args.serial))
        LdromDouble(fd, args.nak_delay_ms, args.die_at_offset).run()
    finally:
        os.close(fd)


if __name__ == "__main__":
    main()
