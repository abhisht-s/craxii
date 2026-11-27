"""The first-boot program against a stand-in metadata service and control plane, in a temporary root."""
import base64
import copy
import importlib.util
import json
import os
from pathlib import Path
import stat
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('first_boot', Path(__file__).with_name('first-boot.py'))
f = importlib.util.module_from_spec(spec)
spec.loader.exec_module(f)
REAL_KIT_FILES = f.kit_files

INSTALLATION = '3f1c5a2e-9b7d-4e41-8c2a-6d0f4b8e1a96'
USER_DATA = {'contract_version': 1, 'installation_id': INSTALLATION, 'launch_nonce': 'N' * 43, 'api_origin': 'https://api.craxii.test'}
DOCUMENT = b'{"instanceId":"i-0123456789abcdef0","accountId":"123456789012"}'
SIGNATURE = bytes(range(128))
KIT = {'guest_bootstrap': json.dumps({'installation_id': INSTALLATION, 'volume_id': 'vol-0'}), 'distribution': '{"origin":"https://api.craxii.test/"}',
       'compute_watchdog': '{"contract_version":1}', 'bootstrap_permit': '{"permit":{},"key_id":"k","signature":"s"}'}


def answer(credential='C' * 43, kit=None):
    return {'contract_version': 1, 'installation_id': INSTALLATION, 'enrollment_id': '7f1c5a2e-9b7d-4e41-8c2a-6d0f4b8e1a93',
            'enrollment_credential': credential, 'expected_instance_ref': '1f1c5a2e-9b7d-4e41-8c2a-6d0f4b8e1a94', 'kit': kit or KIT}


