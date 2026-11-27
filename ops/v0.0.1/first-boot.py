#!/usr/bin/python3 -I
"""A new customer VM's first boot, as root (B6.6; decisions 37, 40, 43).

`kit` (craxii-first-boot.service, before the watchdog): reads the launch's user data and AWS's signed identity from
the metadata service (only root may reach it), trades them at the control plane's first-boot address for the VM's
starting kit and a one-use enrollment credential, writes the four kit files with the owners and modes their readers
check, and leaves the enrollment handoff for the worker.

`credential` (craxii-first-boot-credential.service, just before the worker): asks again for a fresh credential (the
first one lasts 5 minutes and disk setup can take longer), checks the kit is unchanged and replaces the handoff.

Once the worker is bound both do nothing (`credential` removes a leftover handoff). Failures print a code only.
"""
import base64
import grp
from http.client import HTTPException
import json
import os
from pathlib import Path
import pwd
import re
import ssl
import stat
import sys
import time
import urllib.error
import urllib.request

IMDS = 'http://169.254.169.254'
GUEST = Path('/etc/craxii/guest-bootstrap.json')
WATCHDOG = Path('/etc/craxii/compute-watchdog.json')
PERMIT = Path('/var/lib/craxii-compute/bootstrap.json')
IDENTITY = Path('/var/lib/craxii-worker/identity')
WORKER = 'craxii-server'
# The control plane may still be recording the launch when the VM first boots; it answers 401 until then.
DEADLINE_SECONDS = 600
UUID4 = re.compile(r'^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$')
TOKEN = re.compile(r'^[A-Za-z0-9_-]{43}$')
ORIGIN = re.compile(r'^https://[a-z0-9.-]+$')


class Failure(Exception):
    def __init__(self, code):
        super().__init__(code)
        self.code = code


def require(condition, code):
    if not condition:
        raise Failure(code)


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        raise Failure('first_boot_redirected')


# No proxy from the environment and no redirects; HTTPS verifies the certificate against the system's roots.
OPENER = urllib.request.build_opener(
    urllib.request.ProxyHandler({}), NoRedirect(), urllib.request.HTTPSHandler(context=ssl.create_default_context()))


def http(method, url, headers=None, body=None, limit=65536, timeout=10):
    """(status, body) for one request; raises OSError-like errors on transport failure."""
    request = urllib.request.Request(url, data=body, headers=headers or {}, method=method)
    try:
        with OPENER.open(request, timeout=timeout) as response:
            data = response.read(limit + 1)
            status = response.status
    except urllib.error.HTTPError as error:
        data, status = error.read(limit + 1), error.code
    require(len(data) <= limit, 'first_boot_response_invalid')
    return status, data


def metadata():
    """The launch's user data and AWS's identity document and signature (IMDSv2)."""
    status, token = http('PUT', f'{IMDS}/latest/api/token', {'X-aws-ec2-metadata-token-ttl-seconds': '300'}, b'', 256, 2)
    if status != 200 or not token:
        return None
    def get(path, limit):
        code, data = http('GET', f'{IMDS}{path}', {'X-aws-ec2-metadata-token': token.decode('ascii')}, None, limit, 2)
        if code != 200:
            raise ValueError('metadata not ready')
        return data
    return get('/latest/user-data', 4096), get('/latest/dynamic/instance-identity/document', 9000), \
        get('/latest/dynamic/instance-identity/signature', 1024)


def launch(user_data):
    """The fleet's user data: exactly {contract_version, installation_id, launch_nonce, api_origin}."""
    try:
        value = json.loads(user_data, object_pairs_hook=unique)
    except (ValueError, UnicodeDecodeError):
        raise Failure('user_data_invalid') from None
    require(isinstance(value, dict) and set(value) == {'contract_version', 'installation_id', 'launch_nonce', 'api_origin'}
            and value['contract_version'] == 1 and type(value['contract_version']) is int, 'user_data_invalid')
    require(all(isinstance(value[k], str) for k in ('installation_id', 'launch_nonce', 'api_origin')), 'user_data_invalid')
    require(UUID4.match(value['installation_id']) and TOKEN.match(value['launch_nonce']) and ORIGIN.match(value['api_origin']),
            'user_data_invalid')
    return value


