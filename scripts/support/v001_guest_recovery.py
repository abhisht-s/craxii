"""Real Linux crash barriers over recorded disposable loop images, in private mount namespaces.

Only the serial/size projection is injected for small loop devices. Formatting, receipts, mounts,
filesystem consistency checks and process death are real. This is not EC2 identity evidence.
"""
import copy
import ctypes
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import time
import uuid

HERE=Path(__file__).resolve()
spec=importlib.util.spec_from_file_location('kernel_probes',HERE.with_name('v001_guest_boundary.py'))
k=importlib.util.module_from_spec(spec);spec.loader.exec_module(k)
b=k.b
POOL=Path('/var/lib/craxii-fixture-q01-recovery')/('run-'+str(uuid.uuid4()))

def preflight():
    assert os.geteuid()==0
    assert Path('/etc/craxii-v001-fixture').read_text().strip()=='craxii-v001-guest-boundary-disposable'
    assert Path('/sys/class/dmi/id/product_uuid').read_text().strip()==Path('/etc/craxii-v001-fixture-instance').read_text().strip()=='13131313-1313-4313-8313-131313131314'

def namespace(root):
    assert ctypes.CDLL(None,use_errno=True).unshare(0x20000)==0
    k.run(['/usr/bin/mount','--make-rprivate','/'])
    for path in ['/home/craxii','/srv/craxii/workspaces','/var/lib/craxii','/srv/craxii-data']:
        if os.path.ismount(path):k.run(['/usr/bin/umount',path])
    # Overlay the directory, rather than bind-mounting fstab itself: the production
    # algorithm deliberately uses atomic rename and must be able to exercise that syscall.
    upper=root/'etc-upper';work=root/'etc-work'
    upper.mkdir(mode=0o700,exist_ok=True);work.mkdir(mode=0o700,exist_ok=True)
    k.run(['/usr/bin/mount','-t','overlay','overlay','-o',f'lowerdir=/etc,upperdir={upper},workdir={work}','/etc'])
    fstab=Path('/etc/fstab')
    # The installed disk's managed stanza is irrelevant in this private test namespace.
    fstab.write_text('# disposable crash fixture only\n');fstab.chmod(0o644)

def allocate(name,megabytes=64):
    root=POOL/name;root.mkdir(mode=0o700)
    image=root/'disk.img'
    fd=os.open(image,os.O_RDWR|os.O_CREAT|os.O_EXCL,0o600);os.ftruncate(fd,megabytes*1024*1024);os.close(fd)
    device=k.run(['/usr/sbin/losetup','--find','--show','--nooverlap',str(image)]).decode().strip()
    b.atomic_record(root/'owned.json',{'device':device,'image':str(image),'bytes':megabytes*1024*1024})
    c=copy.deepcopy(k.C)
    c['bootstrap_operation_id']=str(uuid.uuid4());c['filesystem_uuid']=str(uuid.uuid4());c['disk_lineage_id']=str(uuid.uuid4())
    c['marker'].update(birth_operation_id=c['bootstrap_operation_id'],filesystem_uuid=c['filesystem_uuid'],disk_lineage_id=c['disk_lineage_id'])
    c['marker_sha256']=b.digest(c['marker']);binding=dict(c);binding.pop('provenance');c['provenance']['binding_sha256']=b.digest(binding)
    receipts=root/'receipts';receipts.mkdir(mode=0o700)
    return root,image,device,c,receipts

def host_for(c,device,image,root):
    class Host(b.LinuxHost):
        def probe(self):
            backing=k.run(['/usr/sbin/losetup','--noheadings','--output','BACK-FILE',device]).decode().strip()
            assert Path(backing)==image and image.parent==root and root.parent==POOL
            rows=super().probe()
            for row in rows:
                if row['path']==device:row.update(type='disk',serial=c['volume_id'].replace('-',''),model='Amazon Elastic Block Store')
            return rows
    host=Host(c);host.size=image.stat().st_size
    return host

