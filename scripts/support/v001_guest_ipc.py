"""Bounded installed-launcher IPC regression. Root controls must prove endpoints separately."""
import ctypes
import errno
import os
from pathlib import Path
import socket
import stat
import sys

CANARY = b'<14>Oct 15 12:00:00 craxii-ipc-regression: remediation01-denied-canary'
ENDPOINTS = [
    ('/dev/log', socket.SOCK_DGRAM),
    ('/run/systemd/journal/dev-log', socket.SOCK_DGRAM),
    ('/run/systemd/journal/syslog', socket.SOCK_DGRAM),
    ('/run/systemd/journal/socket', socket.SOCK_DGRAM),
    ('/run/systemd/journal/stdout', socket.SOCK_STREAM),
    ('/run/systemd/journal/io.systemd.journal', socket.SOCK_STREAM),
    ('/run/systemd/private', socket.SOCK_STREAM),
    ('/run/dbus/system_bus_socket', socket.SOCK_STREAM),
    ('/run/udev/control', socket.SOCK_SEQPACKET),
    ('/run/snapd.socket', socket.SOCK_STREAM),
    ('\0/org/kernel/linux/storage/multipathd', socket.SOCK_STREAM),
    ('\0ISCSIADM_ABSTRACT_NAMESPACE', socket.SOCK_STREAM),
    ('/run/craxii-remediation01.sock', socket.SOCK_DGRAM),
    ('\0craxii-remediation01', socket.SOCK_DGRAM),
]

def denied(call):
    try:
        result = call()
    except OSError as error:
        assert error.errno == errno.EPERM, error.errno
    else:
        if hasattr(result, 'close'):
            result.close()
        raise AssertionError('local IPC policy absent')

def probe():
    assert os.getuid() == 997 and os.geteuid() == 997
    assert 'Seccomp:\t2' in Path('/proc/self/status').read_text()
    # Run in this process, and again in the fork/setsid child and exec grandchild below.
    import runpy
    runpy.run_path('/opt/craxii/qualification/network.py', run_name='__main__')
    # Original bounded sends, with socket creation inside the denied operation. EPERM is
    # required; ENOENT/ECONNREFUSED cannot stand in for enforcement.
    def send(endpoint, kind):
        with socket.socket(socket.AF_UNIX, kind) as sock:
            if kind == socket.SOCK_DGRAM:
                sock.sendto(CANARY, endpoint)
            else:
                sock.connect(endpoint)
    for endpoint, kind in ENDPOINTS:
        denied(lambda: send(endpoint, kind))
    print('PASS original3 pathname and native/stream/abstract IPC denied EPERM')
    for path in ['/dev/log', '/run/systemd/journal/dev-log', '/run/systemd/journal/syslog']:
        denied(lambda: send('/proc/self/root' + path, socket.SOCK_DGRAM))
    alias = Path(os.environ['TMPDIR']) / 'logging-alias'
    alias.symlink_to('/dev/log')
    try:
        denied(lambda: send(str(alias), socket.SOCK_DGRAM))
    finally:
        alias.unlink()
    for kind in [socket.SOCK_DGRAM, socket.SOCK_STREAM, socket.SOCK_SEQPACKET]:
        denied(lambda: socket.socketpair(socket.AF_UNIX, kind))
    for family in [socket.AF_NETLINK, socket.AF_VSOCK]:
        denied(lambda: socket.socket(family, socket.SOCK_DGRAM))
    libc = ctypes.CDLL(None, use_errno=True)
    # Native raw syscalls, x32, high argument bits and alternate FD acquisition paths.
    for number, args in [
        (41, [1, 2, 0]), (41, [0x100000001, 2, 0]),
        (425, [0, 0, 0]), (438, [-1, 0, 0]), (0x40000029, [1, 2, 0]),
        (317, [1, 0, 0]), (157, [22, 2, 0]),
    ]:
        ctypes.set_errno(0)
        assert libc.syscall(ctypes.c_long(number), *[ctypes.c_long(x) for x in args]) == -1
        assert ctypes.get_errno() == errno.EPERM
    class Filter(ctypes.Structure):
        _fields_ = [('code', ctypes.c_ushort), ('jt', ctypes.c_ubyte),
                    ('jf', ctypes.c_ubyte), ('k', ctypes.c_uint)]
    class Program(ctypes.Structure):
        _fields_ = [('size', ctypes.c_ushort), ('filter', ctypes.POINTER(Filter))]
    # Valid additional logging filter; its installation must be denied, not merely rejected
    # because of a bad pointer. No logging filter is actually installed on the passing path.
    rules = (Filter * 4)(Filter(0x20, 0, 0, 0), Filter(0x15, 0, 1, 39),
                         Filter(0x06, 0, 0, 0x7ffc0000), Filter(0x06, 0, 0, 0x7fff0000))
    program = Program(4, rules)
    for number, first, second in [(317, 1, 2), (157, 22, 2)]:
        ctypes.set_errno(0)
        assert libc.syscall(number, first, second, ctypes.byref(program)) == -1
        assert ctypes.get_errno() == errno.EPERM
    for fd in [0, 1, 2]:
        assert not stat.S_ISSOCK(os.fstat(fd).st_mode)
    # IPv4/IPv6 TCP and UDP creation remain supported; existing UID nft policy still applies.
    for family in [socket.AF_INET, socket.AF_INET6]:
        for kind in [socket.SOCK_STREAM, socket.SOCK_DGRAM]:
            with socket.socket(family, kind):
                pass

