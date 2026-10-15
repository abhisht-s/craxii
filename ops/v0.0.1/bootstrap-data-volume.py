#!/usr/bin/python3
"""Root-only exact-device initializer. The installed entrypoint has no device/fixture override.

Root receipts record installation safety, never execution or cloud authority. Probe failures hold.
"""
import contextlib
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tempfile
import time

CONFIG = Path('/etc/craxii/guest-bootstrap.json')
INTENTS = Path('/var/lib/craxii-worker/bootstrap')
DATA = Path('/srv/craxii-data')
GUARD = '/opt/craxii/current/craxii-guest-guard'
MAX = 16384
ENV = {'PATH': '/usr/sbin:/usr/bin:/sbin:/bin', 'LANG': 'C', 'LC_ALL': 'C'}
CODES = {'guest_config_invalid', 'disk_identity_mismatch', 'disk_not_blank', 'disk_in_use', 'lineage_mismatch', 'disk_probe_unavailable', 'disk_format_uncertain', 'layout_unavailable', 'isolation_unavailable'}

class Hold(Exception):
    def __init__(self, code):
        self.code = code if code in CODES else 'layout_unavailable'

def require(condition, code):
    if not condition:
        raise Hold(code)

def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode('utf-8')

def digest(value):
    return hashlib.sha256(canonical(value)).hexdigest()

def decode(bytes_, limit=MAX):
    require(len(bytes_) <= limit and not bytes_.startswith(b'\xef\xbb\xbf'), 'guest_config_invalid')
    def pairs(items):
        result = {}
        for key, value in items:
            require(key not in result, 'guest_config_invalid')
            result[key] = value
        return result
    try:
        value = json.loads(bytes_.decode('utf-8'), object_pairs_hook=pairs)
    except (ValueError, UnicodeError, RecursionError):
        raise Hold('guest_config_invalid') from None
    require(isinstance(value, dict), 'guest_config_invalid')
    return value

def safe_open(path, mode, owner=0, directory=False):
    """Pin all ancestors with openat/O_NOFOLLOW; no group/other-writable ancestor."""
    path = Path(path)
    require(path.is_absolute() and '..' not in path.parts, 'layout_unavailable')
    fd = os.open('/', os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    try:
        for index, name in enumerate(path.parts[1:]):
            last = index == len(path.parts) - 2
            flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC | os.O_NONBLOCK
            if not last or directory:
                flags |= os.O_DIRECTORY
            next_fd = os.open(name, flags, dir_fd=fd)
            os.close(fd)
            fd = next_fd
            info = os.fstat(fd)
            if last:
                require(info.st_uid == owner and stat.S_IMODE(info.st_mode) == mode, 'layout_unavailable')
                require(stat.S_ISDIR(info.st_mode) if directory else stat.S_ISREG(info.st_mode) and info.st_nlink == 1, 'layout_unavailable')
            else:
                require(stat.S_ISDIR(info.st_mode) and info.st_uid in (0, owner) and info.st_mode & 0o022 == 0, 'layout_unavailable')
        return fd
    except BaseException:
        os.close(fd)
        raise

def read_safe(path, mode=0o600, owner=0, limit=MAX):
    fd = safe_open(path, mode, owner)
    try:
        bytes_ = os.read(fd, limit + 1)
        require(len(bytes_) <= limit, 'layout_unavailable')
        return bytes_
    finally:
        os.close(fd)

def atomic_record(path, material, mode=0o600, gid=0, replace=False):
    """Fsync bytes then atomically publish, then fsync parent. Failure never permits mkfs."""
    path = Path(path)
    parent = safe_open(path.parent, stat.S_IMODE(path.parent.stat().st_mode), directory=True)
    name = '.intent-' + os.urandom(12).hex()
    fd = None
    try:
        fd = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, mode, dir_fd=parent)
        bytes_ = canonical(material)
        require(len(bytes_) <= MAX, 'layout_unavailable')
        os.fchmod(fd, mode)
        os.fchown(fd, 0, gid)
        with os.fdopen(fd, 'wb', closefd=True) as file:
            fd = None
            file.write(bytes_)
            file.flush()
            os.fsync(file.fileno())
        if replace:
            read_safe(path, mode)
            os.rename(name, path.name, src_dir_fd=parent, dst_dir_fd=parent)
        else:
            # linkat is no-replace publication; a competing existing record is never overwritten.
            os.link(name, path.name, src_dir_fd=parent, dst_dir_fd=parent, follow_symlinks=False)
            os.unlink(name, dir_fd=parent)
        os.fsync(parent)
    finally:
        if fd is not None:
            os.close(fd)
        with contextlib.suppress(FileNotFoundError):
            os.unlink(name, dir_fd=parent)
        os.close(parent)

