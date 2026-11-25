# Synthetic denied-access assertions; never print credential or process environment bytes.
import os,errno,resource,socket,stat,ctypes,json,sys
from pathlib import Path
assert os.getuid()==os.geteuid()!=0
assert not os.getgroups()
s=Path('/proc/self/status').read_text()
for name in ('CapEff','CapPrm','CapInh','CapAmb','CapBnd'):
 assert int(next(x.split()[1] for x in s.splitlines() if x.startswith(name+':')),16)==0
assert 'NoNewPrivs:\t1' in s
assert resource.getrlimit(resource.RLIMIT_NOFILE)==(1024,1024)
assert resource.getrlimit(resource.RLIMIT_CORE)==(0,0)
assert set(os.environ)<=set('HOME USER LOGNAME SHELL LANG PATH CRAXII_WORK_ID CRAXII_WORKSPACE_ID TMPDIR TMP TEMP PWD SHLVL _'.split())
def denied(path,flags,missing=False):
 try: fd=os.open(path,flags)
 except OSError as e:
  allowed=(errno.EACCES,errno.EPERM,errno.ENOENT) if missing else (errno.EACCES,errno.EPERM)
  if path.startswith('/mnt/') and flags & os.O_WRONLY: allowed+= (errno.EROFS,)
  assert e.errno in allowed, (path,e.errno)
 else: os.close(fd);raise AssertionError('protected path accessible')
for path in ['/mnt/repo/AGENTS.md','/mnt/build/x86_64-unknown-linux-gnu/debug/craxii-server','/dev/nvme0n1','/dev/vda','/var/log/cloud-init.log','/var/log/cloud-init-output.log','/var/lib/amazon/ssm/qualification-synthetic','/var/log/amazon/ssm/qualification-synthetic','/var/lib/craxii-worker/identity/worker.pk8','/var/lib/craxii-worker/identity/distribution.json','/etc/craxii/guest-bootstrap.json','/var/lib/craxii/db/craxii.sqlite3','/srv/craxii-data/state/db/craxii.sqlite3','/var/lib/craxii-worker/bootstrap/bootstrap.lock']:
 denied(path,os.O_RDONLY);denied(path,os.O_WRONLY)
for path in ['/opt/craxii/current/craxii-server','/etc/systemd/system/craxii-distribution.service','/srv/craxii-data/.craxii-data-lineage.json','/etc/passwd','/sys/fs/cgroup/cgroup.procs']:
 denied(path,os.O_WRONLY)
for binary in ['/usr/bin/sudo','/usr/bin/su','/usr/bin/systemctl','/usr/bin/mount','/usr/bin/crontab']:
 if Path(binary).exists():assert not os.access(binary,os.X_OK)
# Both direct protected paths and a user-controlled symlink resolve to denied objects.
p=Path.cwd()/'qualification-link'
try:
 p.symlink_to('/var/lib/craxii/db/craxii.sqlite3');denied(str(p),os.O_RDONLY);denied(str(p),os.O_WRONLY)
finally:p.unlink(missing_ok=True)
# Scratch cannot write the shared tmp roots or follow a scratch symlink into trusted state.
for base in ['/tmp','/var/tmp','/dev/shm']:
 denied(base+'/qualification-escape',os.O_WRONLY|os.O_CREAT)
tmp=Path(os.environ['TMPDIR']);(tmp/'own').write_text('harmless')
p=tmp/'link';p.symlink_to('/var/lib/craxii/db/craxii.sqlite3');denied(str(p),os.O_WRONLY)
assert (tmp/'own').read_text()=='harmless'
# The fixture parent publishes only its PID, never secret bytes.
pid=int(sys.argv[1]); inherited_fd=int(sys.argv[2])
try: os.fstat(inherited_fd)
except OSError as e: assert e.errno==errno.EBADF
else: raise AssertionError('inherited secret descriptor survived exec')
for suffix in ['environ','mem','fd/9']:
 denied(f'/proc/{pid}/{suffix}',os.O_RDONLY,True)