def unique(pairs):
    keys = [k for k, _ in pairs]
    if len(keys) != len(set(keys)):
        raise ValueError('duplicate key')
    return dict(pairs)


def signature_bytes(text):
    """The metadata service's signature: base64 text, with line breaks."""
    try:
        raw = base64.b64decode(b''.join(text.split()), validate=True)
    except ValueError:
        raise Failure('metadata_unavailable') from None
    require(0 < len(raw) <= 512, 'metadata_unavailable')
    return raw


def retrying(attempt, seconds, code):
    """attempt() until it returns something other than None; transport errors and None are retried until the deadline."""
    end, delay = time.monotonic() + seconds, 1
    while True:
        try:
            result = attempt()
            if result is not None:
                return result
        except (OSError, ValueError, HTTPException):
            pass
        require(time.monotonic() + delay < end, code)
        time.sleep(delay)
        delay = min(delay * 2, 30)


def first_boot(origin, request, denied_is_final=False):
    """The control plane's answer, retried until it answers 200 or the deadline passes. 401 is retried while the fleet
    may still be recording the launch (kit); once the kit is in place it is final (the launch is no longer awaiting
    its first enrollment)."""
    body = json.dumps(request, separators=(',', ':')).encode()
    def attempt():
        status, data = http('POST', f'{origin}/v1/worker/first-boot', {'content-type': 'application/json'}, body)
        # 400 means this VM's request is malformed: retrying can't help.
        require(status != 400, 'first_boot_refused')
        require(not (denied_is_final and status == 401), 'first_boot_denied')
        return data if status == 200 else None
    return retrying(attempt, DEADLINE_SECONDS, 'first_boot_unavailable')


def answer(data, installation_id):
    try:
        value = json.loads(data, object_pairs_hook=unique)
    except (ValueError, UnicodeDecodeError):
        raise Failure('first_boot_response_invalid') from None
    require(isinstance(value, dict) and set(value) == {'contract_version', 'installation_id', 'enrollment_id',
            'enrollment_credential', 'expected_instance_ref', 'kit'}, 'first_boot_response_invalid')
    kit = value['kit']
    require(value['contract_version'] == 1 and value['installation_id'] == installation_id
            and isinstance(value['enrollment_id'], str) and UUID4.match(value['enrollment_id'])
            and isinstance(value['enrollment_credential'], str) and TOKEN.match(value['enrollment_credential'])
            and isinstance(value['expected_instance_ref'], str) and UUID4.match(value['expected_instance_ref'])
            and isinstance(kit, dict) and set(kit) == {'guest_bootstrap', 'distribution', 'compute_watchdog', 'bootstrap_permit'},
            'first_boot_response_invalid')
    for name, text in kit.items():
        require(isinstance(text, str) and 0 < len(text.encode()) <= 16384, 'first_boot_response_invalid')
        try:
            json.loads(text, object_pairs_hook=unique)
        except ValueError:
            raise Failure('first_boot_response_invalid') from None
    require(json.loads(kit['guest_bootstrap']).get('installation_id') == installation_id, 'first_boot_response_invalid')
    return value


def owners():
    worker = pwd.getpwnam(WORKER)
    return worker.pw_uid, grp.getgrnam(WORKER).gr_gid


def kit_files(kit):
    """Each kit file: (path, bytes, owner uid, group gid, mode), as the watchdog, disk setup and worker check them."""
    uid, gid = owners()
    return [
        (GUEST, kit['guest_bootstrap'].encode(), 0, gid, 0o640),
        (WATCHDOG, kit['compute_watchdog'].encode(), 0, gid, 0o640),
        (PERMIT, kit['bootstrap_permit'].encode(), 0, gid, 0o600),
        (IDENTITY / 'distribution.json', kit['distribution'].encode(), uid, gid, 0o600),
    ]


def read_owned(path, uid, mode, limit, code):
    """A file's bytes, or None when absent. Only a plain, single-link file of this owner and mode is read: never
    through a link, never blocking (the worker owns its folder, root reads it)."""
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
    except FileNotFoundError:
        return None
    except OSError:
        raise Failure(code) from None
    try:
        info = os.fstat(fd)
        require(stat.S_ISREG(info.st_mode) and info.st_uid == uid and stat.S_IMODE(info.st_mode) == mode and info.st_nlink == 1
                and info.st_size <= limit, code)
        data = b''
        while len(data) <= limit:
            chunk = os.read(fd, limit + 1 - len(data))
            if not chunk:
                break
            data += chunk
        require(len(data) <= limit, code)
        return data
    finally:
        os.close(fd)


