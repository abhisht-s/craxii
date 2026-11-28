#!/usr/bin/python3 -I
"""The agent's package installs, as root (B6.9 part 4, decision 46).

Run by craxii-apt@.service for one connection on /run/craxii-apt.sock (socket activation: stdin/stdout are the
connection), only for the agent's UID. One request, {"packages": [...]}, is installed from Ubuntu's signed archive
through the image's own sources, and answered with one JSON line. It never hands out root. Closed by default:

1. plain package names only (at most 20) and a blocklist; a dry run refused if it would remove anything, upgrade a
   protected package or bring a blocked one;
2. every .deb is downloaded and read first, and refused if it would place anything that runs at boot or as root
   later, or changes system configuration (services, init scripts, cron, apt, PAM, sudo, sysctl, modules, udev,
   network, loader, tmpfiles, ...), or if its maintainer scripts divert files or edit such settings; too large refused;
3. the install itself runs with services refused (policy-rc.d), in its own process group under a time limit, and is
   always followed, even after a timeout, by neutralising: setuid/setgid bits and file capabilities removed from every
   file changed during it, sensitive system files restored from a snapshot, the installer's ACL denies reapplied, and
   anything the agent could newly write on the system disk closed. A crash or power-off part-way leaves a marker that
   `apt-helper.py recover` (run by the worker's root pre-start) finishes before the worker starts.

One install at a time, a few an hour. Answers are codes only.
"""
import base64
import fcntl
import json
import os
from pathlib import Path
import re
import signal
import socket
import stat
import struct
import subprocess
import sys
import tarfile
import time

MODEL = 'craxii'
MAX_PACKAGES = 20
NAME = re.compile(r'^[a-z0-9][a-z0-9+.-]{1,62}$')
# Exact names and name prefixes the agent may never install (decision 46). The content check (2) is what is
# closed by default; this list names what would otherwise slip through it (configuration changed by scripts).
BLOCKED_PREFIXES = ('linux-image', 'linux-modules', 'linux-headers', 'linux-generic', 'linux-virtual', 'linux-aws', 'linux-kvm',
                    'linux-signed', 'linux-firmware', 'linux-tools', 'linux-cloud-tools', 'grub', 'shim', 'systemd', 'udev', 'cloud-', 'openssh', 'docker', 'containerd', 'runc',
                    'podman', 'buildah', 'lxc', 'lxd', 'snapd', 'apparmor', 'polkit', 'policykit', 'pkexec', 'nftables',
                    'iptables', 'ufw', 'firewalld', 'netplan', 'network-manager', 'wireguard', 'openvpn', 'apt-transport',
                    'postfix', 'exim', 'sendmail', 'opensmtpd', 'courier', 'masqmail', 'nullmailer', 'qemu', 'libvirt',
                    'kexec', 'ubuntu-', 'unattended-upgrades', 'needrestart', 'libpam-', 'libnss-', 'cryptsetup', 'ec2-',
                    'amazon-', 'aws', 'sudo', 'opendoas', 'snoopy', 'auto-apt-proxy', 'squid-deb-proxy', 'molly-guard',
                    'etckeeper', 'dkms', 'virtualbox', 'fail2ban', 'connman', 'dhcpcd', 'ifupdown', 'resolvconf',
                    'openresolv', 'dnsmasq', 'unbound', 'bind9', 'miredo', 'tinyssh', 'dropbear', 'policyrcd', 'cronie',
                    'bcron', 'fcron', 'inetutils-inetd', 'openbsd-inetd', 'xinetd', 'rinetd')
BLOCKED_NAMES = {'apt', 'apt-utils', 'aptitude', 'dpkg', 'doas', 'login', 'passwd', 'su', 'util-linux', 'mount', 'cron', 'anacron', 'at', 'bash', 'dash',
                 'coreutils', 'libc6', 'libc-bin', 'init', 'init-system-helpers', 'base-files', 'base-passwd', 'acl', 'quota',
                 'python3', 'ca-certificates', 'openssl', 'gnupg', 'tor', 'chrony', 'nscd', 'findutils', 'e2fsprogs'}
