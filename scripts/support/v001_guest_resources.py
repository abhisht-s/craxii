"""Bounded harmless pressure commands, only under the actual installed tool cgroup/UID."""
import errno
import os
from pathlib import Path
import pwd
import sys
import time
assert os.getuid()!=0 and pwd.getpwuid(os.getuid()).pw_name=='craxii'
assert Path('/proc/self/cgroup').read_text().startswith('0::/system.slice/craxii-distribution.service/tools/')
mode=sys.argv[1]
if mode=='pids':
    hit=False
    for _ in range(128):
        try: pid=os.fork()
        except BlockingIOError: hit=True;break
        if pid==0:time.sleep(60);os._exit(0)
    assert hit
    time.sleep(60)
elif mode=='memory':
    data=bytearray(512*1024*1024)
    raise AssertionError('installed memory bound was not enforced')
elif mode=='cpu':
    for _ in range(2):
        if os.fork()==0:
            while True:pass
    time.sleep(60)
elif mode in ('scratch','scratch-hold'):
    # The root fixture may narrow the installed tmpfs maximum to 32 MiB for bounded pressure.
    try:
        with (Path(os.environ['TMPDIR'])/'pressure').open('wb',buffering=0) as f:
            for _ in range(257): f.write(bytes(1024*1024))
    except OSError as e:
        assert e.errno==errno.ENOSPC
    else:raise AssertionError('scratch limit was not enforced')
    if mode=='scratch-hold':
        assert sys.argv[2]==Path('/proc/self/cgroup').read_text().strip().rsplit('/',1)[1]
        (Path.cwd()/('qualification-pressure-'+sys.argv[2])).write_text('persistent-workspace-canary')
        time.sleep(5)
else:raise AssertionError('unknown bounded fixture command')
