#!/usr/bin/env python3
"""Inventory local audio plugin bundles and suggest name-based cross-format matches.

AAX bundles are READ ONLY: this tool does not host, load, decode or bridge AAX.
A name match does not imply preset/state interchangeability.
"""
from __future__ import annotations
import argparse
from collections import defaultdict
from pathlib import Path
import json
import re
import sys

FORMATS = {'vst3': 'VST3', 'vst': 'VST2', 'component': 'AU', 'aaxplugin': 'AAX', 'clap': 'CLAP'}
SYSTEM_PATHS = [
    '/Library/Audio/Plug-Ins/VST3',
    '/Library/Audio/Plug-Ins/VST',
    '/Library/Audio/Plug-Ins/Components',
    '/Library/Audio/Plug-Ins/CLAP',
    '/Library/Application Support/Avid/Audio/Plug-Ins',
]
USER_PATHS = ['~/Library/Audio/Plug-Ins/VST3', '~/Library/Audio/Plug-Ins/VST',
              '~/Library/Audio/Plug-Ins/Components', '~/Library/Audio/Plug-Ins/CLAP']


def key_for(name: str) -> str:
    return re.sub(r'[^a-z0-9]+', '', name.casefold())


def scan(dirs: list[Path]) -> dict:
    plugins = []
    seen = set()
    for folder in dirs:
        folder = folder.expanduser()
        if not folder.is_dir():
            continue
        try:
            bundles = sorted(folder.iterdir())
        except OSError:
            continue
        for entry in bundles:
            suffix = entry.suffix.lower().lstrip('.')
            if suffix not in FORMATS:
                continue
            # No recursive traversal into plugin executable bundles.
            fmt = FORMATS[suffix]
            item = (str(entry), fmt)
            if item in seen:
                continue
            seen.add(item)
            plugins.append({'name': entry.stem, 'format': fmt, 'path': str(entry),
                            'status': 'found, not load-tested'})
    by_name = defaultdict(list)
    for item in plugins:
        by_name[key_for(item['name'])].append(item)
    alternatives = []
    for normalized, group in sorted(by_name.items()):
        formats = {item['format'] for item in group}
        if len(formats) > 1:
            alternatives.append({'name': group[0]['name'], 'formats': sorted(formats),
                                 'paths': [item['path'] for item in group],
                                 'state_transfer': 'unverified'})
    return {
        'note': 'Directory inventory only; plugin compatibility is NOT confirmed',
        'aax': 'AAX bundles are never loaded and native AAX hosting is unsupported',
        'plugins': sorted(plugins, key=lambda x:(x['name'].casefold(), x['format'])),
        'possible_cross_format_matches': alternatives,
    }


def main(argv=None) -> int:
    p = argparse.ArgumentParser()
    p.add_argument('--dir', dest='dirs', type=Path, action='append', help='Override paths (repeatable)')
    p.add_argument('--output', type=Path, help='Write JSON report here; stdout otherwise')
    args = p.parse_args(argv)
    dirs = args.dirs or [Path(x) for x in SYSTEM_PATHS+USER_PATHS]
    result = scan(dirs)
    raw = json.dumps(result, indent=2, ensure_ascii=False)+'\n'
    if args.output:
        args.output.expanduser().write_text(raw, encoding='utf-8')
        print(f"Scanned {len(result['plugins'])} bundles; report: {args.output}")
    else:
        sys.stdout.write(raw)
    return 0

if __name__ == '__main__':
    raise SystemExit(main())