# Installed packages a request may never upgrade (the boundary, boot and core tooling). Other installed packages may be
# upgraded to the archive's newer signed versions (a build toolchain needs libc6 matching libc6-dev, say).
PROTECTED = {'systemd', 'systemd-sysv', 'systemd-resolved', 'systemd-timesyncd', 'udev', 'libsystemd0', 'libudev1', 'nftables',
             'libnftables1', 'iptables', 'apt', 'libapt-pkg6.0t64', 'dpkg', 'cloud-init', 'snapd', 'apparmor', 'libapparmor1',
             'sudo', 'openssh-server', 'openssh-client', 'grub-common', 'grub2-common', 'shim-signed', 'login', 'passwd',
             'util-linux', 'mount', 'libpam0g', 'libpam-modules', 'libpam-runtime', 'acl', 'quota', 'e2fsprogs', 'init',
             'init-system-helpers', 'base-files', 'base-passwd', 'cryptsetup', 'linux-image-aws', 'linux-aws'}
# What a package may not place (paths inside its data, without the leading '/').
FORBIDDEN_PATHS = ('lib/systemd/system-generators', 'usr/lib/systemd/system-generators', 'lib/systemd/system-preset',
                   'usr/lib/systemd/system-preset', 'lib/systemd/system-environment-generators',
                   'usr/lib/systemd/system-environment-generators', 'lib/systemd/system-sleep', 'usr/lib/systemd/system-sleep',
                   'lib/systemd/system-shutdown', 'usr/lib/systemd/system-shutdown', 'lib/systemd/network', 'usr/lib/systemd/network',
                   'etc/systemd/', 'etc/rc', 'etc/cron', 'var/spool/cron', 'etc/apt/', 'etc/pam.d/', 'usr/lib/pam.d/', 'etc/security/',
                   'etc/sudo', 'etc/doas', 'etc/sysctl', 'lib/sysctl.d/', 'usr/lib/sysctl.d/', 'etc/modprobe.d/', 'lib/modprobe.d/',
                   'usr/lib/modprobe.d/', 'etc/modules', 'lib/modules', 'usr/lib/modules', 'etc/udev/', 'lib/udev/', 'usr/lib/udev/',
                   'etc/network/', 'etc/netplan/', 'etc/NetworkManager/', 'etc/ld.so', 'etc/nsswitch.conf', 'etc/resolv.conf',
                   'etc/hosts', 'etc/environment', 'etc/profile', 'etc/login.defs', 'etc/shells', 'usr/sbin/policy-rc.d', 'etc/inetd',
                   'etc/xinetd', 'etc/tmpfiles.d/', 'lib/tmpfiles.d/', 'usr/lib/tmpfiles.d/', 'etc/binfmt.d/', 'usr/lib/binfmt.d/',
                   'etc/dbus-1/', 'usr/share/dbus-1/system', 'etc/polkit-1/', 'usr/share/polkit-1/', 'etc/apparmor', 'boot/',
                   'etc/cloud/', 'etc/dpkg/dpkg.cfg', 'etc/kernel/', 'etc/initramfs-tools/', 'usr/share/initramfs-tools/',
                   'etc/update-motd.d/', 'etc/ssh/')
# A unit wanted from the package's own tree would start without any enable link (the restore can't undo that).
WANTED_FROM_TREE = re.compile(r'^(usr/)?lib/systemd/system/[^/]+\.(wants|requires|upholds)/')
# What a maintainer script may not do.
FORBIDDEN_SCRIPT = ('dpkg-divert', 'ld.so.preload', 'nsswitch', 'pam-auth-update', 'update-inetd', 'sudoers', 'policy-rc.d',
                    'crontab', 'setcap', 'dpkg-statoverride', 'modprobe', 'sysctl', 'iptables', 'nft ', 'resolv.conf', '/etc/hosts',
                    'chpasswd', 'usermod', 'ldconfig.real')