def output_limit():
    import resource
    resource.setrlimit(resource.RLIMIT_FSIZE,(1048576,1048576))
    resource.setrlimit(resource.RLIMIT_CORE,(0,0))

def command(argv, timeout=5, stdin=None, allowed=(0,), pass_fds=()):
    """Fixed argv only; bounded captures on an unlinked file, no shell/error/output logging."""
    with tempfile.TemporaryFile(dir='/run') as out:
        process = subprocess.Popen(argv, stdin=subprocess.PIPE if stdin is not None else subprocess.DEVNULL,
                                   stdout=out, stderr=subprocess.DEVNULL, env=ENV, start_new_session=True, pass_fds=pass_fds,
                                   preexec_fn=output_limit)
        try:
            process.communicate(stdin, timeout=timeout)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, 9)
            process.wait()
            raise Hold('disk_probe_unavailable') from None
        require(process.returncode in allowed and out.tell() <= 1048576, 'disk_probe_unavailable')
        out.seek(0)
        return out.read(1048577), process.returncode

class LinuxHost:
    """Installed probes. Test injection exists only through direct unittest construction."""
    def __init__(self, config):
        self.c = config
        self.size = 85899345920
        self.data = DATA
        import pwd, grp
        self.worker = pwd.getpwnam('craxii-server').pw_uid
        self.worker_group = grp.getgrnam('craxii-server').gr_gid
        self.model = pwd.getpwnam('craxii').pw_uid
        self.model_group = grp.getgrnam('craxii').gr_gid
        require(0 not in (self.worker, self.model, self.worker_group, self.model_group) and self.worker != self.model and self.worker_group != self.model_group, 'isolation_unavailable')
    def probe(self):
        bytes_, _ = command(['/usr/bin/lsblk', '--bytes', '--json', '--output', 'KNAME,PATH,TYPE,SIZE,MAJ:MIN,FSTYPE,UUID'])
        tree = decode(bytes_, 1048576).get('blockdevices')
        require(isinstance(tree, list), 'disk_probe_unavailable')
        flat = []
        def visit(rows):
            for row in rows:
                require(isinstance(row, dict) and len(flat) < 4096, 'disk_probe_unavailable')
                flat.append(row)
                visit(row.get('children', []))
        visit(tree)
        for d in flat:
            sysfs = Path('/sys/class/block') / d['kname']
            d['serial'] = ''
            if (sysfs / 'device/serial').exists():
                d['serial'] = (sysfs / 'device/serial').read_text().strip()
                d['model'] = (sysfs / 'device/model').read_text().strip()
            d['holders'] = list((sysfs / 'holders').iterdir())
            d['partition'] = (sysfs / 'partition').exists()
        self.devices = flat
        return flat
    def root_ancestry(self):
        mounts = self.mounts()
        root = [m for m in mounts if m['target'] == '/']
        require(len(root) == 1, 'disk_probe_unavailable')
        seen = set()
        def visit(path):
            path = path.resolve(strict=True)
            dev = path / 'dev'
            require(dev.exists(), 'disk_probe_unavailable')
            identity = dev.read_text().strip()
            if identity in seen:
                return
            seen.add(identity)
            if (path / 'partition').exists():
                visit(path.parent)
            slaves=path/'slaves'
            if slaves.exists():
                for slave in slaves.iterdir(): visit(slave)
            else: require((path/'partition').exists(),'disk_probe_unavailable')
        visit(Path('/sys/dev/block') / root[0]['device'])
        return seen
    def mounts(self):
        bytes_ = Path('/proc/self/mountinfo').read_bytes()
        require(len(bytes_) <= 1048576, 'disk_probe_unavailable')
        rows = []
        for line in bytes_.decode().splitlines():
            h, t = line.split(' - ', 1)
            a, b = h.split(), t.split()
            require(len(rows) < 4096 and len(a) >= 6 and len(b) == 3 and '\\' not in a[3] + a[4], 'disk_probe_unavailable')
            rows.append(dict(device=a[2], root=a[3], target=a[4], flags=a[5].split(','), super_flags=b[2].split(','), fs=b[0]))
        return rows
    def check_use(self, d, formatting=False):
        require(d['type'] == 'disk' and not d['partition'] and not d.get('children') and not d['holders'], 'disk_in_use')
        require(d['maj:min'] not in self.root_ancestry(), 'disk_identity_mismatch')
        mounts = [m for m in self.mounts() if m['device'] == d['maj:min']]
        expected = {str(self.data): '/', '/var/lib/craxii': '/state', '/srv/craxii/workspaces': '/workspaces', '/home/craxii': '/home/craxii'}
        require(not mounts if formatting else all(m['target'] in expected and m['root'] == expected[m['target']] and m['fs'] == 'ext4' for m in mounts), 'disk_in_use')
        swaps = Path('/proc/swaps').read_text().splitlines()[1:]
        require(len(swaps) <= 4096, 'disk_probe_unavailable')
        for swap in swaps:
            path = Path(swap.split()[0])
            info = path.stat()
            require((info.st_rdev if stat.S_ISBLK(info.st_mode) else info.st_dev) != os.makedev(*map(int, d['maj:min'].split(':'))), 'disk_in_use')
    def blank(self, d):
        self.check_use(d, True)
        signatures, _ = command(['/usr/sbin/wipefs', '--no-act', '--json', d['path']])
        require(decode(signatures, 1048576).get('signatures') == [], 'disk_not_blank')
        fd = os.open(d['path'], os.O_RDONLY | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC)
        try:
            require(os.fstat(fd).st_rdev == os.makedev(*map(int,d['maj:min'].split(':'))), 'disk_identity_mismatch')
            left = self.size
            end = time.monotonic() + 1800
            while left:
                require(time.monotonic() < end, 'disk_probe_unavailable')
                chunk = os.read(fd, min(left, 1048576))
                require(chunk, 'disk_probe_unavailable')
                require(chunk == bytes(len(chunk)), 'disk_not_blank')
                left -= len(chunk)
        finally:
            os.close(fd)
    def format_once(self, d):
        try:
            require(select(self,self.c)==d,'disk_identity_mismatch')
            self.check_use(d,True)
            # Keep the exact block descriptor pinned. mke2fs acquires its own exclusive claim;
            # a parent O_EXCL claim would cause its mandatory in-use check to refuse.
            fd=os.open(d['path'],os.O_RDWR|os.O_NOFOLLOW|os.O_CLOEXEC)
            try:
                info=os.fstat(fd)
                require(stat.S_ISBLK(info.st_mode) and info.st_rdev==os.makedev(*map(int,d['maj:min'].split(':'))),'disk_identity_mismatch')
                command(['/usr/sbin/mkfs.ext4', '-q', '-U', self.c['filesystem_uuid'], '-O', 'quota', '-E', 'lazy_itable_init=0,lazy_journal_init=0', f'/proc/self/fd/{fd}'], timeout=120,pass_fds=(fd,))
            finally: os.close(fd)
        except BaseException:
            raise Hold('disk_format_uncertain') from None
    def verify_filesystem(self, d):
        result, status = command(['/usr/sbin/blkid', '-p', '-o', 'export', d['path']], allowed=(0,2))
        if status != 0:
            raise Hold('disk_format_uncertain')
        fields = dict(line.split('=',1) for line in result.decode().splitlines() if '=' in line)
        require(fields.get('TYPE') == 'ext4' and fields.get('UUID') == self.c['filesystem_uuid'] and 'PTTYPE' not in fields, 'lineage_mismatch')
    def verify_complete_format(self, d):
        # A superblock recognized by blkid can precede completion of mke2fs. An uncertain
        # formatter may resume only after a full, non-mutating offline consistency check.
        require(not any(m['device']==d['maj:min'] for m in self.mounts()),'disk_format_uncertain')
        try:
            command(['/usr/sbin/e2fsck','-f','-n',d['path']],timeout=120)
        except BaseException:
            raise Hold('disk_format_uncertain') from None
    def layout(self, d, resume_new):
        mounts = self.mounts()
        target = [m for m in mounts if m['target'] == str(self.data)]
        require(len(target) <= 1, 'layout_unavailable')
        if not target:
            self.empty_target(self.data)
            command(['/usr/bin/mount', '-t', 'ext4', '-o', 'rw,nosuid,nodev,usrquota' if resume_new else 'ro,noload,nosuid,nodev', d['path'], str(self.data)])
        # An interrupted adoption may have left our exact mount read-only. Reinspect its
        # immutable marker/layout before making it writable, just as on a cold adoption.
        readonly=not resume_new and (not target or 'ro' in target[0]['flags'])
        self.verify_data_mount(d,readonly=readonly)
        marker_path = self.data / '.craxii-data-lineage.json'
        if marker_path.exists() or marker_path.is_symlink():
            require(decode(read_safe(marker_path, 0o440, limit=4096),4096) == self.c['marker'], 'lineage_mismatch')
        else:
            require(resume_new, 'lineage_mismatch')
            atomic_record(marker_path, self.c['marker'], 0o440, self.worker_group)
        layout = [('state',self.worker,self.worker_group,0o700),('workspaces',0,0,0o755),('home',0,0,0o755),('home/craxii',self.model,self.model_group,0o700),('workspaces/primary',self.model,self.model_group,0o750),('state/db',self.worker,self.worker_group,0o700),('state/artifacts',self.worker,self.worker_group,0o700),('state/locks',self.worker,self.worker_group,0o700)]
        for relative, uid, gid, mode in layout:
            p = self.data / relative
            if not p.exists() and not p.is_symlink():
                require(resume_new, 'layout_unavailable')
                p.mkdir(mode=mode)
                os.chown(p, uid, gid)
                os.chmod(p, mode)
            info = p.lstat()
            require(stat.S_ISDIR(info.st_mode) and info.st_uid == uid and info.st_gid == gid and stat.S_IMODE(info.st_mode) == mode, 'layout_unavailable')
        if not resume_new:
            require((self.data / 'state/db/craxii.sqlite3').is_file(), 'layout_unavailable')
        primary = self.data / 'workspaces/primary'
        if resume_new:
            command(['/usr/bin/setfacl','-m',f'u:{self.worker}:r-x,d:u::rwx,d:u:{self.worker}:r-x,d:g::---,d:m::r-x,d:o::---',str(primary)])
        if readonly:
            command(['/usr/bin/mount','-o','remount,rw,nosuid,nodev,usrquota',str(self.data)])
            self.verify_data_mount(d)
        # Never recursive chown: existing files and installation identity stay intact.
        for source,target in [('state','/var/lib/craxii'),('workspaces','/srv/craxii/workspaces'),('home/craxii','/home/craxii')]:
            mounted = [m for m in self.mounts() if m['target'] == target]
            require(len(mounted) <= 1, 'layout_unavailable')
            if not mounted:
                self.empty_target(Path(target))
                command(['/usr/bin/mount','--bind',str(self.data / source),target])
                command(['/usr/bin/mount','-o','remount,bind,rw,nosuid,nodev',target])
        self.verify_layout(d)
        self.fstab()
    def empty_target(self,p):
        fd = safe_open(p,0o755,directory=True)
        try:
            require(not os.listdir(fd), 'layout_unavailable')
        finally:
            os.close(fd)
    def verify_data_mount(self,d,readonly=False):
        matches = [m for m in self.mounts() if m['target'] == str(self.data)]
        require(len(matches) == 1 and matches[0]['device'] == d['maj:min'] and matches[0]['root'] == '/' and matches[0]['fs'] == 'ext4' and {('ro' if readonly else 'rw'),'nosuid','nodev'} <= set(matches[0]['flags']), 'layout_unavailable')
        info = self.data.lstat()
        # First fresh ext4 root is root:root0755; only new lineage may change this root metadata.
        if not (self.data / '.craxii-data-lineage.json').exists() and self.c['mode'] == 'initialize_blank':
            require(info.st_uid == 0 and info.st_mode & 0o022 == 0, 'layout_unavailable')
            os.chown(self.data,0,self.worker_group)
            os.chmod(self.data,0o750)
        else:
            require(info.st_uid == 0 and info.st_gid == self.worker_group and stat.S_IMODE(info.st_mode) == 0o750, 'layout_unavailable')
    def verify_layout(self,d):
        self.verify_data_mount(d)
        for target, root in [('/var/lib/craxii','/state'),('/srv/craxii/workspaces','/workspaces'),('/home/craxii','/home/craxii')]:
            matches = [m for m in self.mounts() if m['target'] == target]
            require(len(matches)==1 and matches[0]['root']==root and matches[0]['device']==d['maj:min'] and matches[0]['fs']=='ext4' and {'rw','nosuid','nodev'} <= set(matches[0]['flags']), 'layout_unavailable')
        acl,_=command(['/usr/bin/getfacl','--numeric','--omit-header',str(self.data/'workspaces/primary')])
        expected={'user::rwx',f'user:{self.worker}:r-x','group::r-x','mask::r-x','other::---','default:user::rwx',f'default:user:{self.worker}:r-x','default:group::---','default:mask::r-x','default:other::---'}
        require(set(acl.decode().split())==expected,'layout_unavailable')
    def fstab(self):
        path = Path('/etc/fstab')
        original = read_safe(path, 0o644)
        text = original.decode('utf-8')
        begin,end = '# BEGIN CRAXII GUEST V1\n','# END CRAXII GUEST V1\n'
        require(text.count(begin) <= 1 and text.count(end) <= 1 and text.count(begin)==text.count(end), 'layout_unavailable')
        outside = text
        if begin in text:
            before, rest = text.split(begin)
            _, after = rest.split(end)
            outside = before + after
        targets = {'/srv/craxii-data','/var/lib/craxii','/srv/craxii/workspaces','/home/craxii'}
        for line in outside.splitlines():
            fields = line.split()
            require(not fields or fields[0].startswith('#') or len(fields)<2 or fields[1] not in targets, 'layout_unavailable')
        block = begin + f"UUID={self.c['filesystem_uuid']} /srv/craxii-data ext4 noauto,x-systemd.requires=craxii-guest-bootstrap.service,rw,nosuid,nodev,usrquota 0 2\n" + '/srv/craxii-data/state /var/lib/craxii none noauto,x-systemd.requires=craxii-guest-bootstrap.service,bind,nosuid,nodev 0 0\n/srv/craxii-data/workspaces /srv/craxii/workspaces none noauto,x-systemd.requires=craxii-guest-bootstrap.service,bind,nosuid,nodev 0 0\n/srv/craxii-data/home/craxii /home/craxii none noauto,x-systemd.requires=craxii-guest-bootstrap.service,bind,nosuid,nodev 0 0\n' + end
        if begin in text:
            require(text[text.index(begin):text.index(end)+len(end)] == block, 'layout_unavailable')
            return
        new = (outside.rstrip('\n')+'\n'+block).encode()
        # Root-owned exact managed replacement; no generic fstab editor.
        fd,name = tempfile.mkstemp(prefix='.craxii-fstab-', dir='/etc')
        try:
            with os.fdopen(fd,'wb') as file:
                os.fchmod(file.fileno(),0o644)
                file.write(new); file.flush(); os.fsync(file.fileno())
            require(read_safe(path,0o644)==original,'layout_unavailable')
            os.replace(name,path)
            parent=os.open('/etc',os.O_RDONLY|os.O_DIRECTORY)
            try: os.fsync(parent)
            finally: os.close(parent)
        finally:
            with contextlib.suppress(FileNotFoundError): os.unlink(name)

