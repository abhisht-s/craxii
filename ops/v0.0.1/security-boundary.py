#!/usr/bin/python3
"""Typed installation/verification helper. No command, UID or firewall text passthrough."""
import importlib.util
import ctypes
import grp
import ipaddress
import json
import os
from pathlib import Path
import pwd
import stat
import sys
import uuid

spec=importlib.util.spec_from_file_location('bootstrap',Path(__file__).with_name('bootstrap-data-volume.py'))
b=importlib.util.module_from_spec(spec); spec.loader.exec_module(b)
ROOT=Path('/sys/fs/cgroup/system.slice/craxii-distribution.service')

def protect_system_logs(harden=False):
    # Ubuntu's /var/log is root:syslog0775 and cloud-init.log may belong to syslog.
    # Pin this one trusted OS directory, reject extended ACLs/other writers, and never
    # relax the strict ancestor walk used for guest configuration, keys or journals.
    try: logger=pwd.getpwnam('syslog')
    except KeyError: logger=None
    var=b.safe_open('/var',0o755,directory=True)
    log=None
    try:
        log=os.open('log',os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW|os.O_CLOEXEC,dir_fd=var)
        info=os.fstat(log);mode=stat.S_IMODE(info.st_mode)
        b.require(info.st_uid==0 and mode in (0o755,0o775),'isolation_unavailable')
        b.require(mode==0o755 or (logger is not None and info.st_gid==logger.pw_gid and logger.pw_gid!=account('craxii').pw_gid),'isolation_unavailable')
        acl,_=b.command(['/usr/bin/getfacl','--numeric','--omit-header','/var/log'])
        b.require(set(acl.decode().split())=={'user::rwx','group::'+('rwx' if mode==0o775 else 'r-x'),'other::r-x'},'isolation_unavailable')
        for name,directory in [('amazon',True),('cloud-init.log',False),('cloud-init-output.log',False)]:
            try: fd=os.open(name,os.O_RDONLY|os.O_NOFOLLOW|os.O_CLOEXEC|(os.O_DIRECTORY if directory else os.O_NONBLOCK),dir_fd=log)
            except FileNotFoundError: continue
            try:
                m=os.fstat(fd)
                b.require(m.st_uid in ({0} if directory or logger is None else {0,logger.pw_uid}),'isolation_unavailable')
                b.require(stat.S_ISDIR(m.st_mode) if directory else stat.S_ISREG(m.st_mode) and m.st_nlink==1,'isolation_unavailable')
                if harden: os.fchmod(fd,0o700 if directory else 0o600)
                elif directory: b.require(stat.S_IMODE(m.st_mode)==0o700,'isolation_unavailable')
                else:
                    b.require(m.st_mode&0o007==0 and m.st_gid!=account('craxii').pw_gid,'isolation_unavailable')
                    rules,_=b.command(['/usr/bin/getfacl','--numeric','--omit-header','/var/log/'+name])
                    b.require(all(line.startswith(('user::','group::','other::')) for line in rules.decode().splitlines() if line),'isolation_unavailable')
            finally: os.close(fd)
    finally:
        if log is not None:os.close(log)
        os.close(var)

def remove_scratch(parent,name):
    """No symlink following, recursion, absolute descendant paths or depth-sized FD stack.

    Called only after the model UID is empty and nested mounts were rejected. Names and
    iterators use memory bounded by the verified tmpfs inode limit, including hostile depth.
    """
    base=os.open(parent,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW|os.O_CLOEXEC)
    current=None
    try:
        current=os.open(name,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW|os.O_CLOEXEC,dir_fd=base)
        device=os.fstat(current).st_dev
        stack=[];entries=iter(os.listdir(current));visited=0
        while True:
            entry=next(entries,None)
            if entry is None:
                if not stack:
                    os.close(current);current=None
                    os.rmdir(name,dir_fd=base)
                    return
                previous=os.open('..',os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW|os.O_CLOEXEC,dir_fd=current)
                os.close(current);current=previous
                child,entries=stack.pop();os.rmdir(child,dir_fd=current)
                continue
            visited+=1;b.require(visited<=65536,'isolation_unavailable')
            info=os.stat(entry,dir_fd=current,follow_symlinks=False)
            b.require(info.st_dev==device,'isolation_unavailable')
            if stat.S_ISDIR(info.st_mode):
                child=os.open(entry,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW|os.O_CLOEXEC,dir_fd=current)
                b.require(os.fstat(child).st_dev==device,'isolation_unavailable')
                stack.append((entry,entries));os.close(current);current=child
                entries=iter(os.listdir(current))
            else:os.unlink(entry,dir_fd=current)
    finally:
        if current is not None:os.close(current)
        os.close(base)