# `systemctl` only to reload or ask (enable links are undone by the restore; starts go through policy-rc.d).
SYSTEMCTL = re.compile(r'systemctl(?![^\n]*(daemon-reload|is-active|is-enabled|--quiet is-))')
# Restored after every install whatever happened (sensitive configuration root and the boot read).
SENSITIVE = ['/etc/ld.so.preload', '/etc/ld.so.conf', '/etc/ld.so.conf.d', '/etc/nsswitch.conf', '/etc/hosts', '/etc/environment',
             '/etc/sudoers', '/etc/sudoers.d', '/etc/pam.d', '/etc/security', '/etc/apt', '/etc/crontab', '/etc/cron.d',
             '/etc/inetd.conf', '/etc/profile', '/etc/profile.d', '/etc/systemd/system', '/etc/resolv.conf', '/etc/modprobe.d',
             '/etc/sysctl.d', '/etc/sysctl.conf', '/etc/udev/rules.d', '/etc/tmpfiles.d', '/etc/ssh', '/etc/rc0.d', '/etc/rc1.d',
             '/etc/rc2.d', '/etc/rc3.d', '/etc/rc4.d', '/etc/rc5.d', '/etc/rc6.d', '/etc/rcS.d', '/etc/logrotate.d']
ACCOUNTS = ['/etc/passwd', '/etc/shadow', '/etc/group', '/etc/gshadow']
# The installer's ACL denies (install-security-boundary.sh), reapplied after every install.
DENIED_TOOLS = ('sudo', 'su', 'pkexec', 'systemctl', 'busctl', 'crontab', 'at', 'newuidmap', 'newgidmap', 'mount', 'umount')
# What the worker's start-up scan allows the agent to write on the system disk (security-boundary.py verify).
AGENT_WRITABLE = ('/tmp', '/var/tmp', '/dev/shm', '/home/craxii', '/srv/craxii/workspaces')
MAX_INSTALLED = 3 * 1024 ** 3
MIN_FREE = 5 * 1024 ** 3
MIN_INODES = 100000
PER_HOUR = 6
STATE = Path('/var/lib/craxii-apt')
LOCK = Path('/run/craxii-apt.lock')
POLICY = Path('/usr/sbin/policy-rc.d')
POLICY_TEXT = '#!/bin/sh\n# craxii-apt: no service starts during the agent\'s package installs.\nexit 101\n'
ARCHIVES = Path('/var/cache/apt/archives')
ENV = {'PATH': '/usr/sbin:/usr/bin:/sbin:/bin', 'LC_ALL': 'C', 'DEBIAN_FRONTEND': 'noninteractive', 'NEEDRESTART_SUSPEND': '1',
       'APT_LISTCHANGES_FRONTEND': 'none'}
APT = ['/usr/bin/apt-get', '-q', '-o', 'APT::Install-Recommends=false', '-o', 'APT::Install-Suggests=false']
# Time budget, within craxii-apt@.service's RuntimeMaxSec (1800 s): update 180, dry run 60, uris 60, download 300,
# install 600, neutralise ~300.
TIMES = {'update': 180, 'simulate': 60, 'uris': 60, 'download': 300, 'install': 600}


class Refused(Exception):
    def __init__(self, code):
        super().__init__(code)
        self.code = code


def require(condition, code):
    if not condition:
        raise Refused(code)


def run(argv, timeout, check=True):
    """(exit status, stdout): a fixed command, the fixed environment, in its own process group, all of which is killed
    when the time is up."""
    process = subprocess.Popen(argv, env=ENV, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                               start_new_session=True)
    try:
        out, _ = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.communicate()
        raise Refused('apt_timed_out') from None
    require(not check or process.returncode == 0, 'apt_failed')
    return process.returncode, out.decode('utf-8', 'replace')[:8 * 1024 * 1024]


def blocked(name):
    return name in BLOCKED_NAMES or name.startswith(BLOCKED_PREFIXES) or name.endswith('-dkms')


def request(line):
    """The package names from one request line, or Refused."""
    try:
        value = json.loads(line)
    except ValueError:
        raise Refused('request_invalid') from None
    require(isinstance(value, dict) and set(value) == {'packages'} and isinstance(value['packages'], list), 'request_invalid')
    names = value['packages']
    require(0 < len(names) <= MAX_PACKAGES and len(set(names)) == len(names), 'request_invalid')
    require(all(isinstance(n, str) and NAME.match(n) for n in names), 'request_invalid')
    require(not any(blocked(n) for n in names), 'package_not_allowed')
    return names