def select(host,c):
    devices = host.probe()
    expected = c['volume_id']
    def identity(d):
        serial=d.get('serial','')
        if re.fullmatch(r'vol[0-9a-f]{17}',serial): serial='vol-'+serial[3:]
        return serial
    candidates = [d for d in devices if identity(d)==expected]
    require(len(candidates)==1,'disk_identity_mismatch')
    d=candidates[0]
    require(d.get('model')=='Amazon Elastic Block Store' and d.get('size')==host.size and d.get('type')=='disk','disk_identity_mismatch')
    clones = [x for x in devices if x.get('uuid')==c['filesystem_uuid']]
    require(len(clones)<=1 and (not clones or clones[0]['maj:min']==d['maj:min']),'lineage_mismatch')
    host.check_use(d)
    return d

class Bootstrap:
    def __init__(self,host,config,receipts=INTENTS):
        self.host,self.c,self.receipts=host,config,Path(receipts)
    def run(self):
        d=select(self.host,self.c)
        path=self.receipts/(self.c['bootstrap_operation_id']+'.json')
        identity={'version':1,'config_sha256':digest(self.c),'operation_id':self.c['bootstrap_operation_id'],'customer_id':self.c['customer_id'],'logical_agent_id':self.c['logical_agent_id'],'installation_id':self.c['installation_id'],'generation':self.c['generation'],'volume_id':self.c['volume_id'],'device':d['maj:min'],'filesystem_uuid':self.c['filesystem_uuid']}
        old=None
        if path.exists() or path.is_symlink():
            old=decode(read_safe(path))
            require({k:v for k,v in old.items() if k!='phase'}==identity,'lineage_mismatch')
            require(old.get('phase') in ('format_intended','filesystem_verified','layout_verified'),'disk_format_uncertain')
        # Adoption has no formatting branch, with or without a receipt.
        fresh=self.c['mode']=='initialize_blank' and old is None
        if fresh:
            self.host.blank(d)
            # Recheck every identity/use predicate after the scan, immediately before intent+format.
            require(select(self.host,self.c)==d,'disk_identity_mismatch')
            self.host.check_use(d,True)
            atomic_record(path,{**identity,'phase':'format_intended'})
            self.host.format_once(d)
        self.host.verify_filesystem(d)
        if fresh or (old is not None and old['phase']=='format_intended'):
            self.host.verify_complete_format(d)
        # No second mkfs after ANY durable intent, including an all-zero disk.
        if fresh or (old is not None and old['phase']!='layout_verified'):
            atomic_record(path,{**identity,'phase':'filesystem_verified'},replace=True)
        self.host.layout(d,self.c['mode']=='initialize_blank')
        if old is None and not fresh:
            atomic_record(path,{**identity,'phase':'filesystem_verified'})
        self.host.verify_layout(d)
        atomic_record(path,{**identity,'phase':'layout_verified'},replace=True)
        return 'layout_verified'