class FirstBoot(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='craxii-first-boot-')
        root = Path(self.temp.name)
        for d in ('etc', 'compute', 'identity'):
            (root / d).mkdir(mode=0o750)
        os.chmod(root / 'identity', 0o700)
        self.paths = {'GUEST': root / 'etc/guest-bootstrap.json', 'WATCHDOG': root / 'etc/compute-watchdog.json',
                      'PERMIT': root / 'compute/bootstrap.json', 'IDENTITY': root / 'identity'}
        self.uid, self.gid = os.getuid(), os.getgid()
        self.user_data = json.dumps(USER_DATA).encode()
        self.answers = [(200, json.dumps(answer()).encode())]
        self.requests = []
        self.patches = [patch.object(f, name, value) for name, value in self.paths.items()]
        # The test runs as an ordinary user: "root" and the worker are both this user here.
        self.patches += [patch.object(f, 'owners', return_value=(self.uid, self.gid)), patch.object(f, 'http', side_effect=self.http),
                         patch.object(f.time, 'sleep'), patch.object(f, 'kit_files', side_effect=self.kit_files)]
        for p in self.patches:
            p.start()

    def tearDown(self):
        for p in self.patches:
            p.stop()
        self.temp.cleanup()

    def kit_files(self, kit):
        # This user stands in for root and the worker here (the real owners: test_kit_file_owners_and_modes).
        return [(path, data, self.uid, gid, mode) for path, data, _, gid, mode in REAL_KIT_FILES(kit)]

    def test_kit_file_owners_and_modes(self):
        with patch.object(f, 'owners', return_value=(1234, 5678)):
            files = REAL_KIT_FILES(KIT)
        self.assertEqual([(p, uid, gid, mode) for p, _, uid, gid, mode in files], [
            (self.paths['GUEST'], 0, 5678, 0o640), (self.paths['WATCHDOG'], 0, 5678, 0o640), (self.paths['PERMIT'], 0, 5678, 0o600),
            (self.paths['IDENTITY'] / 'distribution.json', 1234, 5678, 0o600)])

    def http(self, method, url, headers=None, body=None, limit=65536, timeout=10):
        self.requests.append((method, url, headers, body))
        if url.endswith('/latest/api/token'):
            return 200, b'token'
        if url.endswith('/latest/user-data'):
            return 200, self.user_data
        if url.endswith('/instance-identity/document'):
            return 200, DOCUMENT
        if url.endswith('/instance-identity/signature'):
            text = base64.b64encode(SIGNATURE)
            return 200, text[:64] + b'\n' + text[64:] + b'\n'
        self.assertEqual(url, 'https://api.craxii.test/v1/worker/first-boot')
        return self.answers.pop(0) if len(self.answers) > 1 else self.answers[0]

    def identity(self, disposition):
        path = self.paths['IDENTITY'] / 'identity.json'
        path.write_text(json.dumps({'disposition': disposition}))
        os.chmod(path, 0o600)

    def handoff(self):
        return json.loads((self.paths['IDENTITY'] / 'enrollment.json').read_bytes())

    def test_kit_mode_places_each_file_with_its_mode_and_leaves_the_handoff(self):
        self.assertEqual(f.run('kit'), 'ready')
        sent = json.loads([r for r in self.requests if r[0] == 'POST'][0][3])
        self.assertEqual(sent, {'contract_version': 1, 'installation_id': INSTALLATION, 'launch_nonce': 'N' * 43,
                                'instance_identity_document_b64': base64.b64encode(DOCUMENT).decode(),
                                'instance_identity_signature_b64': base64.b64encode(SIGNATURE).decode()})
        for path, mode in [(self.paths['GUEST'], 0o640), (self.paths['WATCHDOG'], 0o640), (self.paths['PERMIT'], 0o600),
                           (self.paths['IDENTITY'] / 'distribution.json', 0o600), (self.paths['IDENTITY'] / 'enrollment.json', 0o600)]:
            self.assertEqual(stat.S_IMODE(os.lstat(path).st_mode), mode, path.name)
        self.assertEqual(self.paths['GUEST'].read_text(), KIT['guest_bootstrap'])
        h = self.handoff()
        self.assertEqual(h['enrollment_credential'], 'C' * 43)
        self.assertEqual(base64.b64decode(h['instance_identity_document_b64']), DOCUMENT)
        self.assertEqual(base64.b64decode(h['instance_identity_signature_b64']), SIGNATURE)
        self.assertEqual(list(self.paths['IDENTITY'].glob('.*')), [], 'no temporary file left')

    def test_retries_until_the_launch_is_recorded_but_not_a_malformed_request(self):
        self.answers = [(401, b'{}'), (503, b'{}'), (200, json.dumps(answer()).encode())]
        self.assertEqual(f.run('kit'), 'ready')
        self.assertEqual(len([r for r in self.requests if r[0] == 'POST']), 3)
        self.tearDown(); self.setUp()
        self.answers = [(400, b'{}')]
        with self.assertRaises(f.Failure) as caught:
            f.run('kit')
        self.assertEqual(caught.exception.code, 'first_boot_refused')
        self.assertFalse(self.paths['GUEST'].exists())

    def test_gives_up_at_the_deadline(self):
        self.answers = [(401, b'{}')]
        clock = iter(range(0, 100000, 40))
        with patch.object(f.time, 'monotonic', side_effect=lambda: next(clock)):
            with self.assertRaises(f.Failure) as caught:
                f.run('kit')
        self.assertEqual(caught.exception.code, 'first_boot_unavailable')
        self.assertFalse(self.paths['GUEST'].exists())

    def test_credential_mode_refreshes_only_the_credential_and_refuses_a_changed_kit(self):
        f.run('kit')
        self.answers = [(200, json.dumps(answer('D' * 43)).encode())]
        self.assertEqual(f.run('credential'), 'ready')
        self.assertEqual(self.handoff()['enrollment_credential'], 'D' * 43)
        changed = dict(KIT, compute_watchdog='{"contract_version":2}')
        self.answers = [(200, json.dumps(answer('E' * 43, changed)).encode())]
        with self.assertRaises(f.Failure) as caught:
            f.run('credential')
        self.assertEqual(caught.exception.code, 'kit_conflict')
        self.assertEqual(self.handoff()['enrollment_credential'], 'D' * 43)
        self.assertEqual(self.paths['WATCHDOG'].read_text(), KIT['compute_watchdog'])

    def test_once_bound_nothing_is_asked_and_the_leftover_handoff_goes(self):
        f.run('kit')
        self.identity('bound')
        self.requests.clear()
        self.assertEqual(f.run('kit'), 'already_enrolled')
        self.assertEqual(f.run('credential'), 'already_enrolled')
        self.assertEqual(self.requests, [])
        self.assertFalse((self.paths['IDENTITY'] / 'enrollment.json').exists())

    def test_refuses_bad_user_data_answers_and_foreign_files(self):
        for change in ({'extra': 1}, {'api_origin': 'http://api.craxii.test'}, {'launch_nonce': 'short'}, {'contract_version': True}):
            with self.subTest(change=change):
                self.user_data = json.dumps(dict(USER_DATA, **change)).encode()
                with self.assertRaises(f.Failure) as caught:
                    f.run('kit')
                self.assertEqual(caught.exception.code, 'user_data_invalid')
        self.user_data = b'{"contract_version":1,"contract_version":1}'
        with self.assertRaises(f.Failure):
            f.run('kit')
        self.user_data = json.dumps(USER_DATA).encode()
        for bad in (dict(answer(), installation_id='4f1c5a2e-9b7d-4e41-8c2a-6d0f4b8e1a96'), dict(answer(), extra=1),
                    dict(answer(), kit=dict(KIT, guest_bootstrap='not json')), dict(answer(), enrollment_credential='x')):
            with self.subTest(answer=sorted(bad)):
                self.answers = [(200, json.dumps(bad).encode())]
                with self.assertRaises(f.Failure) as caught:
                    f.run('kit')
                self.assertEqual(caught.exception.code, 'first_boot_response_invalid')
        self.answers = [(200, json.dumps(answer()).encode())]
        self.paths['GUEST'].write_text('someone else')
        os.chmod(self.paths['GUEST'], 0o644)
        with self.assertRaises(f.Failure) as caught:
            f.run('kit')
        self.assertEqual(caught.exception.code, 'kit_conflict')
        self.assertEqual(self.paths['GUEST'].read_text(), 'someone else')

    def test_a_refusal_after_the_kit_is_final_and_a_landed_enrollment_keeps_its_handoff(self):
        f.run('kit')
        before = self.handoff()
        # The proof landed but the worker didn't hear back: it resends with the handoff it has.
        self.identity('enrolling')
        self.answers = [(401, b'{}')]
        self.requests.clear()
        self.assertEqual(f.run('credential'), 'resume_pending')
        self.assertEqual(len([r for r in self.requests if r[0] == 'POST']), 1, 'no retrying a final refusal')
        self.assertEqual(self.handoff(), before)
        # Not enrolling: refused at once, not after ten minutes.
        self.identity('unbound')
        with self.assertRaises(f.Failure) as caught:
            f.run('credential')
        self.assertEqual(caught.exception.code, 'first_boot_denied')

    def test_never_reads_the_worker_identity_through_a_link_or_a_special_file(self):
        target = Path(self.temp.name) / 'elsewhere.json'
        target.write_text('{"disposition":"bound"}')
        os.chmod(target, 0o600)
        (self.paths['IDENTITY'] / 'identity.json').symlink_to(target)
        with self.assertRaises(f.Failure) as caught:
            f.run('credential')
        self.assertEqual(caught.exception.code, 'identity_invalid')
        (self.paths['IDENTITY'] / 'identity.json').unlink()
        os.mkfifo(self.paths['IDENTITY'] / 'identity.json', 0o600)
        with self.assertRaises(f.Failure) as caught:
            f.run('credential')
        self.assertEqual(caught.exception.code, 'identity_invalid')


if __name__ == '__main__':
    unittest.main()