def existing(path, uid, mode):
    """A placed file's bytes, or None; anything other than our plain file is refused."""
    return read_owned(path, uid, mode, 32768, 'kit_conflict')


def place(path, data, uid, gid, mode, replace=False):
    """Writes the file in one step (a new file renamed into place), unless the same bytes are already there."""
    parent = os.lstat(path.parent)
    require(stat.S_ISDIR(parent.st_mode) and parent.st_uid in (0, uid) and not parent.st_mode & 0o002, 'kit_conflict')
    old = existing(path, uid, mode)
    if old == data:
        return
    require(old is None or replace, 'kit_conflict')
    temp = path.parent / f'.{path.name}.first-boot'
    try:
        os.unlink(temp)
    except FileNotFoundError:
        pass
    fd = os.open(temp, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    try:
        os.fchown(fd, uid, gid)
        os.fchmod(fd, mode)
        os.write(fd, data)
        os.fsync(fd)
    finally:
        os.close(fd)
    os.replace(temp, path)
    directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


def disposition():
    """The worker's identity state (unbound, enrolling, bound, ...), or None before it has one."""
    data = read_owned(IDENTITY / 'identity.json', owners()[0], 0o600, 16384, 'identity_invalid')
    if data is None:
        return None
    try:
        value = json.loads(data)
    except ValueError:
        raise Failure('identity_invalid') from None
    require(isinstance(value, dict) and isinstance(value.get('disposition'), str), 'identity_invalid')
    return value['disposition']


def run(mode):
    require(mode in ('kit', 'credential'), 'usage')
    handoff = IDENTITY / 'enrollment.json'
    state = disposition()
    if state == 'bound':
        # Later boots get their running time when they are woken (B7 part 2), not here.
        if mode == 'credential' and handoff.exists():
            os.unlink(handoff)
        return 'already_enrolled'
    user_data, document, signature = retrying(metadata, 60, 'metadata_unavailable')
    launch_data = launch(user_data)
    signature = signature_bytes(signature)
    request = {'contract_version': 1, 'installation_id': launch_data['installation_id'], 'launch_nonce': launch_data['launch_nonce'],
               'instance_identity_document_b64': base64.b64encode(document).decode(),
               'instance_identity_signature_b64': base64.b64encode(signature).decode()}
    try:
        value = answer(first_boot(launch_data['api_origin'], request, denied_is_final=mode == 'credential'), launch_data['installation_id'])
    except Failure as failure:
        # The worker's enrollment landed but it didn't hear back: the control plane issues nothing more once bound.
        # The worker resends its exact proof with the handoff it already has, which needs no new credential.
        if mode == 'credential' and failure.code == 'first_boot_denied' and state == 'enrolling' \
                and existing(handoff, owners()[0], 0o600) is not None:
            return 'resume_pending'
        raise
    for path, data, uid, gid, file_mode in kit_files(value['kit']):
        if mode == 'kit':
            place(path, data, uid, gid, file_mode)
        else:
            require(existing(path, uid, file_mode) == data, 'kit_conflict')
    uid, gid = owners()
    material = {'contract_version': 1, 'enrollment_id': value['enrollment_id'], 'enrollment_credential': value['enrollment_credential'],
                'expected_instance_ref': value['expected_instance_ref'], 'instance_identity_document_b64': request['instance_identity_document_b64'],
                'instance_identity_signature_b64': request['instance_identity_signature_b64']}
    place(handoff, json.dumps(material, separators=(',', ':')).encode(), uid, gid, 0o600, replace=True)
    return 'ready'


if __name__ == '__main__':
    try:
        run(sys.argv[1] if len(sys.argv) == 2 else '')
    except Failure as failure:
        print(f'first_boot_failed {failure.code}', file=sys.stderr)
        sys.exit(1)
    except Exception:
        print('first_boot_failed internal_error', file=sys.stderr)
        sys.exit(1)
