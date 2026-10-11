#!/usr/bin/env bash
# FastZip benchmark harness (06 §3). Usage: bench/run.sh [workdir] ; needs release build of fz.
# Builds the mixed corpus (40% text/code, 30% binaries, 30% already-compressed), runs each
# budgeted operation, and prints a JSON report (input MB/s, wall time, peak RSS).
set -euo pipefail
FZ="$(realpath "${FZ:-$(dirname "$0")/../engine/fz-core/target/release/fz}")"
W="${1:-/tmp/fz-bench}"; SIZE_MB="${SIZE_MB:-200}"
mkdir -p "$W/corpus"; cd "$W"
if [ ! -f corpus/.done ]; then
  rm -rf corpus; mkdir corpus
  python3 - "$SIZE_MB" <<'PY'
import os,sys,random
mb=int(sys.argv[1]); r=random.Random(1)
# Realistic English text: Zipf-distributed words (worst-case uniform random
# word salad makes LZMA match search pathologically slow and is not the
# "text/code" corpus 06 section 3 defines).
import math
n_w=20000
words=[]
try:
    with open('/usr/share/dict/words') as f:
        words=[w.strip().lower() for w in f if w.strip().isalpha()][:n_w]
except Exception:
    words=[''.join(r.choice('abcdefghijklmnopqrstuvwxyz') for _ in range(r.randint(3,9))) for _ in range(n_w)]
if not words: words=['word']
# Zipf weights: rank r -> 1/r
wts=[1.0/(i+1) for i in range(len(words))]
cum=[]; t=0.0
for w in wts: t+=w; cum.append(t)
def zipf():
    x=r.random()*t
    import bisect
    return words[bisect.bisect_left(cum,x)]
def text(n): 
    out=[];s=0
    while s<n:
        w=' '.join(zipf() for _ in range(12))+'\n'; out.append(w); s+=len(w)
    return ''.join(out).encode()
tot=mb*1024*1024
for i in range(int(tot*0.4//(256*1024))):
    open(f'corpus/src{i}.txt','wb').write(text(256*1024))
for i in range(int(tot*0.3//(512*1024))):   # "binaries": semi-compressible
    b=bytearray(os.urandom(64*1024))*8; open(f'corpus/bin{i}.dat','wb').write(bytes(b))
for i in range(int(tot*0.3//(1024*1024))):
    open(f'corpus/media{i}.mp4','wb').write(os.urandom(1024*1024))
open('corpus/.done','w').close()
PY
fi
total=$(du -sm corpus | cut -f1)
run() { # name, command...
  local name="$1"; shift
  python3 - "$name" "$total" "$@" <<'PYBENCH'
import json,platform,re,subprocess,sys,time
name,total,*cmd=sys.argv[1:]
mac=platform.system()=='Darwin'
timecmd=['/usr/bin/time','-l'] if mac else ['/usr/bin/time','-f','OSZ_RSS_KIB=%M']
start=time.perf_counter()
r=subprocess.run(timecmd+cmd,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE,text=True)
elapsed=time.perf_counter()-start
if r.returncode:
    print(r.stderr,file=sys.stderr)
    raise SystemExit(r.returncode)
pattern=r'(\d+)\s+maximum resident set size' if mac else r'OSZ_RSS_KIB=(\d+)'
match=re.search(pattern,r.stderr)
assert match, 'Missing peak RSS measurement'
rss=int(match.group(1)) / (1048576 if mac else 1024)
print(json.dumps({'op':name,'seconds':round(elapsed,3),'input_MiBps':round(float(total)/elapsed,1),'peak_RSS_MiB':round(rss,1)}))
PYBENCH
}
rm -f out.*
run zip_fast_highspeed   "$FZ" create out.fast.zip corpus --level fast
run zip_normal           "$FZ" create out.normal.zip corpus
run zipx_zstd_normal     "$FZ" create out.zipx corpus --format zipx
run 7z_normal_lzma2      "$FZ" create out.7z corpus
run 7z_maximum           "$FZ" create out.max.7z corpus --level max
run tgz_normal           "$FZ" create out.tgz corpus
run tzst_normal          "$FZ" create out.tzst corpus
rm -rf x; run zip_extract    "$FZ" extract out.normal.zip -o x
rm -rf x; run 7z_extract     "$FZ" extract out.7z -o x
rm -rf x; run tgz_extract    "$FZ" extract out.tgz -o x
run zip_test             "$FZ" test out.normal.zip
if command -v zip >/dev/null; then run ref_zip_cli zip -qr out.ref.zip corpus; fi
if command -v 7z  >/dev/null; then run ref_7z_cli  7z a -bd out.ref.7z corpus; fi