def exercise(name,cut):
    root,image,device,c,receipts=allocate(name,512 if cut=='during-format' else 64)
    counter=root/'format-count'
    def child(interrupt):
        namespace(root)
        # LoopHost independently checks exact recorded backing identity on every selection.
        host=host_for(c,device,image,root)
        formatter=host.format_once
        def format_once(d):
            with counter.open('ab',buffering=0) as f:f.write(b'1');os.fsync(f.fileno())
            if interrupt and cut=='during-format':
                original=b.command
                def command(args,**kw):
                    if args[0]=='/usr/sbin/mkfs.ext4':
                        p=subprocess.Popen(args,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,env=b.ENV,pass_fds=kw['pass_fds'])
                        end=time.monotonic()+5
                        while time.monotonic()<end:
                            if p.poll() is not None:raise AssertionError('formatter completed before kill; no crash proof')
                            try:comm=Path(f'/proc/{p.pid}/comm').read_text().strip()
                            except FileNotFoundError:continue
                            io=Path(f'/proc/{p.pid}/io').read_text()
                            written=int(next(line.split()[1] for line in io.splitlines() if line.startswith('write_bytes:')))
                            if comm=='mkfs.ext4' and written>0:
                                os.kill(p.pid,signal.SIGKILL);assert p.wait()==-signal.SIGKILL
                                os._exit(86)
                            time.sleep(.001)
                        raise AssertionError('formatter never observed')
                    return original(args,**kw)
                b.command=command
            formatter(d)
            if interrupt and cut=='recognized-incomplete':
                k.run(['/usr/sbin/debugfs','-w','-R','freei <2>',d['path']])
                host.verify_filesystem(d)
                _,status=b.command(['/usr/sbin/e2fsck','-f','-n',d['path']],allowed=(0,1,2,4,8,16,32,128))
                assert status!=0,'inconsistent-filesystem injection had no observable effect'
                os._exit(86)
            if interrupt and cut=='after-format':os._exit(86)
        host.format_once=format_once
        atomic=b.atomic_record
        def record(path,material,*args,**kw):
            if interrupt and cut=='before-intent' and material.get('phase')=='format_intended':os._exit(86)
            atomic(path,material,*args,**kw)
            if interrupt and cut=='after-intent' and material.get('phase')=='format_intended':os._exit(86)
            if interrupt and cut=='after-marker' and path.name=='.craxii-data-lineage.json':os._exit(86)
            if interrupt and cut=='after-verification' and material.get('phase')=='filesystem_verified':os._exit(86)
        b.atomic_record=record
        command=b.command
        def after_mount(args,**kw):
            result=command(args,**kw)
            if interrupt and cut=='after-mount' and args[0]=='/usr/bin/mount' and args[-1]=='/srv/craxii-data':os._exit(86)
            if interrupt and cut=='after-bind' and args[:2]==['/usr/bin/mount','--bind'] and args[-1]=='/var/lib/craxii':os._exit(86)
            if interrupt and cut=='after-ownership' and args[0]=='/usr/bin/setfacl':os._exit(86)
            return result
        if cut in ('after-mount','after-bind','after-ownership'):b.command=after_mount
        replace=os.replace
        def after_replace(src,dst,*args,**kw):
            replace(src,dst,*args,**kw)
            if interrupt and cut=='after-fstab' and str(dst)=='/etc/fstab':os._exit(86)
        if cut=='after-fstab':os.replace=after_replace
        try:
            assert b.Bootstrap(host,c,receipts).run()=='layout_verified'
            os._exit(0)
        except b.Hold:
            os._exit(87)
    pid=os.fork()
    if pid==0:child(True)
    _,status=os.waitpid(pid,0);assert os.waitstatus_to_exitcode(status)==86,(name,'barrier not reached')
    before=counter.stat().st_size if counter.exists() else 0
    pid=os.fork()
    if pid==0:child(False)
    _,status=os.waitpid(pid,0);result=os.waitstatus_to_exitcode(status)
    after=counter.stat().st_size if counter.exists() else 0
    if cut=='before-intent':assert result==0 and before==0 and after==1
    elif cut=='after-intent':assert result==87 and before==after==0
    elif cut=='recognized-incomplete':assert result==87 and before==after==1
    elif cut=='during-format':assert result in (0,87) and before==after==1
    else:assert result==0 and before==after==1,(name,result,before,after)
    # Preserve image and receipts. Detach only our exact recorded handle after all namespaces exit.
    assert Path(k.run(['/usr/sbin/losetup','--noheadings','--output','BACK-FILE',device]).decode().strip())==image
    k.run(['/usr/sbin/losetup','--detach',device])
    print('PASS '+name,flush=True)

