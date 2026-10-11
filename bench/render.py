#!/usr/bin/env python3
"""Render the README benchmark table from measured JSON (requires matplotlib)."""
import argparse
import json
from pathlib import Path

import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
from matplotlib.patches import Rectangle

ORDER = {
    'ZIP': ['osz-zip', 'osz-zip-full', '7zip-zip', 'bandizip-zip', 'infozip-zip', 'libarchive-zip'],
    '7z': ['osz-7z', 'osz-7z-full', '7zip-7z', 'bandizip-7z'],
}
LABELS = {
    'osz-zip': 'Open Silicon Zip · Default',
    'osz-7z': 'Open Silicon Zip · Default',
    'osz-zip-full': 'Open Silicon Zip · High-speed off',
    'osz-7z-full': 'Open Silicon Zip · High-speed off',
    '7zip-zip': '7-Zip 26.04', '7zip-7z': '7-Zip 26.04',
    'bandizip-zip': 'Bandizip 365 7.39 †', 'bandizip-7z': 'Bandizip 365 7.39 †',
    'infozip-zip': 'Info-ZIP 3.0 / UnZip 6.00',
    'libarchive-zip': 'libarchive 3.7.4',
}
PRECISION = [3, 3, 2, 1, 1]


def values(report, key):
    s = report['results'][key]['summary']
    return [s['create']['median_seconds'], s['extract']['median_seconds'],
            s['archive_bytes'] / 1048576, s['create']['peak_rss_bytes'] / 1048576,
            s['extract']['peak_rss_bytes'] / 1048576]


def render(report, output):
    plt.rcParams.update({'font.family': 'DejaVu Sans', 'svg.fonttype': 'path', 'svg.hashsalt': 'open-silicon-zip'})
    fig = plt.figure(figsize=(17, 10.7), facecolor='#ffffff')
    ax = fig.add_axes([0, 0, 1, 1])
    ax.set(xlim=(0, 1700), ylim=(1070, 0))
    ax.axis('off')
    ink, muted = '#0f172a', '#475569'
    best_bg, best_fg = '#dcfce7', '#166534'
    second_bg, second_fg = '#e0edff', '#1e40af'

    def text(x, y, value, size=13, color=ink, weight='normal', ha='left'):
        ax.text(x, y, value, fontsize=size, color=color, fontweight=weight,
                ha=ha, va='center')

    text(46, 48, 'OPEN SILICON ZIP', 25, weight='bold')
    text(46, 86, 'Archiver Benchmark', 19, muted)
    text(1654, 49, report['measurement_date'], 12, muted, ha='right')
    text(1654, 84, 'Apple M4 Max  /  12P + 4E  /  48 GiB  /  macOS 15.7.1', 12, muted, ha='right')
    text(46, 129, '128 MiB mixed corpus   ·   3 runs per case   ·   30/30 verified round trips', 13, muted)
    for x, bg, fg, label, weight in [(1070, best_bg, best_fg, 'Best', 'bold'),
                                    (1220, second_bg, second_fg, 'Second-best', 'normal')]:
        ax.add_patch(Rectangle((x, 113), 25, 25, facecolor=bg, edgecolor='none'))
        text(x + 35, 126, label, 12, fg, weight)
    text(1654, 127, '↓ Lower is better', 12, muted, ha='right')

    bounds = [46, 470, 708, 946, 1184, 1422, 1654]
    headers = ['Software / mode', 'Compress  ↓\nseconds', 'Extract  ↓\nseconds',
               'Archive size  ↓\nMiB', 'Compress RSS  ↓\nMiB', 'Extract RSS  ↓\nMiB']
    for fmt, title_y, table_y in [('ZIP', 185, 210), ('7z', 623, 650)]:
        text(46, title_y, fmt, 19, weight='bold')
        text(1654, title_y, 'Ranked within this format only', 11, muted, ha='right')
        keys = ORDER[fmt]
        displayed = {key: [round(v, p) for v, p in zip(values(report, key), PRECISION)] for key in keys}
        ranks = [sorted({displayed[key][i] for key in keys}) for i in range(5)]
        ax.add_patch(Rectangle((46, table_y), 1608, 48, facecolor=ink, edgecolor='none'))
        for col, label in enumerate(headers):
            x = bounds[col] + 18 if col == 0 else (bounds[col] + bounds[col + 1]) / 2
            text(x, table_y + 24, label, 11, '#ffffff', 'bold', 'left' if col == 0 else 'center')
        for row, key in enumerate(keys):
            y = table_y + 48 + row * 52
            base = '#f8fafc' if row % 2 == 0 else '#ffffff'
            ax.add_patch(Rectangle((46, y), 1608, 52, facecolor=base, edgecolor='none'))
            text(64, y + 26, LABELS[key], 12)
            for i, v in enumerate(displayed[key]):
                rank = ranks[i].index(v)
                bg, fg, weight = (best_bg, best_fg, 'bold') if rank == 0 else (
                    (second_bg, second_fg, 'normal') if rank == 1 else (base, ink, 'normal'))
                x1, x2 = bounds[i + 1], bounds[i + 2]
                ax.add_patch(Rectangle((x1 + 2, y + 3), x2 - x1 - 4, 46, facecolor=bg, edgecolor='none'))
                text((x1 + x2) / 2, y + 26, f'{v:.{PRECISION[i]}f}', 14, fg, weight, 'center')
            ax.plot([46, 1654], [y + 52, y + 52], color='#e2e8f0', linewidth=0.6)

    notes = [
        'Time: median of 3 runs. RSS: maximum across runs. Warm filesystem cache; extraction reads each tool’s own archive.',
        '† Bandizip includes full app initialization; other rows measure CLI engines. Defaults, threads and compression ratios differ.',
        'High-speed mode stores .bin / .mp4 by extension. Best / second-best are per column; ties use displayed precision. No overall score.',
    ]
    for y, note in zip([950, 978, 1006], notes):
        text(46, y, note, 10, muted)
    text(46, 1044, 'Source: docs/benchmarks/competitors-m4-max.json   ·   Reproduce: bench/competitors.py', 10, muted)
    text(1654, 1044, 'github.com/Tangtaizong-BUAA/open-silicon-zip', 10, muted, ha='right')
    output.parent.mkdir(parents=True, exist_ok=True)
    for ext in ['svg', 'png']:
        fig.savefig(output.with_suffix('.' + ext), dpi=150, facecolor=fig.get_facecolor(),
                    metadata={'Creator': 'Open Silicon Zip benchmark renderer', 'Date': report['measurement_date']})
    plt.close(fig)


if __name__ == '__main__':
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--input', type=Path, default=Path('docs/benchmarks/competitors-m4-max.json'))
    ap.add_argument('--output', type=Path, default=Path('docs/benchmarks/comparison'))
    args = ap.parse_args()
    render(json.loads(args.input.read_text()), args.output)
