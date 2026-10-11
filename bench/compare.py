#!/usr/bin/env python3
"""Repeatable local ARM benchmark. Same seeded corpus and alternating binary order."""
import os,hashlib,json,pathlib,random,statistics,subprocess,sys,tempfile,time
binaries=[str(pathlib.Path(x).resolve()) for x in sys.argv[1:]]
assert len(binaries)==2, 'usage: compare.py BASELINE CANDIDATE'
with tempfile.TemporaryDirectory(prefix='osz-bench-') as tmp:
    w=pathlib.Path(tmp);corpus=w/'corpus';corpus.mkdir();r=random.Random(714)
    for i in range(128):
        if i%4==0: data=r.randbytes(1024*1024);ext='mp4'
        elif i%4==1: data=r.randbytes(128*1024)*8;ext='bin'
        else:
            # Moderate repetition, deterministic UTF-8 text, 1 MiB per file.
            words=[f'word{r.randrange(20000)}' for _ in range(160000)]
            data=(' '.join(words)).encode()[:1024*1024];ext='txt'
        (corpus/f'{i}.{ext}').write_bytes(data)
    total=sum(p.stat().st_size for p in corpus.iterdir())
    checksum=hashlib.sha256(b''.join(hashlib.sha256(p.read_bytes()).digest() for p in sorted(corpus.iterdir()))).hexdigest()
    cases=[('zip-fast',['--level','fast'],'zip'),('zip-normal',[],'zip'),('7z-normal',[],'7z')]
    selected=os.environ.get('OSZ_BENCH_CASES','').split(',')
    if selected != ['']: cases=[c for c in cases if c[0] in selected]
    repetitions=int(os.environ.get('OSZ_BENCH_REPETITIONS','3'))
    assert cases and repetitions>0
    report={'input_bytes':total,'corpus_sha256':checksum,'repetitions':repetitions,'results':{}}
    for case,args,ext in cases:
        samples=[[],[]];rss=[[],[]];sizes=[[],[]]
        for repeat in range(repetitions):
            for bi in ([0,1] if repeat%2==0 else [1,0]):
                output=w/f'out{bi}.{ext}';output.unlink(missing_ok=True)
                start=time.perf_counter()
                result=subprocess.run(['/usr/bin/time','-l',binaries[bi],'create',str(output),str(corpus),*args],capture_output=True,timeout=180)
                elapsed=time.perf_counter()-start
                assert result.returncode==0,result.stderr.decode()
                samples[bi].append(elapsed)
                sizes[bi].append(output.stat().st_size)
                for line in result.stderr.decode().splitlines():
                    if 'maximum resident set size' in line:rss[bi].append(int(line.split()[0]))
                assert subprocess.run([binaries[bi],'test',str(output)],capture_output=True).returncode==0
        report['results'][case]=[{'median_seconds':statistics.median(s),'MiB_per_second':total/1048576/statistics.median(s),'peak_RSS_MiB':max(m)/1048576,'seconds':s,'archive_bytes':z} for s,m,z in zip(samples,rss,sizes)]
    print(json.dumps(report,indent=2))