def plan(simulation):
    """(new packages, upgraded packages, removed packages) from `apt-get -s install` output."""
    new, upgraded, removed = [], [], []
    for line in simulation.splitlines():
        parts = line.split()
        if len(parts) >= 2 and parts[0] == 'Inst':
            (upgraded if len(parts) >= 3 and parts[2].startswith('[') else new).append(parts[1].split(':')[0])
        elif len(parts) >= 2 and parts[0] in ('Remv', 'Purg'):
            removed.append(parts[1].split(':')[0])
    return new, upgraded, removed


def check_plan(new, upgraded, removed):
    require(not removed, 'would_remove_packages')
    require(not any(blocked(n) for n in new), 'package_not_allowed')
    require(not any(n in PROTECTED for n in upgraded), 'would_upgrade_protected')


def deb_files(uris):
    """The .deb file names apt will install, from `apt-get --print-uris` output ('URI' name size checksum)."""
    names = []
    for line in uris.splitlines():
        parts = line.split()
        if len(parts) >= 3 and parts[0].startswith("'") and parts[1].endswith('.deb') and '/' not in parts[1]:
            names.append(parts[1])
    return names


def forbidden_path(path):
    path = path[2:] if path.startswith('./') else path.lstrip('/')
    return path.startswith(FORBIDDEN_PATHS) or bool(WANTED_FROM_TREE.match(path))


def inspect(deb, new=True):
    """(installed bytes, file count) of one downloaded .deb; a new package is Refused if it may place or do anything
    forbidden (an upgrade of an installed package is only measured)."""
    with subprocess.Popen(['/usr/bin/dpkg-deb', '--fsys-tarfile', str(deb)], env=ENV, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL) as data:
        count = 0
        with tarfile.open(fileobj=data.stdout, mode='r|') as archive:
            for member in archive:
                count += 1
                if new:
                    require(not forbidden_path(member.name), 'package_not_allowed')
                    require(not (member.islnk() or member.issym()) or not forbidden_path(member.linkname or ''), 'package_not_allowed')
    require(data.returncode == 0, 'package_unreadable')
    with subprocess.Popen(['/usr/bin/dpkg-deb', '--ctrl-tarfile', str(deb)], env=ENV, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL) as control:
        installed = 0
        with tarfile.open(fileobj=control.stdout, mode='r|') as archive:
            for member in archive:
                if not member.isfile():
                    continue
                text = archive.extractfile(member).read(1024 * 1024).decode('utf-8', 'replace')
                name = member.name.lstrip('./')
                if new and name in ('preinst', 'postinst', 'prerm', 'postrm', 'config', 'triggers'):
                    require(not any(word in text for word in FORBIDDEN_SCRIPT) and not SYSTEMCTL.search(text), 'package_not_allowed')
                if name == 'control':
                    size = re.search(r'^Installed-Size:\s*(\d+)\s*$', text, re.M)
                    installed = int(size.group(1)) * 1024 if size else 0
    require(control.returncode == 0, 'package_unreadable')
    return installed, count


def recent(now):
    """Install times in the last hour, recorded root-only."""
    try:
        return [t for t in json.loads((STATE / 'history.json').read_text()) if isinstance(t, (int, float)) and now - t < 3600]
    except (FileNotFoundError, ValueError):
        return []


def write_state(name, text):
    temp = STATE / f'.{name}'
    temp.write_text(text)
    os.replace(temp, STATE / name)


def snapshot(paths):
    """Every regular file and link under the given paths: {path: {'mode','uid','gid','data'|'link'}}."""
    entries = {}
    for top in paths:
        top = Path(top)
        walk = [top] if not top.is_dir() or top.is_symlink() else [top, *top.rglob('*')]
        for path in walk[:20000]:
            try:
                info = os.lstat(path)
            except FileNotFoundError:
                continue
            if stat.S_ISLNK(info.st_mode):
                entries[str(path)] = {'link': os.readlink(path)}
            elif stat.S_ISREG(info.st_mode) and info.st_size <= 4 * 1024 * 1024:
                entries[str(path)] = {'mode': stat.S_IMODE(info.st_mode), 'uid': info.st_uid, 'gid': info.st_gid,
                                      'data': base64.b64encode(path.read_bytes()).decode()}
    return entries