def account(name):
    p=pwd.getpwnam(name)
    b.require(p.pw_uid!=0 and p.pw_gid!=0 and p.pw_gid==grp.getgrnam(name).gr_gid,'isolation_unavailable')
    return p

def nft_material(c):
    model,worker=account('craxii'),account('craxii-server')
    b.require(model.pw_uid==997,'isolation_unavailable')
    cidrs=c['private_networks']
    b.require(len(cidrs)<=32,'guest_config_invalid')
    networks=[ipaddress.ip_network(x,strict=True) for x in cidrs]
    b.require(all(str(n)==x for n,x in zip(networks,cidrs)),'guest_config_invalid')
    lines=['table inet craxii_guest {',' chain output {',' type filter hook output priority -150; policy accept;']
    for p in (model,worker):
        lines.extend([f' meta skuid {p.pw_uid} ip daddr 169.254.169.254 drop',f' meta skuid {p.pw_uid} ip6 daddr fd00:ec2::254 drop'])
        for net in networks:
            lines.append(f' meta skuid {p.pw_uid} {"ip" if net.version==4 else "ip6"} daddr {net} drop')
    # No protected localhost proxies/control services are available to tool UID.
    lines.extend([f' meta skuid {model.pw_uid} ip daddr 127.0.0.0/8 drop',f' meta skuid {model.pw_uid} ip6 daddr ::1 drop'])
    # OUTPUT precedes local broadcast/multicast loop delivery to INPUT. Those packets are
    # not FIB type local and need their own routing-class denials, in both IP families.
    # Multicast is unavailable to installed tools: even a non-local group can have a local
    # privileged subscriber. Approved non-local unicast TCP/UDP remains available.
    for kind in ('local','broadcast','multicast'):
        lines.append(f' meta skuid {model.pw_uid} fib daddr type {kind} drop')
    # Also cover routes sent through lo regardless of routing type, and protected link-local
    # service ranges. These are protocol classes, never fixture endpoint addresses.
    lines.extend([f' meta skuid {model.pw_uid} oifname "lo" drop',
                  f' meta skuid {model.pw_uid} ip daddr 169.254.0.0/16 drop',
                  f' meta skuid {model.pw_uid} ip6 daddr fe80::/10 drop',' }','}'])
    return '\n'.join(lines)+'\n'

