#!/usr/bin/env python3
"""Measure the large stored-file path, not compression ratio. macOS only."""
import hashlib,json,pathlib,random,statistics,subprocess,sys,tempfile,time
bins=[str(pathlib.Path(p).resolve()) for p in sys.argv[1:]]
assert len(bins)==2,'usage: large-store.py BASELINE CANDIDATE'
with tempfile.TemporaryDirectory(prefix='osz-large-store-') as tmp:
    root=pathlib.Path(tmp);source=root/'media.mp4'
    block=random.Random(715).randbytes(1024*1024)
    with source.open('wb') as f:
        for _ in range(512): f.write(block)
    samples=[[],[]];rss=[[],[]]
    for repeat in range(6):
        for i in ([0,1] if repeat%2==0 else [1,0]):
            out=root/'media.zip';out.unlink(missing_ok=True)
            start=time.perf_counter()
            p=subprocess.run(['/usr/bin/time','-l',bins[i],'create',str(out),str(source),'--level','store'],capture_output=True,text=True,timeout=120)
            elapsed=time.perf_counter()-start
            assert p.returncode==0,p.stderr
            if repeat == 0: continue  # Warm filesystem/code caches before recording.
            samples[i].append(elapsed)
            rss[i].append(next(int(l.split()[0]) for l in p.stderr.splitlines() if 'maximum resident set size' in l)/1048576)
            subprocess.run([bins[i],'test',str(out)],check=True,capture_output=True)
    print(json.dumps({'input_MiB':512,'one_MiB_block_sha256':hashlib.sha256(block).hexdigest(),'repetitions':5,'results':[{'median_seconds':statistics.median(s),'MiB_per_second':512/statistics.median(s),'peak_RSS_MiB':max(m),'seconds':s} for s,m in zip(samples,rss)]},indent=2))