def main():
    require(os.geteuid()==0 and sys.argv[1:]==[], 'isolation_unavailable')
    require(os.uname().machine=='x86_64', 'isolation_unavailable')
    release=Path('/etc/os-release').read_text()
    require('ID=ubuntu\n' in release and 'VERSION_ID="24.04"\n' in release,'isolation_unavailable')
    for unit in ['craxii-distribution.service','craxii-server.service']:
        _,status=command(['/usr/bin/systemctl','is-active','--quiet',unit],allowed=(0,3,4))
        require(status!=0,'disk_in_use')
    bytes_=read_safe(CONFIG,0o640)
    command([GUARD,'validate'],stdin=bytes_)
    c=decode(bytes_)
    require(c['provenance']['kind']=='trusted_infrastructure','isolation_unavailable')
    lockdir=safe_open(INTENTS,0o700,directory=True)
    try:
        fd=os.open('bootstrap.lock',os.O_RDWR|os.O_CREAT|os.O_NOFOLLOW|os.O_CLOEXEC,0o600,dir_fd=lockdir)
        try:
            info=os.fstat(fd)
            require(stat.S_ISREG(info.st_mode) and info.st_uid==0 and info.st_nlink==1 and stat.S_IMODE(info.st_mode)==0o600,'isolation_unavailable')
            fcntl.flock(fd,fcntl.LOCK_EX|fcntl.LOCK_NB)
            # One active operation: unfinished different receipts require reconciliation.
            for p in INTENTS.glob('*.json'):
                r=decode(read_safe(p))
                require(p.name==c['bootstrap_operation_id']+'.json' or r.get('phase')=='layout_verified','lineage_mismatch')
            Bootstrap(LinuxHost(c),c).run()
        finally: os.close(fd)
    finally: os.close(lockdir)

if __name__=='__main__':
    try:
        main()
    except Hold as e:
        print(e.code,file=sys.stderr); sys.exit(1)
    except BaseException:
        print('layout_unavailable',file=sys.stderr); sys.exit(1)
