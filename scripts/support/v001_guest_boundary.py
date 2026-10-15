"""Small kernel probes in an explicitly owned VM; no installed fixture switches.

These prove real loop/ext4, UID, quota, cgroup and nft primitives. They do not certify the installed
x86 guest composition or Rust supervisor; those require additional acceptance with built binaries.
"""
import ctypes
import errno
import importlib.util
import json
import os
from pathlib import Path
import pwd
import socket
import subprocess
import sys
import time

ROOT=Path(__file__).resolve().parents[2]
spec=importlib.util.spec_from_file_location('bootstrap',ROOT/'ops/v0.0.1/bootstrap-data-volume.py')
b=importlib.util.module_from_spec(spec);spec.loader.exec_module(b)
C=json.loads(json.loads((ROOT/'ops/v0.0.1/guest-bootstrap-fixtures.json').read_text())['cases'][0]['json'])
C['bootstrap_operation_id']='0000000c-1111-4111-8111-111111111111'
C['volume_id']='vol-00000000000000003'
C['filesystem_uuid']='0000000d-1111-4111-8111-111111111111'
C['disk_lineage_id']='0000000e-1111-4111-8111-111111111111'
C['marker'].update(birth_operation_id=C['bootstrap_operation_id'],birth_volume_id=C['volume_id'],filesystem_uuid=C['filesystem_uuid'],disk_lineage_id=C['disk_lineage_id'])
C['marker_sha256']=b.digest(C['marker'])
binding=dict(C);binding.pop('provenance');C['provenance']['binding_sha256']=b.digest(binding)
FIX=Path('/var/lib/craxii-fixture-v1')

def run(args,**kw):
    return subprocess.run(args,check=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=30,env=b.ENV,**kw).stdout

def ok(name):
    print('PASS '+name,flush=True)

def as_model(code):
    model=pwd.getpwnam('craxii')
    def drop():os.setgroups([]);os.setgid(model.pw_gid);os.setuid(model.pw_uid)
    return subprocess.run(['/usr/bin/python3','-c',code],preexec_fn=drop,env=b.ENV,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=10).returncode

class LoopHost(b.LinuxHost):
    def __init__(self,c,owned):
        super().__init__(c);self.size=64*1024*1024;self.owned=owned
    def probe(self):
        rows=super().probe()
        for index,(device,image) in enumerate(self.owned):
            # Destructive permission is restricted to our fresh sparse images and recorded handles.
            backing=run(['/usr/sbin/losetup','--noheadings','--output','BACK-FILE',device]).decode().strip()
            b.require(Path(backing)==image and image.parent==FIX,'disk_identity_mismatch')
            for row in rows:
                if row['path']==device:
                    row['type']='disk';row['serial']=self.c['volume_id'].replace('-','') if index==0 else 'volfffffffffffffffff';row['model']='Amazon Elastic Block Store'
        return rows

