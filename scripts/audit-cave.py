#!/usr/bin/env python3
"""Static hook/cave audit (validation route §4).

Usage: python3 scripts/audit-cave.py <effect.bin> <mods.json> <stock.bin>

Audits an emitted Thumb-1 animation code cave against its modification
metadata (produced by `cargo run --bin anim_patch_dump`):
diff confinement, hook detour target, instruction whitelist, store
discipline, branch bounds, termination, config byte, erased cave tail.
Exit 0 = PASS. Drafted by local ollama, reviewed and corrected by hand.
"""

import json
import re
import sys

from capstone import CS_ARCH_ARM, CS_MODE_LITTLE_ENDIAN, CS_MODE_THUMB, Cs

ALLOWED = {
    "tst", "beq", "bne", "bge", "b", "ldr", "ldrb", "str", "strb",
    "movs", "adds", "subs", "cmp", "lsrs", "lsls", "ands", "eors", "nop",
}
STORE_FB = re.compile(r"^r[0-9]+, \[r[0-9]+, r[0-9]+\]$")  # strb framebuffer byte
STORE_PHASE = re.compile(r"^r[0-9]+, \[r[0-9]+(, #0)?\]$")  # str phase global

failures = []


def check(name, ok, detail=""):
    print(f"{'ok' if ok else 'FAIL'} {name}: {detail}")
    if not ok:
        failures.append(name)


def main():
    if len(sys.argv) != 4:
        sys.exit("usage: audit-cave.py <effect.bin> <mods.json> <stock.bin>")
    effect = open(sys.argv[1], "rb").read()
    mods = json.load(open(sys.argv[2]))
    stock = open(sys.argv[3], "rb").read()

    hook_site = mods["hook_site"]
    hook_resume = mods["hook_resume"]
    cave_start = mods["cave_start"]
    cave_size = mods["cave_size"]
    body_len = mods["cave_body_len"]
    config_offset = mods["config_offset"]

    # 1. confinement: diff vs stock (missing stock bytes count as 0xFF)
    diff = {
        i
        for i in range(len(effect))
        if (stock[i] if i < len(stock) else 0xFF) != effect[i]
    }
    extra = diff - set(mods["offsets"])
    # Declared-but-clean offsets (patched byte coincidentally equals the
    # stock/0xFF byte) are expected and not a failure.
    check("confinement", not extra,
          f"{len(diff)} diff offsets, all declared" if not extra
          else f"undeclared writes: {sorted(extra)[:5]}")

    md = Cs(CS_ARCH_ARM, CS_MODE_THUMB | CS_MODE_LITTLE_ENDIAN)
    md.detail = True

    def asm(i):
        return (i.mnemonic + " " + i.op_str).strip()

    # 2. hook detour: single 32-bit b.w at hook_site targeting cave_start
    hook = list(md.disasm(effect[hook_site:hook_site + 4], hook_site))
    ok = (len(hook) == 1 and hook[0].mnemonic.removesuffix(".w") == "b"
          and hook[0].size == 4 and hook[0].operands[-1].imm == cave_start)
    check("hook", ok, asm(hook[0]) if hook else "no instruction")

    # Code region: contiguous instructions from cave_start up to and
    # including the terminating b.w hook_resume. What follows is an
    # optional 0x46C0 (mov r8, r8) alignment nop, then a 32-bit literal
    # pool occupying the rest of the declared body.
    body = effect[cave_start:cave_start + body_len]
    insns = []
    off = 0
    terminator = None
    while off < body_len and terminator is None:
        one = list(md.disasm(body[off:], cave_start + off, count=1))
        if not one:
            break
        insns.append(one[0])
        off += one[0].size
        i = one[0]
        if (i.mnemonic.removesuffix(".w") == "b" and i.size == 4
                and i.operands[-1].imm == hook_resume):
            terminator = i
    code_end = off
    pool_start = code_end
    if (terminator is not None and code_end < body_len
            and body[code_end:code_end + 2] == b"\xc0\x46"):
        pool_start = code_end + 2  # alignment nop

    # 3. decode + whitelist over the code region
    check("decode", terminator is not None,
          f"{code_end}/{body_len} bytes code, pool {body_len - pool_start} B"
          if terminator is not None else f"no terminator, stopped at +{off:#x}")
    bad = [asm(i) for i in insns if i.mnemonic.removesuffix(".w") not in ALLOWED]
    check("whitelist", not bad, f"{len(insns)} instructions" if not bad else f"forbidden: {bad[:3]}")

    # 4. store discipline
    bad_st = [asm(i) for i in insns
              if i.mnemonic in ("str", "strb")
              and not (STORE_FB.match(i.op_str) or STORE_PHASE.match(i.op_str))]
    check("stores", not bad_st,
          "only strb [rN, rN] / str [rN]" if not bad_st else f"bad: {bad_st[:3]}")

    # 5. branch bounds: inside code region or to hook_resume
    bad_br = []
    for i in insns:
        if i.mnemonic.removesuffix(".w") in ("beq", "bne", "bge", "b"):
            target = i.operands[-1].imm
            if not (cave_start <= target < cave_start + code_end or target == hook_resume):
                bad_br.append(f"{asm(i)} -> {target:#x}")
    check("branches", not bad_br, "all targets in code or hook_resume"
          if not bad_br else f"out of bounds: {bad_br[:3]}")

    # 6. literal pool: whole 32-bit words, every pc-relative ldr lands in it
    from capstone.arm import ARM_OP_MEM, ARM_REG_PC
    pool_ok = (body_len - pool_start) % 4 == 0
    lit_ok = True
    if terminator is not None:
        for i in insns:
            if i.mnemonic != "ldr":
                continue
            for op in i.operands:
                if op.type == ARM_OP_MEM and op.mem.base == ARM_REG_PC:
                    la = ((i.address + 4) & ~3) + op.mem.disp
                    rel = la - cave_start
                    if not (pool_start <= rel < body_len and rel % 4 == 0):
                        lit_ok = False
                        print(f"  literal out of pool: {asm(i)} @ {i.address:#x} -> {la:#x}")
    check("literals", pool_ok and lit_ok,
          f"pool [{pool_start:#x},{body_len:#x}) body-relative"
          if pool_ok and lit_ok else "pool misaligned or literal outside")

    # 7. config byte
    ok = (config_offset < len(effect) and effect[config_offset] in (2, 3, 4)
          and (config_offset >= len(stock) or stock[config_offset] == 0xFF))
    check("config", ok, f"value={effect[config_offset] if config_offset < len(effect) else 'OOB'}")

    # 8. erased cave tail
    dirty = [hex(i) for i in range(cave_start + body_len, cave_start + cave_size)
             if i < len(effect) and effect[i] != 0xFF]
    check("erased-tail", not dirty, "tail 0xFF" if not dirty else f"dirty: {dirty[:3]}")

    if failures:
        sys.exit(1)
    print(f"PASS {sys.argv[1]}")


if __name__ == "__main__":
    main()