probe()
if sys.argv[1:] != ['grandchild']:
    if len(sys.argv) == 2:
        try:
            os.fstat(int(sys.argv[1]))
        except OSError as error:
            assert error.errno == errno.EBADF
        else:
            raise AssertionError('inherited Unix socket survived')
    # Fork, setsid, grandchild exec and process replacement must retain the same filter.
    child = os.fork()
    if child == 0:
        os.setsid()
        probe()
        grandchild = os.fork()
        if grandchild == 0:
            os.execv(sys.executable, [sys.executable, __file__, 'grandchild'])
        _, status = os.waitpid(grandchild, 0)
        os._exit(os.waitstatus_to_exitcode(status))
    _, status = os.waitpid(child, 0)
    assert os.waitstatus_to_exitcode(status) == 0
    # int80 uses AUDIT_ARCH_I386. Test rejection without invoking any privileged service.
    import mmap
    code = mmap.mmap(-1, 4096, prot=mmap.PROT_READ | mmap.PROT_WRITE | mmap.PROT_EXEC)
    code.write(bytes.fromhex('b814000000cd80c3'))  # i386 getpid; ret
    assert ctypes.CFUNCTYPE(ctypes.c_int)(
        ctypes.addressof(ctypes.c_char.from_buffer(code))
    )() == -errno.EPERM
    # An offline fixture supplies these allowed public-address stand-ins on lo. Both families
    # and TCP/UDP must still work, in addition to protected-address denials in hostile.py.
    for address in ['198.18.13.2', '2001:db8:13::2']:
        for kind in [socket.SOCK_STREAM, socket.SOCK_DGRAM]:
            with socket.socket(socket.AF_INET6 if ':' in address else socket.AF_INET, kind) as sock:
                sock.settimeout(2)
                sock.connect((address, 18081))
                sock.sendall(b'synthetic-approved')
                assert sock.recv(64) == b'synthetic-approved'
    # Wildcard local services must not be reachable via a public/non-loopback self alias.
    # Root controls prove both the harmless collector and SSH listener are alive here.
    for address in ['198.18.13.13', '2001:db8::13', '::ffff:198.18.13.13']:
        for port in [22, 18081]:
            with socket.socket(socket.AF_INET6 if ':' in address else socket.AF_INET) as sock:
                sock.settimeout(.3)
                try:
                    sock.connect((address, port))
                except OSError as error:
                    assert error.errno in (None, errno.ETIMEDOUT, errno.EPERM, errno.EACCES)
                else:
                    raise AssertionError('guest-local privileged service bypass')
        with socket.socket(socket.AF_INET6 if ':' in address else socket.AF_INET, socket.SOCK_DGRAM) as sock:
            sock.settimeout(.3)
            try:
                sock.connect((address, 18081))
                sock.sendall(b'synthetic-approved')
                sock.recv(64)
            except OSError as error:
                assert error.errno in (None, errno.ETIMEDOUT, errno.EPERM, errno.EACCES)
            else:
                raise AssertionError('guest-local UDP service bypass')
    print('PASS installed-ipc-original3-alias-native-abstract-pair-fd-uring-compat-child-grandchild')