def storage_probe():
    for name,args in [('craxii-server',['--system','--home-dir','/var/lib/craxii','--shell','/usr/sbin/nologin']),('craxii',['--system','--home-dir','/home/craxii','--shell','/bin/bash'])]:
        try:pwd.getpwnam(name)
        except KeyError:run(['/usr/sbin/useradd','--no-create-home',*args,name])
    for path in ['/srv/craxii-data','/var/lib/craxii','/srv/craxii/workspaces','/home/craxii']:
        p=Path(path);b.require(not p.is_symlink(),'layout_unavailable')
        if os.path.ismount(p): continue
        p.mkdir(parents=True,exist_ok=True);b.require(not list(p.iterdir()),'layout_unavailable');p.chmod(0o755)
    if FIX.exists():
        # An interrupted fixture may resume only recorded exact handles and immutable intent.
        record=b.decode(b.read_safe(FIX/'owned-devices.json'))
        owned=[(x['device'],Path(x['backing'])) for x in record['owned']]
        b.require(len(owned)==2 and all(p.parent==FIX for _,p in owned),'disk_identity_mismatch')
        receipts=FIX/'receipts'
    else:
        FIX.mkdir(mode=0o700)
        receipts=FIX/'receipts';receipts.mkdir(mode=0o700)
        owned=[]
        for number in (1,2):
            image=FIX/f'disk-{number}.img'
            fd=os.open(image,os.O_RDWR|os.O_CREAT|os.O_EXCL,0o600);os.ftruncate(fd,64*1024*1024);os.close(fd)
            device=run(['/usr/sbin/losetup','--find','--show','--nooverlap',str(image)]).decode().strip();owned.append((device,image))
        b.atomic_record(FIX/'owned-devices.json',{'owned':[{'device':d,'backing':str(p)} for d,p in owned]})
    host=LoopHost(C,owned)
    original=host.format_once
    calls=[]
    def formatter(d):
        receipt=b.decode(b.read_safe(receipts/(C['bootstrap_operation_id']+'.json')))
        assert receipt['phase']=='format_intended'
        calls.append(d['maj:min']);original(d)
    host.format_once=formatter
    prior=(receipts/(C['bootstrap_operation_id']+'.json')).exists()
    assert b.Bootstrap(host,C,receipts).run()=='layout_verified';assert len(calls)==(0 if prior else 1)
    ok('owned-loop-zero-scan-format-intent-ext4-three-binds')
    baseline_calls=len(calls)
    b.Bootstrap(host,C,receipts).run();assert len(calls)==baseline_calls
    ok('bootstrap-restart-never-reformats')
    worker=pwd.getpwnam('craxii-server')
    import sqlite3
    db=Path('/var/lib/craxii/db/craxii.sqlite3')
    conn=sqlite3.connect(db);conn.execute('PRAGMA journal_mode=WAL');conn.execute('PRAGMA synchronous=FULL');conn.execute('CREATE TABLE IF NOT EXISTS fixture(value INTEGER)');conn.execute('INSERT INTO fixture SELECT 13 WHERE NOT EXISTS (SELECT 1 FROM fixture WHERE value=13)');conn.commit();conn.close();db.chmod(0o600);os.chown(db,worker.pw_uid,worker.pw_gid)
    secret=FIX/'worker-key-canary';secret.write_text('synthetic-key-canary');secret.chmod(0o600)
    for file in [secret,db]:
        assert as_model(f"import os;os.open({str(file)!r},os.O_RDONLY)")!=0
        assert as_model(f"import os;os.open({str(file)!r},os.O_WRONLY)")!=0
    ok('actual-model-uid-denied-key-and-journal-read-write')
    # Test quota profile is deliberately smaller than the installed contract profile.
    run(['/usr/sbin/setquota','-u','craxii','512','512','32','32','/srv/craxii-data'])
    subprocess.run(['/usr/sbin/quotaon','-u','/srv/craxii-data'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    assert as_model("import os,errno; f=open('/home/craxii/quota-pressure','wb');\ntry:\n for i in range(2048): f.write(b'x'*4096);f.flush()\nexcept OSError as e:\n raise SystemExit(0 if e.errno in (errno.EDQUOT,errno.ENOSPC) else 1)\nraise SystemExit(2)")==0
    conn=sqlite3.connect(db);conn.execute('INSERT INTO fixture SELECT 14 WHERE NOT EXISTS (SELECT 1 FROM fixture WHERE value=14)');conn.commit();conn.close()
    assert as_model("import os,errno\ntry:\n for i in range(128): open('/home/craxii/inode-'+str(i),'w').close()\nexcept OSError as e:\n raise SystemExit(0 if e.errno in (errno.EDQUOT,errno.ENOSPC) else 1)\nraise SystemExit(2)")==0
    ok('quota-block-inode-pressure-trusted-journal-headroom')
    for target in ['/home/craxii','/srv/craxii/workspaces','/var/lib/craxii','/srv/craxii-data']:run(['/usr/bin/umount',target])
    # Reconstruct the same filesystem without a formatter, preserving committed synthetic state.
    adopt=json.loads(json.dumps(C));adopt['mode']='adopt_existing';adopt['bootstrap_operation_id']='00000009-1111-4111-8111-111111111111'
    binding=dict(adopt);binding.pop('provenance');adopt['provenance']['binding_sha256']=b.digest(binding)
    host.c=adopt;b.Bootstrap(host,adopt,receipts).run();assert len(calls)==baseline_calls
    conn=sqlite3.connect(db);assert conn.execute('SELECT value FROM fixture ORDER BY value').fetchall()==[(13,),(14,)];assert conn.execute('PRAGMA integrity_check').fetchone()==('ok',);conn.close()
    ok('cold-ext4-adoption-preserves-committed-synthetic-sqlite')
    # Keep exact owned loop handles until the VM is destroyed; no arbitrary detach/unmount cleanup.
    assert b.decode(b.read_safe(FIX/'owned-devices.json'))=={'owned':[{'device':d,'backing':str(p)} for d,p in owned]}

def cgroup_probe():
    run(['/usr/bin/systemd-run','--unit=craxii-fixture-tools','--property=Delegate=cpu memory pids','--property=DelegateSubgroup=supervisor','/usr/bin/sleep','600'])
    root=Path('/sys/fs/cgroup/system.slice/craxii-fixture-tools.service')
    for i in range(50):
        if (root/'supervisor').exists():break
        time.sleep(.1)
    assert not (root/'cgroup.procs').read_text().strip()
    (root/'cgroup.subtree_control').write_text('+cpu +memory +pids')
    group=root/'owned-tools';group.mkdir()
    for k,v in [('pids.max','16'),('memory.max','67108864'),('memory.swap.max','0'),('cpu.max','10000 100000')]:(group/k).write_text(v)
    model=pwd.getpwnam('craxii')
    def attach():
        (group/'cgroup.procs').write_text('0');os.setgroups([]);os.setgid(model.pw_gid);os.setuid(model.pw_uid)
    p=subprocess.Popen(['/usr/bin/python3','-c',"import os,time;os.setsid();\nif os.fork()==0: time.sleep(60)\nelse: os._exit(0)"],preexec_fn=attach,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,env=b.ENV)
    p.wait(timeout=5);assert 'populated 1' in (group/'cgroup.events').read_text()
    assert as_model(f"open({str(group/'pids.max')!r},'w').write('max')")!=0
    (group/'cgroup.kill').write_text('1')
    for i in range(100):
        if 'populated 0' in (group/'cgroup.events').read_text():break
        time.sleep(.02)
    assert 'populated 0' in (group/'cgroup.events').read_text()
    ok('actual-setsid-descendant-population-kill-and-uid-control-denial')
    forkcode="import os,time,errno\ntry:\n for i in range(64):\n  if os.fork()==0: time.sleep(60);os._exit(0)\nexcept OSError as e: os._exit(0 if e.errno==errno.EAGAIN else 1)\nos._exit(2)"
    p=subprocess.Popen(['/usr/bin/python3','-c',forkcode],preexec_fn=attach,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL);assert p.wait(timeout=5)==0
    assert int((group/'pids.events').read_text().split()[1])>0;(group/'cgroup.kill').write_text('1')
    for i in range(100):
        if 'populated 0' in (group/'cgroup.events').read_text():break
        time.sleep(.02)
    ok('actual-fork-limit-enforced')
    p=subprocess.Popen(['/usr/bin/python3','-c',"x=bytearray(128*1024*1024)"],preexec_fn=attach,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL);assert p.wait(timeout=10)!=0
    assert 'oom_kill 1' in (group/'memory.events').read_text();ok('actual-memory-limit-enforced')
    p=subprocess.Popen(['/usr/bin/python3','-c','while True: pass'],preexec_fn=attach,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL);time.sleep(1)
    (group/'cgroup.kill').write_text('1');p.wait(timeout=5)
    stat=dict(line.split() for line in (group/'cpu.stat').read_text().splitlines());assert int(stat['nr_throttled'])>0;ok('actual-cpu-throttling-enforced')
    run(['/usr/bin/systemctl','stop','craxii-fixture-tools.service'])

def network_probe():
    # Entire fake address space exists in a fresh network namespace; never contact real IMDS.
    run(['/usr/bin/unshare','--net','/usr/bin/python3',str(Path(__file__).resolve()),'network-child'])
    ok('namespaced-fake-ipv4-ipv6-metadata-and-private-network-uid-denial')

def network_child():
    run(['/usr/sbin/ip','link','set','lo','up'])
    for addr in ['169.254.169.254/32','fd00:ec2::254/128','10.1.2.3/32']:run(['/usr/sbin/ip','addr','add',addr,'dev','lo'])
    spec=importlib.util.spec_from_file_location('security',ROOT/'ops/v0.0.1/security-boundary.py');security=importlib.util.module_from_spec(spec);spec.loader.exec_module(security)
    policy=Path('/run/craxii-fixture.nft');policy.write_text(security.nft_material(C));run(['/usr/sbin/nft','-f',str(policy)]);security.network_verify(C)
    for address,family in [('169.254.169.254',socket.AF_INET),('fd00:ec2::254',socket.AF_INET6),('10.1.2.3',socket.AF_INET)]:
        port=5432 if address=='10.1.2.3' else 80
        listener=socket.socket(family,socket.SOCK_STREAM);listener.bind((address,port));listener.listen()
        control=socket.socket(family,socket.SOCK_STREAM);control.settimeout(1);control.connect((address,port));control.close()
        for method in ('GET','PUT'):
            code=f"import socket; s=socket.socket({int(family)},socket.SOCK_STREAM);s.settimeout(.2);s.connect(({address!r},{port}));s.sendall({(method+' /latest/api/token HTTP/1.0'+chr(13)+chr(10)+chr(13)+chr(10)).encode()!r})"
            assert as_model(code)!=0
            assert as_model('import os;os.setsid();'+code)!=0
        if family==socket.AF_INET and port==80:
            assert as_model("import socket;s=socket.socket(socket.AF_INET6,socket.SOCK_STREAM);s.settimeout(.2);s.connect(('::ffff:169.254.169.254',80))")!=0
        listener.close()

if __name__=='__main__':
    assert os.geteuid()==0 and Path('/etc/craxii-v001-fixture').read_text().strip()=='craxii-v001-guest-boundary-disposable' and Path('/sys/class/dmi/id/sys_vendor').read_text().strip()=='QEMU' and Path('/sys/class/dmi/id/product_uuid').read_text().strip()==Path('/etc/craxii-v001-fixture-instance').read_text().strip()
    try:
        if sys.argv[1:]==['network-child']:network_child()
        else:storage_probe();cgroup_probe();network_probe();print('KERNEL_PRIMITIVES_PASS_INSTALLED_COMPOSITION_UNVERIFIED',flush=True)
    except b.Hold as e:
        print('KERNEL_ACCEPTANCE_FAILED '+e.code,flush=True);sys.exit(1)
    except subprocess.CalledProcessError as e:
        print('KERNEL_ACCEPTANCE_FAILED command_exit_'+str(e.returncode),flush=True);sys.exit(1)
    except AssertionError:
        print('KERNEL_ACCEPTANCE_FAILED assertion',flush=True);sys.exit(1)
    except OSError as e:
        print('KERNEL_ACCEPTANCE_FAILED os_errno_'+str(e.errno),flush=True);sys.exit(1)
    except BaseException:
        print('KERNEL_ACCEPTANCE_FAILED unexpected',flush=True);sys.exit(1)
