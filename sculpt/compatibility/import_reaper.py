#!/usr/bin/env python3
"""Experimental REAPER (.rpp) -> native SoundCraft (.scraft) via SoundCraft's own CLI.

This importer retains audio-track grouping, clip placement, source offset and
region duration. Region extraction is offline to WAV through ffmpeg. It does NOT
claim to preserve plugins, MIDI, automation, routing, take comping or tempo maps.

Usage:
 python3 sculpt/compatibility/import_reaper.py Song.rpp --output ./SculptImport
 cargo run --release -p soundcraft-cli -- script ./SculptImport/import.scscript --save ./SculptImport/Song.scraft
"""
from __future__ import annotations
import argparse
from dataclasses import dataclass, field
import json
import math
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys

MAX_ITEMS = 4096
MAX_DURATION = 8 * 60 * 60


def tokens(line: str) -> list[str]:
    try:
        return shlex.split(line, posix=True)
    except ValueError:
        return []


def number(v: str | None, default: float = 0.0) -> float:
    try:
        n = float(v)
        return n if math.isfinite(n) else default
    except (ValueError, TypeError):
        return default


@dataclass
class Item:
    position: float = 0.0
    length: float = 0.0
    offset: float = 0.0
    file: str | None = None
    rate: float = 1.0
    is_midi: bool = False
    fades: bool = False


@dataclass
class Track:
    name: str
    volume: float = 1.0
    pan: float = 0.0
    mute: bool = False
    solo: bool = False
    items: list[Item] = field(default_factory=list)
    plugin_chain: bool = False
    envelopes: bool = False


@dataclass
class Project:
    name: str
    rate: int = 48000
    tempo: float = 120.0
    tracks: list[Track] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)


def parse_rpp(path: Path) -> Project:
    project = Project(name=path.stem)
    stack: list[str] = []
    track: Track | None = None
    item: Item | None = None
    for n, raw in enumerate(path.read_text(encoding='utf-8-sig', errors='replace').splitlines(), start=1):
        s = raw.strip()
        if not s:
            continue
        if s == '>':
            if not stack:
                project.warnings.append(f'line {n}: unexpected section closing')
                continue
            close = stack.pop()
            if close == 'ITEM' and item is not None and track is not None:
                track.items.append(item)
                item = None
            if close == 'TRACK' and track is not None:
                project.tracks.append(track)
                track = None
            continue
        if s.startswith('<'):
            parts = tokens(s[1:])
            kind = parts[0] if parts else 'UNKNOWN'
            if kind == 'TRACK' and track is None:
                track = Track(f'Track {len(project.tracks) + 1}')
            elif kind == 'ITEM' and track is not None and item is None:
                item = Item()
            elif kind == 'SOURCE' and item is not None and len(parts) > 1:
                item.is_midi |= parts[1] == 'MIDI'
            elif kind == 'FXCHAIN' and track is not None:
                track.plugin_chain = True
            elif kind.startswith('PARMENV') and track is not None:
                track.envelopes = True
            stack.append(kind)
            continue
        parts = tokens(s)
        if not parts:
            continue
        key, vals = parts[0], parts[1:]
        if not vals:
            continue
        current = stack[-1] if stack else None
        if track is None and key == 'TEMPO':
            project.tempo = number(vals[0], 120.0)
        elif track is None and key == 'SAMPLERATE':
            sample_rate = round(number(vals[0], 48000))
            if 8000 <= sample_rate <= 768000:
                project.rate = sample_rate
        elif track is not None and item is None and current == 'TRACK':
            if key == 'NAME':
                track.name = vals[0]
            elif key == 'VOLPAN':
                track.volume = max(0.0, number(vals[0], 1.0))
                track.pan = max(-1.0, min(1.0, number(vals[1] if len(vals)>1 else None)))
            elif key == 'MUTESOLO':
                track.mute = vals[0] == '1'
                track.solo = len(vals)>1 and vals[1] == '1'
        elif item is not None:
            if key == 'POSITION' and current == 'ITEM':
                item.position = number(vals[0])
            elif key == 'LENGTH' and current == 'ITEM':
                item.length = number(vals[0])
            elif key == 'SOFFS' and current == 'ITEM':
                item.offset = number(vals[0])
            elif key == 'PLAYRATE' and current == 'ITEM':
                item.rate = number(vals[0], 1.0)
            elif key == 'FILE' and 'SOURCE' in stack:
                item.file = vals[0]
            elif key in {'FADEIN','FADEOUT'} and current == 'ITEM':
                item.fades = True
    if len(project.tracks) > 2048:
        raise ValueError('too many tracks')
    if sum(len(t.items) for t in project.tracks) > MAX_ITEMS:
        raise ValueError('too many items')
    if not project.tracks:
        project.warnings.append('No audio tracks found')
    return project


def unique_names(tracks: list[Track]) -> list[str]:
    used: set[str] = set()
    result = []
    for track in tracks:
        root = (track.name.strip() or 'Audio')[:50]
        name = root
        i = 2
        while name.casefold() in used:
            name = f'{root} ({i})'
            i += 1
        used.add(name.casefold())
        result.append(name)
    return result


def command(cmd: str, **params) -> str:
    return cmd + ' ' + json.dumps(params, ensure_ascii=False, separators=(',', ':'), allow_nan=False)


