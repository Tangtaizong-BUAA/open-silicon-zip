#!/usr/bin/env python3
"""Sign a final release with Sparkle, without accessing macOS Keychain."""
import argparse
import html
import os
import pathlib
import plistlib
import shutil
import stat
import subprocess
import tempfile
import urllib.parse
import zipfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('archive', type=pathlib.Path)
p.add_argument('--appcast-tool', type=pathlib.Path, required=True)
p.add_argument('--key-file', type=pathlib.Path, default=pathlib.Path.home()/'.config/open-silicon-zip/update-signing.key')
p.add_argument('--output', type=pathlib.Path, required=True)
p.add_argument('--version', required=True)
p.add_argument('--base-url', default='https://home.argonai.cn/open-silicon-zip')
a = p.parse_args()
if urllib.parse.urlsplit(a.base_url).scheme != 'https':
    p.error('HTTPS is required')
key = a.key_file.expanduser().resolve(strict=True)
if key.is_relative_to(ROOT):
    p.error('Keep the private signing key outside the repository')
for path in (key, key.parent):
    st = path.stat()
    if st.st_uid != os.getuid() or stat.S_IMODE(st.st_mode) & 0o077:
        p.error('The key and its directory must belong to you and deny group/other access')
if not key.is_file():
    p.error('The key must be a regular file')
with zipfile.ZipFile(a.archive) as z:
    info = plistlib.loads(z.read('Open Silicon Zip.app/Contents/Info.plist'))
if info['CFBundleIdentifier'] != 'org.opensiliconzip.mac' or not info.get('SURequireSignedFeed'):
    p.error('Expected an Open Silicon Zip release requiring signed feeds')
with tempfile.TemporaryDirectory(prefix='osz-appcast-') as folder:
    stage = pathlib.Path(folder)
    shutil.copyfile(a.archive, stage/a.archive.name)
    (stage/a.archive.with_suffix('.html').name).write_text(
        '<p>Open Silicon Zip '+html.escape(a.version)+'</p>'
        '<p>Finder 右键简洁压缩与进度窗口；默认解压到独立文件夹；支持自动更新。</p>'
        '<p>此预览版尚未经过 Apple 公证。</p>', encoding='utf-8')
    subprocess.run([str(a.appcast_tool.resolve()), '--ed-key-file', str(key),
        '--download-url-prefix', a.base_url.rstrip('/')+'/releases/',
        '--maximum-deltas', '0', '--embed-release-notes',
        '--link', 'https://github.com/Tangtaizong-BUAA/open-silicon-zip', str(stage)], check=True)
    generated = stage/'appcast.xml'
    subprocess.run(['python3', str(ROOT/'scripts/verify-appcast.py'), str(generated),
                    str(a.archive.resolve()), '--public-key', info['SUPublicEDKey']], check=True)
    a.output.parent.mkdir(parents=True, exist_ok=True)
    temporary = a.output.with_suffix('.xml.tmp')
    shutil.copyfile(generated, temporary)
    temporary.replace(a.output)
print('Signed and independently verified:', a.output)