def adoption():
    root,image,device,c,receipts=allocate('adoption')
    pid=os.fork()
    if pid==0:
        namespace(root)
        host=host_for(c,device,image,root)
        calls=[];original=host.format_once
        def formatter(d):calls.append(1);original(d)
        host.format_once=formatter
        assert b.Bootstrap(host,c,receipts).run()=='layout_verified' and len(calls)==1
        import sqlite3
        db=Path('/var/lib/craxii/db/craxii.sqlite3')
        conn=sqlite3.connect(db);conn.execute('CREATE TABLE fixture(value INTEGER)');conn.execute('INSERT INTO fixture VALUES(13)');conn.commit();conn.close();db.chmod(0o600);os.chown(db,host.worker,host.worker_group)
        def unmount():
            for path in ['/home/craxii','/srv/craxii/workspaces','/var/lib/craxii','/srv/craxii-data']:k.run(['/usr/bin/umount',path])
        unmount()
        c['mode']='adopt_existing';c['bootstrap_operation_id']=str(uuid.uuid4());c['installation_id']=str(uuid.uuid4())
        binding=dict(c);binding.pop('provenance');c['provenance']['binding_sha256']=b.digest(binding)
        k.run([b.GUARD,'validate'],input=b.canonical(c))
        host=host_for(c,device,image,root)
        host.format_once=lambda d:(_ for _ in ()).throw(AssertionError('adoption tried formatting'))
        # The parent keeps the namespace alive when the adopter dies, retaining the ro mount.
        fault=os.fork()
        if fault==0:
            command=b.command
            def cut(args,**kw):
                out=command(args,**kw)
                if args[0]=='/usr/bin/mount' and 'ro,noload,nosuid,nodev' in args:os._exit(86)
                return out
            b.command=cut
            b.Bootstrap(host,c,receipts).run();os._exit(1)
        _,status=os.waitpid(fault,0);assert os.waitstatus_to_exitcode(status)==86
        assert any(m['target']=='/srv/craxii-data' and 'ro' in m['flags'] for m in host.mounts())
        assert b.Bootstrap(host,c,receipts).run()=='layout_verified'
        conn=sqlite3.connect(db);assert conn.execute('SELECT value FROM fixture').fetchone()==(13,);conn.close()
        # Contradictory durable phase is a hold, preserving exact SQLite bytes.
        path=receipts/(c['bootstrap_operation_id']+'.json');record=b.decode(b.read_safe(path));wrong=dict(record,phase='contradictory')
        b.atomic_record(path,wrong,replace=True)
        try:b.Bootstrap(host,c,receipts).run();raise AssertionError('contradictory phase accepted')
        except b.Hold as e:assert e.code=='disk_format_uncertain'
        b.atomic_record(path,record,replace=True)
        # Removing the required worker ACL blocks adoption rather than silently rewriting it.
        primary=Path('/srv/craxii-data/workspaces/primary');k.run(['/usr/bin/setfacl','-x',f'u:{host.worker}',str(primary)])
        try:b.Bootstrap(host,c,receipts).run();raise AssertionError('missing ACL accepted')
        except b.Hold as e:assert e.code=='layout_unavailable'
        k.run(['/usr/bin/setfacl','-m',f'u:{host.worker}:r-x',str(primary)])
        assert b.Bootstrap(host,c,receipts).run()=='layout_verified'
        unmount()
        print('PASS interrupted-readonly-adoption-contradictory-phase-ACL-state-preservation',flush=True)
        import shutil
        clone=root/'restored.img';shutil.copyfile(image,clone);clone.chmod(0o600)
        target=k.run(['/usr/sbin/losetup','--find','--show','--nooverlap',str(clone)]).decode().strip()
        b.atomic_record(root/'restored-owned.json',{'device':target,'image':str(clone),'bytes':clone.stat().st_size})
        restored=copy.deepcopy(c);restored.update(mode='adopt_restored',bootstrap_operation_id=str(uuid.uuid4()),volume_id='vol-00000000000000004',restore={'source_snapshot_id':'snap-00000000000000001','source_volume_id':c['volume_id'],'authorization_id':str(uuid.uuid4()),'disk_lineage_id':c['disk_lineage_id']})
        binding=dict(restored);binding.pop('provenance');restored['provenance']['binding_sha256']=b.digest(binding)
        k.run([b.GUARD,'validate'],input=b.canonical(restored))
        restored_host=host_for(restored,target,clone,root)
        restored_host.format_once=lambda d:(_ for _ in ()).throw(AssertionError('restore tried formatting'))
        try:b.Bootstrap(restored_host,restored,receipts).run();raise AssertionError('simultaneous UUID clone accepted')
        except b.Hold as e:assert e.code=='lineage_mismatch'
        assert Path(k.run(['/usr/sbin/losetup','--noheadings','--output','BACK-FILE',device]).decode().strip())==image
        k.run(['/usr/sbin/losetup','--detach',device])
        assert b.Bootstrap(restored_host,restored,receipts).run()=='layout_verified'
        conn=sqlite3.connect(db);assert conn.execute('SELECT value FROM fixture').fetchone()==(13,);conn.close()
        unmount()
        print('PASS restored-clone-lineage-adoption-duplicate-UUID-refusal-state-preservation',flush=True)
        os._exit(0)
    _,status=os.waitpid(pid,0);assert os.waitstatus_to_exitcode(status)==0
    for record in [root/'owned.json',root/'restored-owned.json']:
        owned=b.decode(b.read_safe(record));path=Path(owned['image']);dev=owned['device']
        assert path.parent==root and root.parent==POOL
        active=subprocess.run(['/usr/sbin/losetup','--noheadings','--output','BACK-FILE',dev],stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,timeout=5)
        if active.returncode in (0,1) and not active.stdout.strip():
            assert not (Path('/sys/class/block')/Path(dev).name/'loop/backing_file').exists()
            continue
        assert active.returncode==0 and Path(active.stdout.decode().strip())==path
        k.run(['/usr/sbin/losetup','--detach',dev])

def main():
    preflight();POOL.parent.mkdir(mode=0o700,exist_ok=True);POOL.mkdir(mode=0o700)
    b.atomic_record(POOL/'ownership.json',{'kind':'craxii-v001-disposable-loop-recovery','uuid':Path('/etc/craxii-v001-fixture-instance').read_text().strip()})
    import sys
    if len(sys.argv)==1 or sys.argv[1]=='crashes':
        for cut in ['before-intent','after-intent','during-format','after-format','after-verification','after-mount']:
            exercise(cut,cut)
    elif sys.argv[1]=='adoption':adoption()
    elif sys.argv[1]=='layout':
        for cut in ['after-marker','after-ownership','after-bind','after-fstab']:exercise(cut,cut)
    elif sys.argv[1]=='incomplete':exercise('recognized-incomplete','recognized-incomplete')
    else:raise AssertionError('unknown fixture subgroup')
if __name__=='__main__':main()