def restore(saved, paths):
    """Puts the sensitive files back as they were: new ones removed, changed or removed ones rewritten."""
    current = snapshot(paths)
    for path in current:
        if path not in saved:
            os.unlink(path)
    for path, entry in saved.items():
        if current.get(path) == entry:
            continue
        target = Path(path)
        if target.is_symlink() or target.exists():
            if target.is_dir() and not target.is_symlink():
                continue
            os.unlink(target)
        target.parent.mkdir(parents=True, exist_ok=True)
        if 'link' in entry:
            os.symlink(entry['link'], target)
        else:
            fd = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
            with os.fdopen(fd, 'wb') as file:
                file.write(base64.b64decode(entry['data']))
            os.chown(target, entry['uid'], entry['gid'])
            os.chmod(target, entry['mode'])


def safe_accounts(saved):
    """New accounts are fine only as unprivileged system accounts without a login shell; otherwise put them back."""
    try:
        lines = Path('/etc/passwd').read_text().splitlines()
        old = {line.split(':')[0] for line in base64.b64decode(saved['/etc/passwd']['data']).decode().splitlines()}
    except (KeyError, OSError, ValueError):
        return False
    for line in lines:
        fields = line.split(':')
        if len(fields) != 7 or fields[0] in old:
            continue
        if fields[2] == '0' or fields[3] == '0' or not fields[6].endswith(('/nologin', '/false')):
            return False
    return True


def strip_privileges(marker):
    """No file changed during the install keeps setuid/setgid bits or file capabilities."""
    _, found = run(['/usr/bin/find', '/', '-xdev', '-type', 'f', '-cnewer', str(marker), '-print0'], 300, check=False)
    for path in found.split('\0'):
        if not path:
            continue
        try:
            info = os.lstat(path)
            if stat.S_ISREG(info.st_mode) and info.st_mode & (stat.S_ISUID | stat.S_ISGID):
                os.chmod(path, stat.S_IMODE(info.st_mode) & ~(stat.S_ISUID | stat.S_ISGID))
            os.removexattr(path, 'security.capability')
        except OSError:
            pass


def reapply_denials():
    for tool in DENIED_TOOLS:
        for directory in ('/usr/bin', '/usr/sbin', '/bin', '/sbin'):
            path = Path(directory) / tool
            if path.is_file() and not path.is_symlink():
                run(['/usr/bin/setfacl', '-m', f'u:{MODEL}:---', str(path)], 10, check=False)


def close_writable(model_uid, model_gid):
    """The worker refuses to start if the agent can write anywhere else on the system disk: close what a package
    opened (world-writable files and folders). Returns False if something belongs to the agent."""
    prune = []
    for path in AGENT_WRITABLE:
        prune += ['-path', path, '-prune', '-o']
    _, found = run(['/usr/bin/find', '/', '-xdev', *prune, '(', '-uid', str(model_uid), '-o', '-gid', str(model_gid), '-o', '(',
                    '!', '-type', 'l', '-perm', '-0002', ')', ')', '-print0'], 300, check=False)
    clean = True
    for path in found.split('\0'):
        if not path or path in AGENT_WRITABLE:
            continue
        try:
            info = os.lstat(path)
            if info.st_uid == model_uid or info.st_gid == model_gid:
                clean = False
                continue
            os.chmod(path, stat.S_IMODE(info.st_mode) & ~stat.S_IWOTH)
        except OSError:
            continue
    return clean


def place_policy():
    if POLICY.exists() or POLICY.is_symlink():
        require(not POLICY.is_symlink() and POLICY.read_text() == POLICY_TEXT, 'install_unsafe')
        return
    fd = os.open(POLICY, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o755)
    with os.fdopen(fd, 'w') as file:
        file.write(POLICY_TEXT)


def neutralise():
    """After any install (or a crash part-way): strip, restore, reapply, close; then forget the install."""
    import pwd
    model = pwd.getpwnam(MODEL)
    saved = json.loads((STATE / 'snapshot.json').read_text())
    strip_privileges(STATE / 'marker')
    restore(saved['sensitive'], SENSITIVE)
    if not safe_accounts(saved['accounts']):
        restore(saved['accounts'], ACCOUNTS)
    run(['/usr/sbin/ldconfig'], 120, check=False)
    reapply_denials()
    clean = close_writable(model.pw_uid, model.pw_gid)
    run(APT + ['clean'], 120, check=False)
    if POLICY.exists() and not POLICY.is_symlink() and POLICY.read_text() == POLICY_TEXT:
        POLICY.unlink()
    (STATE / 'pending').unlink(missing_ok=True)
    return clean


