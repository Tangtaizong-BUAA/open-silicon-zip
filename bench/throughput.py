#!/usr/bin/env python3
"""Paired codec workloads: create, own-archive extraction and same-archive extraction.

All inputs are synthetic. Each timed output is validated outside the timing window.
"""
import argparse
from datetime import date
import platform
import hashlib
import json
from pathlib import Path
import random
import shutil
import statistics
import subprocess
import tempfile

from competitors import corpus_at, manifest, run_timed


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('baseline', type=Path)
    parser.add_argument('candidate', type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--repetitions', type=int, default=5)
    parser.add_argument('--cases', help='Comma-separated case IDs; omit for all workloads')
    args = parser.parse_args()
    if args.repetitions < 1:
        parser.error('repetitions must be positive')
    binaries = [args.baseline.resolve(), args.candidate.resolve()]
    report = {
        'repetitions': args.repetitions, 'warmups_per_binary_per_case': 1,
        'timing': 'wall time including process startup and JSON progress; alternating order; warm cache; full SHA-256 validation excluded',
        'reference_archive': 'one archive produced by the baseline, read by both versions',
        'measurement_date': date.today().isoformat(),
        'environment': {'architecture': platform.machine(), 'macos': platform.mac_ver()[0],
                        'cpu': subprocess.check_output(['sysctl', '-n', 'machdep.cpu.brand_string'], text=True).strip(),
                        'memory_bytes': int(subprocess.check_output(['sysctl', '-n', 'hw.memsize'], text=True))},
        'binary_sha256': [hashlib.sha256(b.read_bytes()).hexdigest() for b in binaries],
        'results': {},
    }
    with tempfile.TemporaryDirectory(prefix='osz-throughput-') as temp:
        root = Path(temp)
        mixed = root/'mixed'; corpus_at(mixed)
        rng = random.Random(719)
        block = ' '.join(f'word{rng.randrange(20000)}' for _ in range(170000)).encode()[:1 << 20]
        words = root/'words'; words.mkdir(); (words/'词语.txt').write_bytes(block*2)
        large = root/'large'; large.mkdir(); (large/'large.txt').write_bytes(block*64)
        media = root/'media'; media.mkdir()
        with (media/'synthetic.mp4').open('wb') as file:
            noise = rng.randbytes(1 << 20)
            for _ in range(256): file.write(noise)
        tiny = root/'tiny'; tiny.mkdir()
        for index in range(2048):
            (tiny/f'文件-{index:04}.txt').write_bytes(block[:8192])
        cases = [
            ('zip-mixed', mixed, 'zip', []),
            ('zip-large', large, 'zip', []),
            ('zip-ppmd', words, 'zip', ['--method', 'ppmd']),
            ('7z-mixed', mixed, '7z', []),
            ('7z-mixed-full', mixed, '7z', ['--high-speed', 'off']),
            ('7z-single', large, '7z', []),
            ('7z-store-single', media, '7z', []),
            ('7z-small-files', tiny, '7z', []),
        ]
        selected = set(args.cases.split(',')) if args.cases else {c[0] for c in cases}
        unknown = selected - {c[0] for c in cases}
        if unknown: parser.error(f'unknown cases: {sorted(unknown)}')
        for name, source, extension, flags in cases:
            if name not in selected: continue
            expected = manifest(source)
            reference = root/f'reference.{extension}'
            reference.unlink(missing_ok=True)
            subprocess.run([str(binaries[0]), '--json', 'create', str(reference), str(source), *flags],
                           check=True, stdout=subprocess.DEVNULL)
            samples = [[], []]
            for repeat in range(-1, args.repetitions):
                for index in ([0, 1] if repeat % 2 == 0 else [1, 0]):
                    archive = root/f'output.{extension}'
                    archive.unlink(missing_ok=True)
                    command = [binaries[index], '--json']
                    create = run_timed([*command, 'create', archive, source, *flags], root)
                    row = {'create': create, 'archive_bytes': archive.stat().st_size}
                    for key, input_archive in [('extract_own', archive), ('extract_reference', reference)]:
                        dest = root/'extracted'
                        if dest.exists(): shutil.rmtree(dest)
                        row[key] = run_timed([*command, 'extract', input_archive, '-o', dest], root)
                        if manifest(dest/source.name) != expected:
                            raise RuntimeError(f'{name}: {key} content mismatch')
                    if repeat >= 0: samples[index].append(row)
            report['results'][name] = {
                'input_bytes': sum(v['bytes'] for v in expected.values()), 'input_files': len(expected),
                'input_sha256': hashlib.sha256(json.dumps(expected, sort_keys=True).encode()).hexdigest(),
                'create_options': flags, 'reference_archive_bytes': reference.stat().st_size,
                'reference_archive_sha256': hashlib.sha256(reference.read_bytes()).hexdigest(),
                'binaries': [dict(samples=rows, **{
                    f'{op}_{metric}': (statistics.median(row[op][metric] for row in rows) if metric == 'seconds'
                                      else max(row[op][metric] for row in rows))
                    for op in ('create', 'extract_own', 'extract_reference')
                    for metric in ('seconds', 'peak_rss_bytes')}) for rows in samples],
            }
            summary = report['results'][name]['binaries']
            print(name, [(round(r['create_seconds'], 4), round(r['extract_own_seconds'], 4),
                          round(r['extract_reference_seconds'], 4), r['samples'][0]['archive_bytes'])
                         for r in summary], flush=True)
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(json.dumps(report, indent=2)+'\n')
            archive.unlink(); reference.unlink(); shutil.rmtree(dest)


if __name__ == '__main__':
    main()