def network_verify(c):
    # nft --check validates syntax only. Compare actual kernel expressions AND their attachment
    # to this active inet base chain. Reject dormant tables, other chains, objects and extra rules.
    b.require(account('craxii').pw_uid==997,'isolation_unavailable')
    actual,_=b.command(['/usr/sbin/nft','--json','list','table','inet','craxii_guest'])
    value=b.decode(actual,1048576)
    entries=value.get('nftables',[])
    b.require(all(len(x)==1 and next(iter(x)) in ('metainfo','table','chain','rule') for x in entries),'isolation_unavailable')
    tables=[x['table'] for x in entries if 'table' in x]
    b.require(len(tables)==1 and tables[0].get('family')=='inet' and tables[0].get('name')=='craxii_guest' and not tables[0].get('flags'),'isolation_unavailable')
    rule_records=[x['rule'] for x in entries if 'rule' in x]
    b.require(all(r.get('family')=='inet' and r.get('table')=='craxii_guest' and r.get('chain')=='output' for r in rule_records),'isolation_unavailable')
    rules=[r['expr'] for r in rule_records]
    # Explicit expected matches. A required prefix may be encoded as addr/len in nft JSON.
    expected=[]
    for name,addresses in [('craxii',['169.254.169.254','fd00:ec2::254',*c['private_networks']]),('craxii-server',['169.254.169.254','fd00:ec2::254',*c['private_networks']]),('craxii',['127.0.0.0/8','::1'])]:
        uid=account(name).pw_uid
        for addr in addresses:
            net=ipaddress.ip_network(addr,strict=True)
            right=str(net.network_address) if net.prefixlen==net.max_prefixlen else {'prefix':{'addr':str(net.network_address),'len':net.prefixlen}}
            expected.append([{'match':{'op':'==','left':{'meta':{'key':'skuid'}},'right':uid}}, {'match':{'op':'==','left':{'payload':{'protocol':'ip' if net.version==4 else 'ip6','field':'daddr'}},'right':right}}, {'drop':None}])
    uid={'match':{'op':'==','left':{'meta':{'key':'skuid'}},'right':997}}
    for kind in ('local','broadcast','multicast'):
        expected.append([uid, {'match':{'op':'==','left':{'fib':{'result':'type','flags':['daddr']}},'right':kind}}, {'drop':None}])
    expected.append([uid, {'match':{'op':'==','left':{'meta':{'key':'oifname'}},'right':'lo'}}, {'drop':None}])
    for protocol,addr,length in [('ip','169.254.0.0',16),('ip6','fe80::',10)]:
        expected.append([uid, {'match':{'op':'==','left':{'payload':{'protocol':protocol,'field':'daddr'}},'right':{'prefix':{'addr':addr,'len':length}}}}, {'drop':None}])
    b.require(rules==expected,'isolation_unavailable')
    chains=[x['chain'] for x in value.get('nftables',[]) if 'chain' in x]
    b.require(len(chains)==1 and chains[0].get('family')=='inet' and chains[0].get('table')=='craxii_guest' and chains[0].get('name')=='output' and chains[0].get('type')=='filter' and chains[0].get('hook')=='output' and chains[0].get('prio')==-150 and chains[0].get('policy')=='accept','isolation_unavailable')

def network_reload(c):
    # Reload only this exact owned policy in one atomic kernel transaction. Never flush the
    # ruleset, accept arbitrary nft text, or repair conflicting/missing enforcement silently.
    material=nft_material(c).encode()
    b.require(b.read_safe(Path('/etc/craxii/network-policy.nft'))==material,'isolation_unavailable')
    network_verify(c)
    b.command(['/usr/sbin/nft','--file','-'],stdin=b'delete table inet craxii_guest\n'+material)
    network_verify(c)

