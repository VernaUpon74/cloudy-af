#!/usr/bin/env python3
"""Verify i18n completeness: every data-lang/data-lang-title key used in the
HTML pages must exist in en.json, and all 17 locale files must have the same
key set. Exit 1 (and print the offenders) on any gap. Run before committing
translation work; the rebuild watcher also runs it as a gate."""
import glob
import json
import re
import sys

root = __file__.rsplit('/scripts/', 1)[0]
en = json.load(open(f'{root}/public/i18n/en.json'))
used = set()
for page in ('index.html', 'firmware.html', 'monitor.html'):
    html = open(f'{root}/{page}').read()
    used |= set(re.findall(r'data-lang(?:-title)?="([^"]+)"', html))

missing = sorted(k for k in used if k not in en)
locales = {}
for f in sorted(glob.glob(f'{root}/public/i18n/*.json')):
    d = json.load(open(f))
    locales[f.rsplit('/', 1)[-1]] = d

parity = {len(d) for d in locales.values()}
bad = 0
if missing:
    bad = 1
    print('MISSING from en.json:')
    for k in missing:
        print(' ', k)
if len(parity) != 1:
    bad = 1
    print('KEY-COUNT PARITY BROKEN:')
    for name, d in locales.items():
        extra = sorted(set(d) - set(locales['en.json']))
        gone = sorted(set(locales['en.json']) - set(d))
        if extra or gone:
            print(f' {name}: +{extra} -{gone}')
if not bad:
    n = len(locales['en.json'])
    print(f'i18n OK: {len(locales)} locales, {n} keys each, all {len(used)} used keys present')
sys.exit(bad)