def protected(argv, timeout, now):
    """Runs one dpkg-changing command with services refused, then always neutralises. (status, clean)"""
    # From here a crash or power-off is finished by `recover` before the worker next starts.
    (STATE / 'marker').write_text(str(now))
    write_state('snapshot.json', json.dumps({'sensitive': snapshot(SENSITIVE), 'accounts': snapshot(ACCOUNTS)}))
    write_state('pending', 'install')
    place_policy()
    status = 1
    try:
        status, _ = run(argv, timeout, check=False)
    finally:
        clean = neutralise()
    return status, clean


def install(names, now=None):
    now = time.time() if now is None else now
    times = recent(now)
    require(len(times) < PER_HOUR, 'too_many_installs')
    lock = os.open(LOCK, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    try:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            raise Refused('install_busy') from None
        write_state('history.json', json.dumps(times + [now]))
        if (STATE / 'pending').exists():
            neutralise()
        # An install a crash interrupted: finish configuring it (its packages were inspected when it began).
        _, audit = run(['/usr/bin/dpkg', '--audit'], 60, check=False)
        if audit.strip():
            protected(['/usr/bin/dpkg', '--force-confdef', '--force-confold', '--configure', '-a'], TIMES['install'], now)
        run(APT + ['update'], TIMES['update'])
        _, simulation = run(APT + ['-s', 'install', '--no-remove', '--'] + names, TIMES['simulate'])
        new, upgraded, removed = plan(simulation)
        check_plan(new, upgraded, removed)
        if not new and not upgraded:
            return {'result': 'already_installed', 'packages': names}
        _, uris = run(APT + ['--print-uris', '-qq', 'install', '--no-remove', '--'] + names, TIMES['uris'])
        debs = deb_files(uris)
        require(len(debs) == len(new) + len(upgraded), 'apt_failed')
        fresh = set(new)
        run(APT + ['-y', '--download-only', 'install', '--no-remove', '--'] + names, TIMES['download'])
        installed = files = 0
        for deb in debs:
            size, count = inspect(ARCHIVES / deb, deb.split('_')[0] in fresh)
            installed, files = installed + size, files + count
        free = os.statvfs('/')
        require(installed <= MAX_INSTALLED and free.f_bavail * free.f_frsize >= MIN_FREE + installed
                and free.f_favail >= MIN_INODES + files, 'too_large')
        status, clean = protected(APT + ['-y', '--no-remove', '-o', 'Dpkg::Options::=--force-confdef', '-o', 'Dpkg::Options::=--force-confold',
                                         'install', '--'] + names, TIMES['install'], now)
        require(clean, 'install_unsafe')
        require(status == 0, 'apt_failed')
        return {'result': 'installed', 'packages': names, 'new': sorted(new), 'upgraded': sorted(upgraded)}
    finally:
        os.close(lock)


def recover():
    """Run as root before the worker starts: finish neutralising an install a crash or power-off interrupted."""
    if (STATE / 'pending').exists():
        neutralise()


def peer_uid(connection):
    creds = connection.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, struct.calcsize('3i'))
    return struct.unpack('3i', creds)[1]


def main():
    import pwd
    connection = socket.socket(fileno=os.dup(0))
    try:
        require(peer_uid(connection) == pwd.getpwnam(MODEL).pw_uid, 'not_allowed')
        connection.settimeout(10)
        line = b''
        while b'\n' not in line and len(line) <= 4096:
            chunk = connection.recv(4097 - len(line))
            if not chunk:
                break
            line += chunk
        require(len(line) <= 4096, 'request_invalid')
        answer = install(request(line.split(b'\n')[0].decode('utf-8', 'replace')))
    except Refused as refused:
        answer = {'result': 'refused', 'code': refused.code}
    except Exception as error:
        answer = {'result': 'refused', 'code': 'internal_error', 'error': type(error).__name__}
    try:
        connection.sendall((json.dumps(answer) + '\n').encode())
    except OSError:
        pass
    finally:
        connection.close()
    print(answer['result'], answer.get('code', ''), answer.get('error', ''), file=sys.stderr)


if __name__ == '__main__':
    if sys.argv[1:] == ['recover']:
        recover()
    else:
        main()
