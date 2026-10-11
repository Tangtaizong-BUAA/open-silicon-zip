#!/usr/bin/env python3
"""Verify a Sparkle feed and its release ZIP using only the public key."""
import argparse
import pathlib
import subprocess
import tempfile
import xml.etree.ElementTree as ET

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('feed', type=pathlib.Path)
p.add_argument('archive', type=pathlib.Path)
p.add_argument('--public-key', required=True)
a = p.parse_args()
helper = pathlib.Path(__file__).with_name('update-key.swift')
data = a.feed.read_bytes()
marker = b'<!-- sparkle-signatures:\n'
content, found, block = data.rpartition(marker)
if not found or not block.endswith(b'-->\n'):
    raise SystemExit('Missing or malformed feed signature')
fields = dict(line.split(': ', 1) for line in block[:-4].decode().splitlines() if ': ' in line)
if int(fields['length']) != len(content):
    raise SystemExit('Feed length mismatch')
with tempfile.TemporaryDirectory(prefix='osz-verify-') as temp:
    signed = pathlib.Path(temp)/'feed-content.xml'
    signed.write_bytes(content)
    subprocess.run(['swift', str(helper), 'verify', a.public_key, str(signed), fields['edSignature']], check=True)
root = ET.fromstring(content)
entries = root.findall('./channel/item/enclosure')
if len(entries) != 1:
    raise SystemExit('Expected exactly one full update archive')
entry = entries[0]
if int(entry.attrib['length']) != a.archive.stat().st_size:
    raise SystemExit('Archive length mismatch')
subprocess.run(['swift', str(helper), 'verify', a.public_key, str(a.archive),
                entry.attrib['{http://www.andymatuschak.org/xml-namespaces/sparkle}edSignature']], check=True)
print('PASS: signed feed and archive verified with the public key only')