# Secret-bearing socket directory is real, with a root-owned harmless listening fixture.
for path in ['/run/craxii-worker/protected.sock','/run/dbus/system_bus_socket']:
 sock=None
 try:
  sock=socket.socket(socket.AF_UNIX);sock.connect(path)
 except OSError as e:assert e.errno in (errno.EACCES,errno.EPERM,errno.ENOENT)
 else:raise AssertionError('protected socket accessible')
 finally:
  if sock is not None:sock.close()
# A bounded FD attempt fails at the installed hard bound.
fds=[]
try:
 while True:fds.append(os.open('/dev/null',os.O_RDONLY))
except OSError as e:assert e.errno==errno.EMFILE and len(fds)<1024
finally:
 for fd in fds:os.close(fd)
print('PASS hostile-permission-environment-fd-capability-scratch')

libc=ctypes.CDLL(None,use_errno=True)
assert libc.setuid(0)==-1 and ctypes.get_errno()==errno.EPERM
assert libc.unshare(0x10000000)==-1 and ctypes.get_errno()==errno.EPERM
assert libc.unshare(0x40000000)==-1 and ctypes.get_errno()==errno.EPERM
assert libc.ptrace(0x4206,pid,0,0)==-1 and ctypes.get_errno()==errno.EPERM
class Iovec(ctypes.Structure):
 _fields_=[('base',ctypes.c_void_p),('size',ctypes.c_size_t)]
buf=ctypes.create_string_buffer(1);local=Iovec(ctypes.cast(buf,ctypes.c_void_p),1);remote=Iovec(1,1)
assert libc.process_vm_readv(pid,ctypes.byref(local),1,ctypes.byref(remote),1,0)==-1 and ctypes.get_errno()==errno.EPERM
for path in ['/var/lib/dpkg/status','/var/spool/cron/crontabs/qualification','/var/lib/systemd/linger/qualification']:
 denied(path,os.O_WRONLY|os.O_CREAT)
# These exact synthetic addresses are installed on this NIC-free fixture's loopback by root;
# every listener has already answered a root control before the actual launcher is tested.
def network():
 for address in ['169.254.169.254','fd00:ec2::254','10.13.13.13','fd12:3456::13','::ffff:169.254.169.254','127.0.0.1','::1']:
  family=socket.AF_INET6 if ':' in address else socket.AF_INET
  for method in [b'GET',b'PUT']:
   sock=socket.socket(family);sock.settimeout(.3)
   try:sock.connect((address,18080));sock.sendall(b'PUT /latest/api/token HTTP/1.0\r\nX-aws-ec2-metadata-token-ttl-seconds: 60\r\n\r\n' if method==b'PUT' else b'GET /latest/meta-data/iam/security-credentials/ HTTP/1.0\r\n\r\n')
   except OSError as e:assert e.errno in (None,errno.ETIMEDOUT,errno.EPERM,errno.EACCES)
   else:raise AssertionError('protected network endpoint accessible')
   finally:sock.close()
network()
child=os.fork()
if child==0:
 os.setsid();grandchild=os.fork()
 if grandchild==0:network();os._exit(0)
 _,status=os.waitpid(grandchild,0);os._exit(os.waitstatus_to_exitcode(status))
_,status=os.waitpid(child,0);assert os.waitstatus_to_exitcode(status)==0
print('PASS hostile-proc-ptrace-memory-root-userns-persistence-network-descendants')

child=os.fork()
if child==0:
 os.chdir(os.environ['TMPDIR']);getattr(libc,'raise')(11);os._exit(1)
_,status=os.waitpid(child,0);assert os.WIFSIGNALED(status) and os.WTERMSIG(status)==11
assert not any(p.name.startswith('core') for p in tmp.iterdir())
print('PASS kernel-core-dump-suppression')
