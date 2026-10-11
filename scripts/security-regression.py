#!/usr/bin/env python3
"""Local synthetic archive attacks. Never reads user documents or uses the network."""
import io, json, os, pathlib, struct, subprocess, sys, tarfile, tempfile, zipfile, time, signal
fz = str(pathlib.Path(sys.argv[1] if len(sys.argv)>1 else 'engine/fz-core/target/release/fz').resolve())
checks = []
def run(*args, password=None):
    return subprocess.run([fz, *map(str,args), '--json'], input=password, capture_output=True, timeout=30)
def require(ok, label):
    assert ok, label
    checks.append(label)
with tempfile.TemporaryDirectory(prefix='osz-security-') as work:
    w=pathlib.Path(work); outside=w/'outside'; outside.mkdir(); (outside/'victim').write_text('ORIGINAL')
    for kind in ('zip','tar','7z'):
        archive=w/f'attack.{kind}'; out=w/f'out-{kind}'; out.mkdir(); (out/'link').symlink_to(outside, target_is_directory=True)
        if kind=='zip':
            with zipfile.ZipFile(archive,'w') as z: z.writestr('link/victim','ATTACK')
        elif kind=='tar':
            with tarfile.open(archive,'w') as t:
                entry=tarfile.TarInfo('link/victim');entry.size=6;t.addfile(entry,io.BytesIO(b'ATTACK'))
        else:
            src=w/'src'; (src/'link').mkdir(parents=True); (src/'link/victim').write_text('ATTACK')
            assert run('create',archive,src,'--no-wrap').returncode==0
        result=run('extract',archive,'-o',out,'--overwrite','overwrite')
        require((outside/'victim').read_text()=='ORIGINAL',kind+': refuses existing symlink ancestor')
        require(result.returncode!=0,kind+': refusal is a failed operation')
    archive=w/'traversal.zip'
    with zipfile.ZipFile(archive,'w') as z:
        z.writestr('../escape','ATTACK'); z.writestr('/absolute','ATTACK'); z.writestr('.hidden','OK')
    out=w/'traversal';result=run('extract',archive,'-o',out)
    require(result.returncode!=0 and not (w/'escape').exists(),'ZIP traversal rejected')
    require((out/'.hidden').read_text()=='OK','dotfile name preserved')
    # A corrupt overwrite must leave the existing original and its mode unchanged.
    archive=w/'corrupt.zip'
    with zipfile.ZipFile(archive,'w',compression=zipfile.ZIP_STORED) as z:z.writestr('victim',b'NEW CONTENT')
    data=bytearray(archive.read_bytes());at=data.index(b'NEW CONTENT');data[at]^=1;archive.write_bytes(data)
    out=w/'corrupt';out.mkdir();victim=out/'victim';victim.write_text('ORIGINAL');victim.chmod(0o600)
    result=run('extract',archive,'-o',out,'--overwrite','overwrite')
    require(result.returncode!=0 and victim.read_text()=='ORIGINAL','corrupt ZIP overwrite preserves original')
    require(victim.stat().st_mode&0o777==0o600,'corrupt ZIP preserves original permissions')
    archive=w/'duplicates.zip'
    with zipfile.ZipFile(archive,'w') as z:z.writestr('same','ONE');z.writestr('SAME','TWO')
    require(run('extract',archive,'-o',w/'duplicates').returncode!=0,'case-colliding ZIP entries refused')
    # Allocate only after checking advertised central-directory size against the archive.
    archive=w/'huge-directory.zip'
    with zipfile.ZipFile(archive,'w') as z:z.writestr('a','a')
    data=bytearray(archive.read_bytes());at=data.rfind(b'PK\x05\x06');struct.pack_into('<I',data,at+12,0x7fffffff);archive.write_bytes(data)
    require(run('list',archive).returncode!=0,'untrusted ZIP allocation bounded before allocation')
    # Archive-created TAR symlink, followed by a child, cannot redirect a write.
    archive=w/'links.tar'
    with tarfile.open(archive,'w') as t:
        e=tarfile.TarInfo('link');e.type=tarfile.SYMTYPE;e.linkname=str(outside);t.addfile(e)
        e=tarfile.TarInfo('link/victim');e.size=6;t.addfile(e,io.BytesIO(b'ATTACK'))
    require(run('extract',archive,'-o',w/'tarlinks','--overwrite','overwrite').returncode!=0,'TAR archive-created external link refused')
    require((outside/'victim').read_text()=='ORIGINAL','TAR link payload cannot alter outside file')
    secret='中文\npassword'.encode();source=w/'password-input';source.write_text('SYNTHETIC')
    archive=w/'encrypted.zip'
    require(run('create',archive,source,'--password-stdin',password=secret).returncode==0,'encrypted ZIP creation over password pipe')
    require(run('test',archive,'--password-stdin',password=secret).returncode==0,'encrypted ZIP test over password pipe')
    require(run('test',archive,'--password-stdin',password=b'wrong').returncode!=0,'wrong password rejected')
    # Refuse silent 7z symlink dereference (ZIP/TAR preserve links instead).
    link=w/'source-link';link.symlink_to(outside/'victim')
    require(run('create',w/'link.7z',link).returncode!=0,'7z never silently dereferences input symlink')
    # Historical predictable .part names must not redirect creation writes.
    for suffix in ('zip','7z','tar'):
        target=w/f'safe.{suffix}';part=pathlib.Path(str(target)+'.part');part.symlink_to(outside/'victim')
        require(run('create',target,source).returncode==0 and (outside/'victim').read_text()=='ORIGINAL',suffix+': temporary output cannot follow predictable .part symlink')
    # Force actual compression: default high-speed stores .bin and can finish before
    # the polling interval on fast runners. Observe a live writer before signalling it.
    src=w/'cancel-input.bin'
    with src.open('wb') as f:
        block=os.urandom(1024*1024)
        for _ in range(32): f.write(block)
    dest=w/'cancelled.7z';dest.write_bytes(b'ORIGINAL ARCHIVE')
    p=subprocess.Popen([fz,'create',str(dest),str(src),'--level','max','--high-speed','off','--json'],stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    try:
        deadline=time.monotonic()+10
        while not list(w.glob('.osz-create-*')) and p.poll() is None and time.monotonic()<deadline: time.sleep(0.01)
        require(p.poll() is None and bool(list(w.glob('.osz-create-*'))),'cancellation reaches an active archive writer')
        p.send_signal(signal.SIGTERM)
        stdout,stderr=p.communicate(timeout=15)
        require(p.returncode==130, f'SIGTERM becomes cooperative engine cancellation (exit={p.returncode}, stdout={stdout[-2000:]!r}, stderr={stderr[-2000:]!r})')
        require(dest.read_bytes()==b'ORIGINAL ARCHIVE' and not list(w.glob('.osz-create-*')),'cancel preserves previous archive and removes temporary output')
    finally:
        if p.poll() is None: p.kill();p.communicate()
print(json.dumps({'passed':len(checks),'checks':checks},ensure_ascii=False,indent=2))
