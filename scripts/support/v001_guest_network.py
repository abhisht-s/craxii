"""Bounded installed network regression; owned guest with live root controls required."""
import errno
import os
from pathlib import Path
import socket

assert os.getresuid() == (997, 997, 997)
status = Path('/proc/self/status').read_text()
assert 'CapEff:\t0000000000000000' in status and 'NoNewPrivs:\t1' in status

# First five reproduce the independent probe exactly (including payload, port, interface and
# socket options). The fixture supplies routes and trusted receipts even for limited broadcast
# and interface-local multicast, which were previously unavailable rather than isolation PASS.
cases = [
    ('198.18.13.3', None), ('255.255.255.255', None),
    ('224.0.0.1', '198.18.13.1'), ('ff02::1', 'cr01-host'), ('ff01::1', 'lo'),
    ('::ffff:198.18.13.3', None), ('::ffff:224.0.0.1', 'cr01-host'),
    ('239.192.13.1', '198.18.13.1'), ('ff05::114', 'cr01-host'),
    ('198.18.13.1', None), ('2001:db8:13::1', None),
    ('198.18.13.13', None), ('2001:db8::13', None),
    ('169.254.169.254', None), ('fd00:ec2::254', None),
    ('10.13.13.13', None), ('fd12:3456::13', None),
    ('169.254.13.2', None), ('fe80::13:2', 'cr01-host'),
]
for address, interface in cases:
    family = socket.AF_INET6 if ':' in address else socket.AF_INET
    with socket.socket(family, socket.SOCK_DGRAM) as sock:
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
        destination = (address, 18083)
        if family == socket.AF_INET and interface:
            sock.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_IF,
                            socket.inet_aton(interface))
        if family == socket.AF_INET6 and interface:
            index = socket.if_nametoindex(interface)
            sock.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_MULTICAST_IF, index)
            if address.startswith('::ffff:'):
                sock.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_IF,
                                socket.inet_aton('198.18.13.1'))
            destination = (address, 18083, 0, index)
        try:
            sock.sendto(('reverify01-untrusted-' + address).encode(), destination)
        except OSError as error:
            assert error.errno in (errno.EPERM, errno.EACCES), (address, error.errno)
        else:
            raise AssertionError('privileged local delivery permitted: ' + address)
# Decision 45 (B6.9): the tool UID may use localhost. (The fixture's own 198.18.13.x addresses are no longer refused
# as "local" type; recheck those cases against its private ranges when this suite next runs on the QEMU guest.)
for address in ('127.0.0.1', '::1'):
    family = socket.AF_INET6 if ':' in address else socket.AF_INET
    with socket.socket(family, socket.SOCK_DGRAM) as sock:
        sock.sendto(b'reverify01-localhost', (address, 18083))
print('PASS installed-network-19-cases-uid997-permission-denied-localhost-allowed', flush=True)