def verify(c):
    worker,model=account('craxii-server'),account('craxii')
    # Installed shells require filesystem reference/truncation controls (Landlock ABI 3).
    b.require(ctypes.CDLL(None,use_errno=True).syscall(444,None,0,1)>=3,'isolation_unavailable')
    b.require(worker.pw_uid!=model.pw_uid and worker.pw_gid!=model.pw_gid and worker.pw_shell=='/usr/sbin/nologin' and model.pw_dir=='/home/craxii','isolation_unavailable')
    for group in grp.getgrall():
        b.require('craxii' not in group.gr_mem,'isolation_unavailable')
        b.require(group.gr_gid!=model.pw_gid or group.gr_name=='craxii','isolation_unavailable')
    for path in ['/var/lib/cloud','/var/lib/amazon']:
        if Path(path).exists() or Path(path).is_symlink():
            fd=b.safe_open(path,0o700,directory=True);os.close(fd)
    protect_system_logs()
    for name,expected in [('kernel/yama/ptrace_scope','2'),('fs/suid_dumpable','0'),('kernel/unprivileged_userns_clone','0'),('kernel/core_pattern','|/bin/false')]:
        b.require((Path('/proc/sys')/name).read_text().strip()==expected,'isolation_unavailable')
    proc=[m for m in b.LinuxHost(c).mounts() if m['target']=='/proc']
    b.require(len(proc)==1 and any(f in proc[0]['super_flags'] for f in ('hidepid=2','hidepid=invisible')) and f"gid={grp.getgrnam('craxii-observe').gr_gid}" in proc[0]['super_flags'],'isolation_unavailable')
    for path in ['/tmp','/var/tmp','/dev/shm']:
        mounts=[m for m in b.LinuxHost(c).mounts() if m['target']==path]
        b.require(len(mounts)==1 and mounts[0]['fs']=='tmpfs' and {'nosuid','nodev'}<=set(mounts[0]['flags']),'isolation_unavailable')
        v=os.statvfs(path)
        b.require(v.f_blocks*v.f_frsize<=int(c['resources']['temp_bytes']) and v.f_files<=65536,'isolation_unavailable')
        scratch=Path(path)/'craxii-tools'
        info=scratch.lstat()
        b.require(stat.S_ISDIR(info.st_mode) and info.st_uid==0 and info.st_gid==worker.pw_gid and stat.S_IMODE(info.st_mode)==0o1773,'isolation_unavailable')
    for p in ['/run/dbus','/run/lock','/var/crash']:
        if Path(p).exists():
            m=Path(p).stat(); b.require(m.st_uid==0 and m.st_mode&0o007==0,'isolation_unavailable')
    b.require(not Path('/var/lib/systemd/linger/craxii').exists(),'isolation_unavailable')
    for unit in ['cron.service','atd.service','user@'+str(model.pw_uid)+'.service']:
        out,_=b.command(['/usr/bin/systemctl','is-enabled',unit],allowed=(0,1,3,4))
        b.require(out.strip()==b'masked','isolation_unavailable')
    # Root storage cannot supply another model-writable scratch or owned file (for example mail).
    # Symlink mode 0777 does not grant writes to its target or its protected parent.
    # This image-wide startup scan has its own finite cold-filesystem budget.
    writable,_=b.command(['/usr/bin/find','/','-xdev','(','-uid',str(model.pw_uid),'-o','-gid',str(model.pw_gid),'-o','(','!','-type','l','-perm','-0002',')',')','-print'],timeout=60)
    paths=writable.decode().splitlines()
    b.require(len(paths)<=4096 and all(p in ('/tmp','/var/tmp','/dev/shm','/home/craxii','/srv/craxii/workspaces') for p in paths),'isolation_unavailable')
    # Quota support must be effective in the kernel, not just enabled in fstab.
    host=b.LinuxHost(c); disk=b.select(host,c)
    class Dqblk(ctypes.Structure):
        _fields_=[(name,ctypes.c_uint64) for name in ('bhard','bsoft','curspace','ihard','isoft','curinodes','btime','itime')]+[('valid',ctypes.c_uint32)]
    quota=Dqblk()
    libc=ctypes.CDLL(None,use_errno=True)
    result=libc.quotactl(ctypes.c_int((0x800007<<8)&0xffffffff),disk['path'].encode(),model.pw_uid,ctypes.byref(quota))
    b.require(result==0 and quota.bhard==int(c['resources']['disk_quota_bytes'])//1024 and quota.bsoft==quota.bhard and quota.ihard==c['resources']['inode_quota'] and quota.isoft==quota.ihard,'isolation_unavailable')
    network_verify(c)

def prepare(c):
    verify(c)
    host=b.LinuxHost(c); disk=b.select(host,c)
    host.verify_filesystem(disk); host.verify_layout(disk)
    receipt=b.decode(b.read_safe(b.INTENTS/(c['bootstrap_operation_id']+'.json')))
    b.require(receipt.get('phase')=='layout_verified' and receipt.get('config_sha256')==b.digest(c),'layout_unavailable')
    worker=account('craxii-server')
    b.require(Path('/sys/fs/cgroup/cgroup.controllers').is_file(),'isolation_unavailable')
    for name in ['cpu','memory','pids']:
        b.require(name in (ROOT/'cgroup.controllers').read_text().split(),'isolation_unavailable')
    # DelegateSubgroup places the supervisor away from the subtree-controller parent.
    b.require(not (ROOT/'cgroup.procs').read_text().strip(),'isolation_unavailable')
    # systemd killed the old service cgroup before a restart. An off-cgroup model process
    # makes recovery unsafe; do not erase its scratch or pretend that it is idle.
    rows=list(Path('/proc').iterdir())
    b.require(len(rows)<=4096,'isolation_unavailable')
    for path in rows:
        if path.name.isdecimal():
            try: status=(path/'status').read_text()
            except FileNotFoundError: continue
            uid=next((line.split()[1] for line in status.splitlines() if line.startswith('Uid:')),None)
            b.require(uid is not None and uid!=str(account('craxii').pw_uid),'isolation_unavailable')
    for base in ['/tmp','/var/tmp','/dev/shm']:
        parent=Path(base)/'craxii-tools'
        for path in parent.iterdir():
            b.require(str(uuid.UUID(path.name))==path.name,'isolation_unavailable')
            info=path.lstat()
            b.require(stat.S_ISDIR(info.st_mode) and info.st_uid==account('craxii').pw_uid and info.st_dev==parent.stat().st_dev,'isolation_unavailable')
            b.require(not any(m['target'].startswith(str(path)+'/') or m['target']==str(path) for m in host.mounts()),'isolation_unavailable')
            remove_scratch(parent,path.name)
    (ROOT/'cgroup.subtree_control').write_text('+cpu +memory +pids')
    tools=ROOT/'tools'
    if not tools.exists(): tools.mkdir()
    for name,value in [('pids.max','64'),('memory.max','268435456'),('memory.swap.max','0'),('cpu.max','100000 100000')]:
        (tools/name).write_text(value)
        b.require((tools/name).read_text().strip()==value,'isolation_unavailable')
    (tools/'cgroup.subtree_control').write_text('+cpu +memory +pids')
    os.chown(tools,worker.pw_uid,worker.pw_gid)
    # Tool UID cannot modify cgroup parent, controls or migration handles.
    for path in [tools,tools/'cgroup.procs',tools/'cgroup.subtree_control']:
        os.chown(path,worker.pw_uid,worker.pw_gid)
        os.chmod(path,0o700 if path==tools else 0o600)
    # The worker cannot read root-only bootstrap intents. Publish only the verified phase,
    # bound to this boot and exact config, through the already protected read-only receipt.
    record={'version':1,'config_sha256':b.digest(c),'boot_id':Path('/proc/sys/kernel/random/boot_id').read_text().strip(),'storage_phase':'layout_verified'}
    ready=Path('/run/craxii-worker/security-ready.json')
    if ready.exists(): b.require(b.decode(b.read_safe(ready,0o440))==record,'isolation_unavailable')
    else: b.atomic_record(ready,record,0o440,worker.pw_gid)

def configure(c):
    host=b.LinuxHost(c); disk=b.select(host,c);host.verify_filesystem(disk);host.verify_layout(disk)
    b.command(['/usr/sbin/setquota','--user','craxii','67108864','67108864','1000000','1000000','/srv/craxii-data'])
    # Internal ext4 quota files are created by mkfs; no quotacheck over an active journal.
    b.command(['/usr/sbin/quotaon','--user','/srv/craxii-data'],allowed=(0,1))
    policy=Path('/etc/craxii/network-policy.nft')
    material=nft_material(c).encode()
    if policy.exists(): b.require(b.read_safe(policy)==material,'isolation_unavailable')
    else:
        fd=os.open(policy,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
        with os.fdopen(fd,'wb') as file: file.write(material);file.flush();os.fsync(file.fileno())
    # Never flush or replace an unrelated table. A conflicting existing policy holds.
    _,status=b.command(['/usr/sbin/nft','list','table','inet','craxii_guest'],allowed=(0,1))
    if status!=0: b.command(['/usr/sbin/nft','--file',str(policy)])
    network_verify(c)
    b.command(['/usr/sbin/sysctl','--load','/etc/sysctl.d/90-craxii-guest.conf'])
    group=grp.getgrnam('craxii-observe').gr_gid
    b.command(['/usr/bin/mount','-o',f'remount,hidepid=2,gid={group}','/proc'])
    for target in ['/tmp','/var/tmp','/dev/shm']:
        existing=[m for m in host.mounts() if m['target']==target]
        b.require(len(existing)<=1,'isolation_unavailable')
        if existing:
            b.require(existing[0]['fs']=='tmpfs','isolation_unavailable')
            b.command(['/usr/bin/mount','-o','remount,rw,nosuid,nodev,size=268435456,nr_inodes=65536',target])
        else:
            # Ubuntu services may already have root-owned PrivateTmp directories. Preserve
            # them underneath the new mount; never erase or hide preexisting model data.
            b.require(all(p.lstat().st_uid==0 for p in Path(target).iterdir()),'isolation_unavailable')
            model=account('craxii')
            owned,_=b.command(['/usr/bin/find',target,'-xdev','(','-uid',str(model.pw_uid),'-o','-gid',str(model.pw_gid),')','-print'])
            b.require(not owned.strip(),'isolation_unavailable')
            b.command(['/usr/bin/mount','-t','tmpfs','-o','rw,nosuid,nodev,size=268435456,nr_inodes=65536,mode=1777','tmpfs',target])
        scratch=Path(target)/'craxii-tools'
        if not scratch.exists():
            scratch.mkdir(mode=0o1773)
            os.chown(scratch,0,account('craxii-server').pw_gid)
            scratch.chmod(0o1773)
    for path in ['/var/lib/cloud','/var/lib/amazon']:
        p=Path(path)
        if p.exists() or p.is_symlink():
            info=p.lstat();directory=stat.S_ISDIR(info.st_mode)
            fd=b.safe_open(path,stat.S_IMODE(info.st_mode),directory=directory)
            try: os.fchmod(fd,0o700 if directory else 0o600)
            finally: os.close(fd)
    protect_system_logs(harden=True)
    # /run is volatile. Image-time chmod/mkdir does not survive a cold boot. Reapply
    # the approved root-only IPC policy before verification, and recreate only our
    # fixed protected runtime directory through a pinned root-owned parent.
    for path in ['/run/dbus','/run/lock','/var/crash']:
        p=Path(path)
        if p.exists() or p.is_symlink():
            info=p.lstat()
            b.require(stat.S_ISDIR(info.st_mode) and info.st_uid==0,'isolation_unavailable')
            fd=b.safe_open(path,stat.S_IMODE(info.st_mode),directory=True)
            try:os.fchmod(fd,0o700)
            finally:os.close(fd)
    parent=b.safe_open('/run',0o755,directory=True)
    try:
        try:
            os.mkdir('craxii-worker',mode=0o750,dir_fd=parent)
            os.chown('craxii-worker',0,account('craxii-server').pw_gid,dir_fd=parent,follow_symlinks=False)
        except FileExistsError:pass
        fd=os.open('craxii-worker',os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW|os.O_CLOEXEC,dir_fd=parent)
        try:
            info=os.fstat(fd)
            mode=stat.S_IMODE(info.st_mode)
            b.require(info.st_uid==0 and info.st_gid==account('craxii-server').pw_gid and mode in (0o700,0o750),'isolation_unavailable')
            if mode==0o700:
                # Root unit UMask0077 narrows mkdir0750. Only its empty, root-owned
                # interrupted creation may be completed; never rewrite foreign content.
                b.require(not os.listdir(fd),'isolation_unavailable')
                os.fchmod(fd,0o750)
        finally:os.close(fd)
    finally:os.close(parent)
    verify(c)

def main():
    b.require(os.geteuid()==0 and sys.argv[1:] in (['render-network'],['configure'],['prepare'],['verify'],['verify-network'],['reload-network']),'isolation_unavailable')
    b.require(os.uname().machine=='x86_64','isolation_unavailable')
    bytes_=b.read_safe(b.CONFIG,0o640)
    b.command([b.GUARD,'validate'],stdin=bytes_)
    c=b.decode(bytes_)
    b.require(c['provenance']['kind']=='trusted_infrastructure','isolation_unavailable')
    if sys.argv[1]=='render-network':
        # Installed rendering goes to a fixed protected destination, never stdout.
        target=Path('/etc/craxii/network-policy.nft')
        b.require(not target.exists() and not target.is_symlink(),'isolation_unavailable')
        fd=os.open(target,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
        with os.fdopen(fd,'w') as f: f.write(nft_material(c)); f.flush(); os.fsync(f.fileno())
    elif sys.argv[1]=='configure': configure(c)
    elif sys.argv[1]=='prepare': prepare(c)
    elif sys.argv[1]=='verify-network': network_verify(c)
    elif sys.argv[1]=='reload-network': network_reload(c)
    else: verify(c)
if __name__=='__main__':
    try: main()
    except b.Hold as e: print(e.code,file=sys.stderr);sys.exit(1)
    except BaseException: print('isolation_unavailable',file=sys.stderr);sys.exit(1)
