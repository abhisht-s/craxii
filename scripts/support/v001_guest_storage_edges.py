"""Additional real Linux storage refusals and root-main serialization on owned fixtures only."""
import copy,fcntl,hashlib,importlib.util,json,os,subprocess,sys,time,uuid
from pathlib import Path
spec=importlib.util.spec_from_file_location('recovery',Path(__file__).with_name('v001_guest_recovery.py'))
r=importlib.util.module_from_spec(spec);spec.loader.exec_module(r)
b,k=r.b,r.k

def wait_file(path):
    end=time.monotonic()+10
    while not path.exists():
        assert time.monotonic()<end
        time.sleep(.01)

def concurrency():
    root,image,device,c,receipts=r.allocate('concurrent-root-main')
    child=os.fork()
    if child==0:
        r.namespace(root)
        c['provenance']['kind']='trusted_infrastructure'
        cfg=root/'config.json';cfg.write_bytes(b.canonical(c));cfg.chmod(0o640)
        k.run(['/usr/bin/mount','--bind',str(cfg),str(b.CONFIG)])
        k.run(['/usr/bin/mount','--bind',str(receipts),str(b.INTENTS)])
        host=r.host_for(c,device,image,root)
        blank,formatter=host.blank,host.format_once
        def scan(d):
            (root/'scan-arrived').touch();wait_file(root/'release-scan');blank(d)
        def format_once(d):
            assert b.decode(b.read_safe(receipts/(c['bootstrap_operation_id']+'.json')))['phase']=='format_intended'
            with (root/'formats').open('ab',buffering=0) as f:f.write(b'1');os.fsync(f.fileno())
            formatter(d)
        host.blank=scan;host.format_once=format_once
        b.LinuxHost=lambda config:host
        sys.argv=[b.__file__]
        first=os.fork()
        if first==0:b.main();os._exit(0)
        wait_file(root/'scan-arrived')
        second=os.fork()
        if second==0:
            try:b.main()
            except BlockingIOError:os._exit(89)
            os._exit(1)
        _,status=os.waitpid(second,0);assert os.waitstatus_to_exitcode(status)==89
        assert not (root/'formats').exists()
        (root/'release-scan').touch()
        _,status=os.waitpid(first,0);assert os.waitstatus_to_exitcode(status)==0
        b.main();assert (root/'formats').read_bytes()==b'1'
        print('PASS real-root-main-flock-single-format-restart',flush=True)
        os._exit(0)
    _,status=os.waitpid(child,0);assert os.waitstatus_to_exitcode(status)==0
    detach(device,image)

def detach(device,image):
    assert Path(k.run(['/usr/sbin/losetup','--noheadings','--output','BACK-FILE',device]).decode().strip())==image
    k.run(['/usr/sbin/losetup','--detach',device])

def selection():
    c=b.decode(b.read_safe(b.CONFIG,0o640));host=b.LinuxHost(c);disk=b.select(host,c)
    assert disk['path'].startswith('/dev/nvme') and disk['size']==85899345920
    for key,value in [('volume_id','vol-fffffffffffffffff'),('volume_id','vol-00000000000000002')]:
        wrong=copy.deepcopy(c);wrong[key]=value
        try:b.select(b.LinuxHost(wrong),wrong);raise AssertionError('absent/wrong identity selected')
        except b.Hold as e:assert e.code=='disk_identity_mismatch'
    ancestry=host.root_ancestry()
    roots=[d for d in host.probe() if d['maj:min'] in ancestry]
    assert len(roots)>=2
    for root in roots:
        try:host.check_use(root,True);raise AssertionError('root candidate accepted')
        except b.Hold as e:assert e.code in ('disk_in_use','disk_identity_mismatch')
    root,image,device,unused,receipts=r.allocate('equal-sized-foreign',81920)
    assert image.stat().st_blocks==0
    assert b.select(host,c)['maj:min']==disk['maj:min']
    assert image.stat().st_blocks==0
    detach(device,image)
    # Exercise the actual 80 GiB filesystem's adoption path with the real Rust schema9 DB,
    # while the distribution service is stopped. Only a separate root receipt is created.
    files=[Path('/var/lib/craxii/db/craxii.sqlite3'),Path('/var/lib/craxii-worker/identity/worker.pk8'),Path('/var/lib/craxii-worker/identity/identity.json')]
    before=[hashlib.file_digest(p.open('rb'),'sha256').hexdigest() for p in files]
    adopted=copy.deepcopy(c);adopted.update(mode='adopt_existing',bootstrap_operation_id=str(uuid.uuid4()))
    binding=dict(adopted);binding.pop('provenance');adopted['provenance']['binding_sha256']=b.digest(binding)
    k.run([b.GUARD,'validate'],input=b.canonical(adopted))
    actual=b.LinuxHost(adopted)
    actual.format_once=lambda d:(_ for _ in ()).throw(AssertionError('actual adoption formatted'))
    assert b.Bootstrap(actual,adopted,receipts).run()=='layout_verified'
    assert before==[hashlib.file_digest(p.open('rb'),'sha256').hexdigest() for p in files]
    print('PASS actual-NVMe-exact-wrong-missing-root-extra-equal-size-adoption-SQLite9-key-preservation',flush=True)