def generate(project: Project, rpp_path: Path, media_dir: Path, render: bool = True) -> tuple[list[str], dict]:
    lines = [
        '# Native SoundCraft command script generated by Sculpt DAW importer',
        '# Do not confuse with native RPP reader: destructive audio-region preprocessing is used.',
        command('session.new', name=project.name, sample_rate=project.rate),
    ]
    report = {'origin': str(rpp_path), 'format': 'REAPER .rpp', 'target': 'SoundCraft .scraft',
              'track_count': len(project.tracks), 'imported_items': 0, 'skipped_items': 0,
              'limitations': [
                  'Plugin chains, plug-in state and AAX/VST/AU mapping are not imported',
                  'Routing, sends, playlists, envelopes, MIDI and time signatures are not imported',
                  'Source regions are pre-rendered to individual WAV files; non-destructive originals are retained',
                  'Complex comping, fades, item stretch and tempo/time changes are not reproduced',
              ], 'warnings': list(project.warnings)}
    names = unique_names(project.tracks)
    if render and not shutil.which('ffmpeg'):
        raise RuntimeError('ffmpeg not installed. Install ffmpeg (e.g. brew install ffmpeg) to import RPP audio regions.')
    for idx, (track, name) in enumerate(zip(project.tracks, names), start=1):
        lines.append(command('track.new', name=name, kind='audio', format='stereo'))
        db = max(-144.0, min(12.0, 20 * math.log10(max(track.volume, 1e-7))))
        lines.append(command('mix.volume', tracks=[name], db=round(db, 5)))
        lines.append(command('mix.pan', tracks=[name], pan=round(track.pan, 6)))
        if track.mute:
            lines.append(command('mix.mute', tracks=[name], value=True))
        if track.solo:
            lines.append(command('mix.solo', tracks=[name], value=True, exclusive=False))
        if track.plugin_chain:
            report['warnings'].append(f'{name}: FXCHAIN omitted')
        if track.envelopes:
            report['warnings'].append(f'{name}: envelopes omitted')
        for j, item in enumerate(track.items, start=1):
            label = f'{name} clip {j}'
            source = (Path(item.file).expanduser() if item.file else None)
            if source is not None and not source.is_absolute():
                source = (rpp_path.parent / source).resolve()
            if item.is_midi or source is None:
                report['warnings'].append(f'{label}: no supported audio media source')
                report['skipped_items'] += 1
                continue
            if not source.is_file():
                report['warnings'].append(f'{label}: missing {source}')
                report['skipped_items'] += 1
                continue
            if (item.position < 0 or item.offset < 0 or item.length <= 0 or item.length > MAX_DURATION or
                item.rate != 1.0):
                report['warnings'].append(f'{label}: unsupported position/offset/length/playrate; not imported')
                report['skipped_items'] += 1
                continue
            if item.fades:
                report['warnings'].append(f'{label}: fades not reconstructed')
            wavefile = media_dir / f't{idx:04d}_clip{j:04d}.wav'
            if render:
                # Decoder pre-roll and trim occur after seeking to avoid imprecise compressed-media seeks.
                media_dir.mkdir(parents=True, exist_ok=True)
                args = ['ffmpeg', '-hide_banner', '-loglevel', 'error', '-nostdin', '-y',
                        '-i', str(source), '-ss', str(item.offset), '-t', str(item.length),
                        '-vn', '-c:a', 'pcm_f32le', str(wavefile)]
                proc = subprocess.run(args, capture_output=True, text=True, timeout=180)
                if proc.returncode != 0 or not wavefile.is_file() or wavefile.stat().st_size < 64:
                    report['warnings'].append(f'{label}: ffmpeg failed: {proc.stderr[:250]}')
                    report['skipped_items'] += 1
                    continue
            lines.append(command('file.import_audio', path=str(wavefile), track=name, at={'seconds': item.position}, new_tracks=False))
            report['imported_items'] += 1
    return lines, report


def cli(argv=None) -> int:
    p = argparse.ArgumentParser(description='Experimental REAPER to native SoundCraft session import')
    p.add_argument('rpp', type=Path)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--plan-only', action='store_true', help='Generate command plan without decoding media')
    p.add_argument('--force', action='store_true', help='Replace prior import outputs')
    a = p.parse_args(argv)
    rpp = a.rpp.expanduser().resolve()
    out = a.output.expanduser().resolve()
    if not rpp.is_file() or rpp.suffix.casefold() != '.rpp':
        p.error('a readable .rpp is required')
    if out.exists() and any(out.iterdir()) and not a.force:
        p.error('output exists and is nonempty; choose a fresh directory or pass --force')
    out.mkdir(parents=True, exist_ok=True)
    project = parse_rpp(rpp)
    lines, report = generate(project, rpp, out / 'Audio Extracted', render=not a.plan_only)
    (out / 'import.scscript').write_text('\n'.join(lines) + '\n', encoding='utf-8')
    (out / 'compatibility-report.json').write_text(json.dumps(report, indent=2, ensure_ascii=False) + '\n', encoding='utf-8')
    print(f"Generated {out / 'import.scscript'}")
    print(f"Imported {report['imported_items']} items; skipped {report['skipped_items']}; warnings {len(report['warnings'])}")
    if a.plan_only:
        print('PLAN ONLY: decoded WAV region media does not exist yet. Do not execute the script.')
    elif report['imported_items'] == 0:
        print('No importable audio regions. Inspect compatibility-report.json', file=sys.stderr)
        return 2
    return 0

if __name__ == '__main__':
    raise SystemExit(cli())
