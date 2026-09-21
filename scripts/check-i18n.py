#!/usr/bin/env python3
"""Verify i18n completeness: every data-lang/data-lang-title key used in any
shipped HTML page must exist in en.json, and every locale file must have the
exact same key set as en.json (not just the same key count). Exit 1 (and print
the offenders) on any gap. Run before committing translation work; the rebuild
watcher also runs it as a gate."""
import glob
import json
import re
import sys

root = __file__.rsplit('/scripts/', 1)[0]
en = json.load(open(f'{root}/public/i18n/en.json'))
used = set()
for page in sorted(glob.glob(f'{root}/*.html')):
    html = open(page).read()
    used |= set(re.findall(r'data-lang(?:-title)?="([^"]+)"', html))

missing = sorted(k for k in used if k not in en)
locales = {}
for f in sorted(glob.glob(f'{root}/public/i18n/*.json')):
    d = json.load(open(f))
    locales[f.rsplit('/', 1)[-1]] = d

bad = 0
if missing:
    bad = 1
    print('MISSING from en.json:')
    for k in missing:
        print(' ', k)
for name, d in locales.items():
    gone = sorted(set(en) - set(d))
    extra = sorted(set(d) - set(en))
    if gone or extra:
        bad = 1
        print(f'KEY PARITY BROKEN: {name}: missing={gone} extra={extra}')
if not bad:
    n = len(en)
    print(f'i18n OK: {len(locales)} locales, {n} keys each, all {len(used)} used keys present')
sys.exit(bad)