def failures():
    root,image,device,c,receipts=r.allocate('conflicting-filesystem')
    host=r.host_for(c,device,image,root)
    assert b.select(host,c)['path']==device
    k.run(['/usr/sbin/mkfs.vfat','-I',device])
    before=hashlib.file_digest(image.open('rb'),'sha256').hexdigest()
    host.format_once=lambda d:(_ for _ in ()).throw(AssertionError('conflicting disk formatted'))
    for mode in ['initialize_blank','adopt_existing']:
        c['mode']=mode
        binding=dict(c);binding.pop('provenance');c['provenance']['binding_sha256']=b.digest(binding)
        k.run([b.GUARD,'validate'],input=b.canonical(c))
        try:b.Bootstrap(host,c,receipts).run();raise AssertionError('foreign filesystem accepted')
        except b.Hold as e:assert e.code in ('disk_not_blank','lineage_mismatch')
    assert before==hashlib.file_digest(image.open('rb'),'sha256').hexdigest() and not list(receipts.iterdir())
    detach(device,image)
    root,image,device,c,receipts=r.allocate('actual-short-read')
    name='craxii-q01-error-'+uuid.uuid4().hex
    k.run(['/usr/sbin/modprobe','dm_mod'])
    table=f'0 2048 linear {device} 0\n2048 {image.stat().st_size//512-2048} error'
    k.run(['/usr/sbin/dmsetup','create',name,'--readonly','--table',table])
    b.atomic_record(root/'owned-mapper.json',{'name':name,'backing':device,'table':table})
    mapped=Path('/dev/mapper')/name;info=mapped.stat();wanted=f'{os.major(info.st_rdev)}:{os.minor(info.st_rdev)}'
    class FaultHost(b.LinuxHost):
        def probe(self):
            assert Path(k.run(['/usr/sbin/losetup','--noheadings','--output','BACK-FILE',device]).decode().strip())==image
            actual=k.run(['/usr/sbin/dmsetup','table',name]).decode().splitlines()
            assert len(actual)==2 and actual[1].split()[-1]=='error'
            rows=super().probe()
            for row in rows:
                if row['maj:min']==wanted:row.update(type='disk',serial=c['volume_id'].replace('-',''),model='Amazon Elastic Block Store',path=str(mapped.resolve()))
            return rows
    host=FaultHost(c);host.size=image.stat().st_size
    host.format_once=lambda d:(_ for _ in ()).throw(AssertionError('unreadable disk formatted'))
    try:
        try:b.Bootstrap(host,c,receipts).run();raise AssertionError('kernel read error accepted')
        except b.Hold as e:assert e.code=='disk_probe_unavailable'
        except OSError as e:assert e.errno==5
        assert not list(receipts.iterdir())
    finally:k.run(['/usr/sbin/dmsetup','remove',name])
    detach(device,image)
    print('PASS real-conflicting-filesystem-and-kernel-read-error-no-intent-no-format',flush=True)

def mapper():
    root,image,device,c,receipts=r.allocate('mapper-root-ancestry')
    name='craxii-q01-'+uuid.uuid4().hex
    child=os.fork()
    if child==0:
        r.namespace(root)
        host=r.host_for(c,device,image,root);assert b.Bootstrap(host,c,receipts).run()=='layout_verified'
        for entry in ['proc','sys']:(Path('/srv/craxii-data')/entry).mkdir()
        for path in ['/home/craxii','/srv/craxii/workspaces','/var/lib/craxii','/srv/craxii-data']:k.run(['/usr/bin/umount',path])
        sectors=image.stat().st_size//512
        k.run(['/usr/sbin/dmsetup','create',name,'--readonly','--table',f'0 {sectors} linear {device} 0'])
        b.atomic_record(root/'owned-mapper.json',{'name':name,'backing':device,'sectors':sectors})
        mount=root/'root';mount.mkdir()
        k.run(['/usr/bin/mount','-t','ext4','-o','ro,noload',f'/dev/mapper/{name}',str(mount)])
        k.run(['/usr/bin/mount','--bind','/sys',str(mount/'sys')])
        k.run(['/usr/bin/mount','-t','proc','proc',str(mount/'proc')])
        original=next(d['maj:min'] for d in host.probe() if d['path']==device)
        mapped_info=os.stat(f'/dev/mapper/{name}');mapped_identity=f'{os.major(mapped_info.st_rdev)}:{os.minor(mapped_info.st_rdev)}'
        os.chroot(mount);os.chdir('/')
        ancestry=host.root_ancestry()
        assert original in ancestry and mapped_identity in ancestry
        print('PASS actual-mapper-root-ancestry-includes-disposable-backing-loop',flush=True)
        os._exit(0)
    _,status=os.waitpid(child,0);assert os.waitstatus_to_exitcode(status)==0
    table=k.run(['/usr/sbin/dmsetup','table',name]).decode().split()
    dev=os.stat(device).st_rdev
    assert table==['0',str(image.stat().st_size//512),'linear',f'{os.major(dev)}:{os.minor(dev)}','0']
    k.run(['/usr/sbin/dmsetup','remove',name]);detach(device,image)

def main():
    r.preflight()
    assert subprocess.run(['systemctl','is-active','--quiet','craxii-distribution.service']).returncode!=0
    r.POOL.parent.mkdir(mode=0o700,exist_ok=True);r.POOL.mkdir(mode=0o700)
    b.atomic_record(r.POOL/'ownership.json',{'kind':'owned-storage-edges','fixture':'13131313-1313-4313-8313-131313131314'})
    group=sys.argv[1] if len(sys.argv)>1 else 'all'
    if group in ('all','selection'):selection();failures()
    if group in ('all','concurrency'):concurrency()
    if group in ('all','mapper'):mapper()
if __name__=='__main__':main()
