#!/usr/bin/env python3
"""macOS mixed-corpus archiver comparison; synthetic data only, no downloads.

Run: python3 bench/competitors.py --fz /absolute/path/to/fz --output results.json
Bandizip uses the vendor-supported executable/commands, including GUI startup.
No competitor binaries or input corpus are redistributed.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import random
import re
import shutil
import signal
import statistics
import subprocess
import tempfile
import time
import zipfile

MIB = 1024 * 1024
BANDI = Path('/Applications/Bandizip365.app/Contents/MacOS/Bandizip365')


def corpus_at(root):
    root.mkdir(parents=True)
    rng = random.Random(714)
    for i in range(128):
        if i % 4 == 0:
            data, ext = rng.randbytes(MIB), 'mp4'
        elif i % 4 == 1:
            data, ext = rng.randbytes(MIB // 8) * 8, 'bin'
        else:
            data = ' '.join(f'word{rng.randrange(20000)}' for _ in range(160000)).encode()[:MIB]
            ext = 'txt'
        (root / f'{i}.{ext}').write_bytes(data)
    return manifest(root)


def manifest(root):
    return {str(p.relative_to(root)): {'bytes': p.stat().st_size,
            'sha256': hashlib.sha256(p.read_bytes()).hexdigest()}
            for p in sorted(root.rglob('*')) if p.is_file()}


def run_timed(command, cwd):
    started = time.perf_counter()
    process = subprocess.Popen(['/usr/bin/time', '-l', *map(str, command)], cwd=cwd,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                               start_new_session=True)
    try:
        _, error_bytes = process.communicate(timeout=600)
    except BaseException:
        # time(1) is a wrapper: stop the whole group we created before removing
        # its temporary inputs/outputs. Never signal an existing app process.
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.communicate()
        raise
    elapsed = time.perf_counter() - started
    stderr = error_bytes.decode(errors='replace')
    if process.returncode:
        raise RuntimeError(f'{command[0]} exited {process.returncode}: {stderr[-2000:]}')
    rss = re.search(r'(\d+)\s+maximum resident set size', stderr)
    if not rss:
        raise RuntimeError('macOS time -l did not report RSS')
    return {'seconds': elapsed, 'peak_rss_bytes': int(rss[1])}


def tools_for(fz):
    seven = shutil.which('7zz') or shutil.which('7z')
    if not seven:
        raise SystemExit('Install official 7-Zip (Homebrew sevenzip) first.')
    return [
        ('osz-zip', 'Open Silicon Zip', 'zip', 'normal; default high-speed on, threads auto',
         lambda a, c: [fz, 'create', a, c], lambda a, d: [fz, 'extract', a, '-o', d]),
        ('7zip-zip', '7-Zip', 'zip', 'ZIP -mx=5; automatic threads',
         lambda a, c: [seven, 'a', '-tzip', '-mx=5', a, c.name], lambda a, d: [seven, 'x', '-y', f'-o{d}', a]),
        ('infozip-zip', 'Info-ZIP', 'zip', 'ZIP -6; recursive; no extra fields',
         lambda a, c: ['/usr/bin/zip', '-q', '-r', '-6', '-X', a, c.name], lambda a, d: ['/usr/bin/unzip', '-q', a, '-d', d]),
        ('libarchive-zip', 'libarchive / bsdtar', 'zip', 'ZIP default Deflate; no macOS metadata',
         lambda a, c: ['/usr/bin/bsdtar', '--no-mac-metadata', '--format', 'zip', '-cf', a, c.name], lambda a, d: ['/usr/bin/bsdtar', '-xf', a, '-C', d]),
        ('osz-7z', 'Open Silicon Zip', '7z', 'normal; default high-speed on, adaptive solid blocks, threads auto',
         lambda a, c: [fz, 'create', a, c], lambda a, d: [fz, 'extract', a, '-o', d]),
        ('osz-zip-full', 'Open Silicon Zip (high-speed off)', 'zip', 'normal; --high-speed off; automatic threads',
         lambda a, c: [fz, 'create', a, c, '--high-speed', 'off'], lambda a, d: [fz, 'extract', a, '-o', d]),
        ('osz-7z-full', 'Open Silicon Zip (high-speed off)', '7z', 'normal; --high-speed off; adaptive solid blocks, automatic threads',
         lambda a, c: [fz, 'create', a, c, '--high-speed', 'off'], lambda a, d: [fz, 'extract', a, '-o', d]),
        ('7zip-7z', '7-Zip', '7z', '7z -mx=5; default solid and automatic threads',
         lambda a, c: [seven, 'a', '-t7z', '-mx=5', a, c.name], lambda a, d: [seven, 'x', '-y', f'-o{d}', a]),
        *([
        ('bandizip-zip', 'Bandizip 365', 'zip', 'official CLI c -l:5 -y -storeroot:yes; includes application startup',
         lambda a, c: [BANDI, 'c', '-y', '-l:5', '-storeroot:yes', a, c], lambda a, d: [BANDI, 'x', '-y', f'-o:{d}', a]),
        ('bandizip-7z', 'Bandizip 365', '7z', 'official CLI c -l:5 -y -storeroot:yes; includes application startup',
         lambda a, c: [BANDI, 'c', '-y', '-l:5', '-storeroot:yes', a, c], lambda a, d: [BANDI, 'x', '-y', f'-o:{d}', a]),
        ] if BANDI.exists() else []),
    ]


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--fz', default='engine/fz-core/target/release/fz')
    ap.add_argument('--output', required=True)
    ap.add_argument('--repetitions', type=int, default=3)
    ap.add_argument('--cases', help='comma-separated case IDs; default all available')
    ap.add_argument('--work-parent', help='parent for disposable temporary directory (must be accessible to Bandizip)')
    args = ap.parse_args()
    if args.repetitions < 1:
        ap.error('repetitions must be positive')
    cases = tools_for(str(Path(args.fz).resolve()))
    if args.cases:
        selected = set(args.cases.split(','))
        if selected - {c[0] for c in cases}:
            ap.error('unknown/unavailable case')
        cases = [c for c in cases if c[0] in selected]
    report = {'schema': 1, 'repetitions': args.repetitions, 'timing': 'perf_counter around /usr/bin/time -l process; warm filesystem cache; validation excluded',
              'rss': 'macOS time -l maximum resident set size in bytes, maximum across repeats',
              'hardware': {}, 'versions': {}, 'results': {}}
    for key in ['machdep.cpu.brand_string', 'hw.memsize', 'hw.perflevel0.physicalcpu', 'hw.perflevel1.physicalcpu']:
        report['hardware'][key] = subprocess.check_output(['sysctl', '-n', key], text=True).strip()
    report['hardware']['macOS'] = subprocess.check_output(['sw_vers', '-productVersion'], text=True).strip()
    report['versions']['fz_sha256'] = hashlib.sha256(Path(args.fz).read_bytes()).hexdigest()
    for name, command in [('7-Zip', [shutil.which('7zz') or shutil.which('7z')]), ('libarchive', ['/usr/bin/bsdtar', '--version']), ('Info-ZIP', ['/usr/bin/zip', '-v']), ('UnZip', ['/usr/bin/unzip', '-v'])]:
        report['versions'][name] = subprocess.run(command, capture_output=True, text=True).stdout.strip().splitlines()[:3]
    if BANDI.exists():
        with (BANDI.parent.parent/'Info.plist').open('rb') as f:
            report['versions']['Bandizip 365'] = plistlib.load(f)['CFBundleShortVersionString']
    with tempfile.TemporaryDirectory(prefix='osz-competitors-', dir=args.work_parent) as tmp:
        work = Path(tmp).resolve()
        corpus = work / 'corpus'
        expected = corpus_at(corpus)
        report['input_bytes'] = sum(x['bytes'] for x in expected.values())
        report['corpus_sha256'] = hashlib.sha256(b''.join(bytes.fromhex(x['sha256']) for x in expected.values())).hexdigest()
        for repeat in range(args.repetitions):
            # Rotate order deterministically to spread thermal/order effects.
            order = cases[repeat % len(cases):] + cases[:repeat % len(cases)]
            for key, name, fmt, settings, create, extract in order:
                archive = work / f'archive.{fmt}'
                dest = work / 'extracted'
                dest.mkdir()
                print(f'round {repeat+1}: {key}', flush=True)
                commands = [list(map(str, create(archive, corpus))), list(map(str, extract(archive, dest)))]
                compression = run_timed(commands[0], work)
                if not archive.is_file():
                    raise RuntimeError(f'{key} returned before archive creation completed')
                size = archive.stat().st_size
                methods = None
                if fmt == 'zip':
                    with zipfile.ZipFile(archive) as z:
                        methods = {str(m): sum(e.file_size for e in z.infolist() if e.compress_type == m) for m in {e.compress_type for e in z.infolist()}}
                extraction = run_timed(commands[1], work)
                actual_root = dest / 'corpus'
                if manifest(actual_root) != expected or any(p != actual_root for p in dest.iterdir()):
                    raise RuntimeError(f'{key} extracted content mismatch / unfinished operation')
                # 7-Zip integrity check, outside timing; SHA-256 comparison above is tool-independent.
                checker = shutil.which('7zz') or shutil.which('7z')
                subprocess.run([checker, 't', str(archive)], check=True, capture_output=True, timeout=300)
                row = report['results'].setdefault(key, {'name': name, 'format': fmt, 'settings': settings, 'samples': []})
                row['commands'] = [[s.replace(str(work), '$WORK').replace(str(Path(args.fz).resolve()), '$FZ') for s in cmd] for cmd in commands]
                row['samples'].append({'create': compression, 'extract': extraction, 'archive_bytes': size,
                                       'zip_method_input_bytes': methods, 'sha256_roundtrip': True, 'sevenzip_test': True})
                archive.unlink()
                shutil.rmtree(dest)
        for row in report['results'].values():
            row['summary'] = {op: {'median_seconds': statistics.median(s[op]['seconds'] for s in row['samples']),
                                  'peak_rss_bytes': max(s[op]['peak_rss_bytes'] for s in row['samples'])} for op in ['create', 'extract']}
            row['summary']['archive_bytes'] = statistics.median(s['archive_bytes'] for s in row['samples'])
    Path(args.output).write_text(json.dumps(report, indent=2, ensure_ascii=False) + '\n')


if __name__ == '__main__':
    main()
