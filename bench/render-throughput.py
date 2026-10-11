#!/usr/bin/env python3
"""Render paired performance tables from throughput.py results (matplotlib)."""
import argparse
import json
from pathlib import Path
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
from matplotlib.patches import Rectangle

LABELS = {
    'zip-mixed': 'ZIP / 128 MiB mixed', 'zip-large': 'ZIP / 64 MiB single file',
    'zip-ppmd': 'ZIP PPMd / 2 MiB', '7z-mixed': '7z / 128 MiB mixed',
    '7z-mixed-full': '7z / high-speed off / 128 MiB', '7z-single': '7z / 64 MiB single file',
    '7z-store-single': '7z copy / 256 MiB single file', '7z-small-files': '7z / 2,048 small files',
}


def render(report, output):
    plt.rcParams.update({'font.family': 'DejaVu Sans', 'svg.fonttype': 'path', 'svg.hashsalt': 'osz-throughput'})
    fig = plt.figure(figsize=(17, 11.2), facecolor='white')
    ax = fig.add_axes([0, 0, 1, 1]); ax.set(xlim=(0, 1700), ylim=(1120, 0)); ax.axis('off')
    ink, muted = '#0f172a', '#475569'
    bounds = [40, 395, 510, 700, 890, 1080, 1260, 1450, 1660]
    def text(x, y, value, size=11, color=ink, weight='normal', ha='left'):
        ax.text(x, y, value, fontsize=size, color=color, fontweight=weight, va='center', ha=ha)
    text(40, 40, 'OPEN SILICON ZIP', 24, weight='bold')
    text(40, 78, 'preview.2 → preview.3  /  Paired Performance', 18, muted)
    text(1660, 40, report['measurement_date'], 12, muted, ha='right')
    text(1660, 78, f"{report['environment']['cpu']}  /  {report['environment']['memory_bytes']/2**30:.0f} GiB", 12, muted, ha='right')
    text(40, 116, f"{report['repetitions']} runs after warmup  ·  Alternating order  ·  Full file SHA-256 verification", 12, muted)
    text(1660, 116, 'Green + bold = lower per-case value', 11, '#166534', ha='right')
    headers = ['Workload', 'Version', 'Compress\nms ↓', 'Extract own\nms ↓', 'Same archive\nms ↓', 'Archive\nMiB ↓', 'Compress RSS\nMiB ↓', 'Extract RSS\nMiB ↓']
    ax.add_patch(Rectangle((40, 145), 1620, 52, facecolor=ink))
    for column, title in enumerate(headers):
        text((bounds[column]+bounds[column+1])/2, 171, title, 11, 'white', 'bold', 'center')
    y = 197
    for name, case in report['results'].items():
        values = []
        for item in case['binaries']:
            values.append([item['create_seconds']*1000, item['extract_own_seconds']*1000,
                           item['extract_reference_seconds']*1000, item['samples'][0]['archive_bytes']/2**20,
                           item['create_peak_rss_bytes']/2**20, item['extract_own_peak_rss_bytes']/2**20])
        rounded = [[round(value, 3 if i == 3 else 1) for i, value in enumerate(row)] for row in values]
        for index, row in enumerate(rounded):
            bg = '#f8fafc' if index == 0 else 'white'
            ax.add_patch(Rectangle((40, y), 1620, 46, facecolor=bg))
            text(53, y+23, LABELS[name] if index == 0 else '', 11)
            text((395+510)/2, y+23, f'preview.{index+2}', 10, muted, ha='center')
            for metric, value in enumerate(row):
                best = value == min(r[metric] for r in rounded)
                left, right = bounds[metric+2:metric+4]
                if best: ax.add_patch(Rectangle((left+2, y+2), right-left-4, 42, facecolor='#dcfce7'))
                text((left+right)/2, y+23, f'{value:.3f}' if metric == 3 else f'{value:.1f}',
                     12, '#166534' if best else ink, 'bold' if best else 'normal', 'center')
            y += 46
        ax.plot([40, 1660], [y, y], color='#cbd5e1', linewidth=.8)
    notes = [
        'Own: each version reads its own archive. Same archive: both read the identical baseline archive. Time includes process startup and I/O.',
        'Default mixed 7z uses more independent blocks; archive size changes. Copy mode stores .mp4 by extension (not video encoding).',
        'RSS is the maximum across runs. Bold indicates a measured minimum, not statistical significance; small differences may be noise.',
        'Source: docs/benchmarks/performance-round3.json  ·  Reproduce: bench/throughput.py  ·  Historical competitor scores are unchanged.',
    ]
    for index, note in enumerate(notes): text(40, y+36+index*29, note, 10, muted)
    output.parent.mkdir(parents=True, exist_ok=True)
    for ext in ['svg', 'png']:
        fig.savefig(output.with_suffix('.'+ext), dpi=140,
                    metadata={'Creator': 'Open Silicon Zip benchmark renderer', 'Date': report['measurement_date']})
    plt.close(fig)


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--input', type=Path, default=Path('docs/benchmarks/performance-round3.json'))
    p.add_argument('--output', type=Path, default=Path('docs/benchmarks/performance-round3'))
    a = p.parse_args(); render(json.loads(a.input.read_text()), a.output)
