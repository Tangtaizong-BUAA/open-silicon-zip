#!/usr/bin/env python3
"""Paired create/extract timings, including process startup and progress shutdown.

Synthetic inputs only. Every timed round trip is checked file-by-file afterwards.
"""
import argparse
import hashlib
import json
from pathlib import Path
import random
import shutil
import statistics
import tempfile
from competitors import corpus_at, manifest, run_timed

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('baseline', type=Path)
p.add_argument('candidate', type=Path)
p.add_argument('--output', type=Path, required=True)
p.add_argument('--repetitions', type=int, default=5)
a = p.parse_args()
assert a.repetitions > 0
binaries = [a.baseline.resolve(), a.candidate.resolve()]
report = {'repetitions': a.repetitions, 'warmups_per_binary_per_case': 1, 'timing': 'wall time including process startup, --json progress and shutdown; warm cache; alternating order; validation excluded',
          'binary_sha256': [hashlib.sha256(b.read_bytes()).hexdigest() for b in binaries], 'results': {}}
with tempfile.TemporaryDirectory(prefix='osz-latency-') as tmp:
    root = Path(tmp)
    small = root/'small'; small.mkdir()
    (small/'中文.txt').write_bytes((b'Open Silicon Zip performance regression\n'*1000)[:32768])
    mixed = root/'mixed'; corpus_at(mixed)
    large = root/'large'; large.mkdir()
    rng = random.Random(719)
    block = ' '.join(f'word{rng.randrange(20000)}' for _ in range(170000)).encode()[:1024*1024]
    (large/'large.txt').write_bytes(block * 64)
    ppmd = root/'ppmd'; ppmd.mkdir()
    (ppmd/'words.txt').write_bytes(block * 2)
    cases = [('zip-small-json', small, 'zip', [], True), ('7z-small-json', small, '7z', [], True),
             ('zip-small-cli', small, 'zip', [], False), ('zip-large-json', large, 'zip', [], True),
             ('zip-mixed-json', mixed, 'zip', [], True), ('7z-mixed-json', mixed, '7z', [], True),
             ('7z-ppmd-json', ppmd, '7z', ['--method','ppmd'], True)]
    for name, corpus, ext, flags, json_mode in cases:
        expected = manifest(corpus)
        rows = [[], []]
        archive_hashes = [[], []]
        # Warm both binaries/codec paths before alternating measured runs.
        for binary in binaries:
            archive = root/f'warm.{ext}'; dest = root/'warm-extracted'
            archive.unlink(missing_ok=True)
            if dest.exists(): shutil.rmtree(dest)
            command = [str(binary)]+(['--json'] if json_mode else [])
            run_timed(command+['create',str(archive),str(corpus),*flags], root)
            run_timed(command+['extract',str(archive),'-o',str(dest)], root)
            assert manifest(dest/corpus.name) == expected, name
        archive.unlink(missing_ok=True); shutil.rmtree(dest)
        for repeat in range(a.repetitions):
            for index in ([0,1] if repeat%2 == 0 else [1,0]):
                archive = root/f'out.{ext}'; dest = root/'extracted'
                archive.unlink(missing_ok=True)
                if dest.exists(): shutil.rmtree(dest)
                command = [str(binaries[index])]+(['--json'] if json_mode else [])
                create = run_timed(command+['create',str(archive),str(corpus),*flags], root)
                extract = run_timed(command+['extract',str(archive),'-o',str(dest)], root)
                assert manifest(dest/corpus.name) == expected, name
                rows[index].append({'create':create,'extract':extract,'archive_bytes':archive.stat().st_size})
                archive_hashes[index].append(hashlib.sha256(archive.read_bytes()).hexdigest())
        report['results'][name] = {'input_bytes':sum(v['bytes'] for v in expected.values()),
            'input_sha256':hashlib.sha256(json.dumps(expected,sort_keys=True).encode()).hexdigest(),
            'identical_archive_bytes':len(set(archive_hashes[0]+archive_hashes[1]))==1,
            'binaries':[{'samples': row, 'create_seconds':statistics.median(r['create']['seconds'] for r in row),
                'extract_seconds':statistics.median(r['extract']['seconds'] for r in row),
                'create_peak_rss_bytes':max(r['create']['peak_rss_bytes'] for r in row),
                'extract_peak_rss_bytes':max(r['extract']['peak_rss_bytes'] for r in row)} for row in rows]}
        print(name, [(round(r['create_seconds'],4),round(r['extract_seconds'],4)) for r in report['results'][name]['binaries']], flush=True)
        a.output.parent.mkdir(parents=True,exist_ok=True)
        a.output.write_text(json.dumps(report,indent=2)+'\n')
