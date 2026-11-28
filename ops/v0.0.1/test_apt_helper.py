"""The package helper's decisions, with a stand-in for apt (real .debs and installs run in a Linux check)."""
import base64
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('apt_helper', Path(__file__).with_name('apt-helper.py'))
a = importlib.util.module_from_spec(spec)
spec.loader.exec_module(a)

SIMULATION = """NOTE: This is only a simulation!
Inst libfoo1 (1.0-1 Ubuntu:24.04/noble [amd64])
Inst ffmpeg (6.1-1 Ubuntu:24.04/noble [amd64])
Conf libfoo1 (1.0-1 Ubuntu:24.04/noble [amd64])
"""
URIS = "'http://archive.ubuntu.com/ubuntu/pool/f/ffmpeg.deb' ffmpeg_6.1-1_amd64.deb 1048576 SHA256:ab\n'http://x/libfoo1.deb' libfoo1_1.0-1_amd64.deb 2048 SHA256:cd\n"


class Decisions(unittest.TestCase):
    def test_only_plain_allowed_names(self):
        self.assertEqual(a.request('{"packages":["ffmpeg","imagemagick"]}'), ['ffmpeg', 'imagemagick'])
        for bad in ['', 'x', '{}', '{"packages":[]}', '{"packages":"ffmpeg"}', '{"packages":["ffmpeg"],"extra":1}',
                    '{"packages":["-o","x"]}', '{"packages":["ffmpeg=1.0"]}', '{"packages":["ffmpeg/noble"]}', '{"packages":["../x"]}',
                    '{"packages":["FFMPEG"]}', '{"packages":["ffmpeg","ffmpeg"]}', '{"packages":["ffmpeg:amd64"]}',
                    json.dumps({'packages': [f'p{i}x' for i in range(21)]})]:
            with self.subTest(bad=bad), self.assertRaises(a.Refused) as caught:
                a.request(bad)
            self.assertEqual(caught.exception.code, 'request_invalid')
        for name in ['sudo', 'sudo-rs', 'opendoas', 'docker.io', 'linux-image-generic', 'systemd-container', 'openssh-server', 'postfix',
                     'apt', 'snapd', 'nftables', 'libpam-foo', 'libnss-mdns', 'snoopy', 'auto-apt-proxy', 'tinysshd', 'foo-dkms', 'policyrcd-script-zg2']:
            with self.subTest(name=name), self.assertRaises(a.Refused) as caught:
                a.request(json.dumps({'packages': ['ffmpeg', name]}))
            self.assertEqual(caught.exception.code, 'package_not_allowed')

    def test_the_dry_run_plan(self):
        self.assertEqual(a.plan(SIMULATION), (['libfoo1', 'ffmpeg'], [], []))
        upgrade = 'Inst libssl3t64 [3.0.13-0ubuntu3] (3.0.13-0ubuntu3.4 Ubuntu:24.04/noble-updates [amd64])\nRemv nano [7.2-2]\n'
        self.assertEqual(a.plan(upgrade), ([], ['libssl3t64'], ['nano']))
        a.check_plan(['ffmpeg'], ['libssl3t64'], [])
        # A toolchain upgrades installed libraries (libc6 matching libc6-dev) and needs dpkg-dev and linux-libc-dev.
        a.check_plan(['gcc', 'dpkg-dev', 'linux-libc-dev', 'libc6-dev'], ['libc6', 'libc-bin', 'python3.12'], [])
        for args, code in [((['ffmpeg'], [], ['nano']), 'would_remove_packages'), ((['systemd-timesyncd'], [], []), 'package_not_allowed'),
                           ((['ffmpeg'], ['sudo'], []), 'would_upgrade_protected'), ((['ffmpeg'], ['libsystemd0'], []), 'would_upgrade_protected'),
                           ((['dpkg-dev', 'sudo'], [], []), 'package_not_allowed')]:
            with self.subTest(args=args), self.assertRaises(a.Refused) as caught:
                a.check_plan(*args)
            self.assertEqual(caught.exception.code, code)
        self.assertEqual(a.deb_files(URIS), ['ffmpeg_6.1-1_amd64.deb', 'libfoo1_1.0-1_amd64.deb'])

    def test_systemctl_only_to_reload_or_ask(self):
        self.assertIsNone(a.SYSTEMCTL.search('if [ -d /run/systemd/system ]; then systemctl --system daemon-reload >/dev/null || true; fi'))
        self.assertIsNone(a.SYSTEMCTL.search('systemctl is-enabled foo.service'))
        for line in ['systemctl start foo', 'systemctl enable --now foo', 'systemctl restart foo']:
            with self.subTest(line=line):
                self.assertIsNotNone(a.SYSTEMCTL.search(line))

    def test_what_a_package_may_not_place(self):
        for path in ['./lib/systemd/system/multi-user.target.wants/x.service', './usr/lib/systemd/system/sockets.target.wants/x.socket',
                     './usr/lib/systemd/system-generators/x', './etc/systemd/system/multi-user.target.wants/x', './etc/rc2.d/S01x',
                     './etc/cron.d/x', './etc/apt/apt.conf.d/01proxy', './etc/pam.d/x', './etc/sudoers.d/x', './etc/ld.so.preload',
                     './etc/ld.so.conf.d/x.conf', './etc/nsswitch.conf', './usr/sbin/policy-rc.d', './usr/lib/tmpfiles.d/x.conf',
                     './etc/udev/rules.d/x', './lib/modules/x/y.ko', './usr/lib/sysctl.d/x', './etc/ssh/sshd_config.d/x',
                     './etc/dpkg/dpkg.cfg.d/x', './usr/share/initramfs-tools/hooks/x']:
            with self.subTest(path=path):
                self.assertTrue(a.forbidden_path(path))
        # Service files may sit on disk inert: the restore removes any enable link a package adds.
        for path in ['./usr/bin/ffmpeg', './usr/lib/x86_64-linux-gnu/libfoo.so.1', './usr/share/doc/ffmpeg/copyright',
                     './usr/lib/systemd/user/x.service', './etc/ImageMagick-6/policy.xml', './usr/share/man/man1/jq.1.gz',
                     './lib/systemd/system/redis-server.service', './etc/init.d/x11-common', './etc/default/redis', './etc/dpkg/shlibs.default']:
            with self.subTest(path=path):
                self.assertFalse(a.forbidden_path(path))


class Installs(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='craxii-apt-')
        root = Path(self.temp.name)
        self.calls, self.policy_seen = [], []
        self.simulation, self.install_status = SIMULATION, 0
        self.patches = [patch.object(a, 'STATE', root), patch.object(a, 'LOCK', root / 'lock'), patch.object(a, 'POLICY', root / 'policy-rc.d'),
                        patch.object(a, 'SENSITIVE', []), patch.object(a, 'ACCOUNTS', []), patch.object(a, 'run', side_effect=self.run_),
                        patch.object(a, 'inspect', return_value=(1024, 10)), patch.object(a, 'strip_privileges'), patch.object(a, 'reapply_denials'),
                        patch.object(a, 'close_writable', return_value=True), patch.object(a, 'safe_accounts', return_value=True),
                        patch('pwd.getpwnam', return_value=type('P', (), {'pw_uid': 997, 'pw_gid': 997})())]
        for p in self.patches:
            p.start()

    def tearDown(self):
        for p in self.patches:
            p.stop()
        self.temp.cleanup()

    def run_(self, argv, timeout, check=True):
        self.calls.append(argv)
        if argv[:2] == ['/usr/bin/dpkg', '--audit']:
            return 0, ''
        if '-s' in argv:
            return 0, self.simulation
        if '--print-uris' in argv:
            return 0, URIS
        if argv[-len(['install', '--', 'ffmpeg']):-1] == ['install', '--'] and '--download-only' not in argv:
            self.policy_seen.append(a.POLICY.read_text() if a.POLICY.exists() else None)
            if self.install_status == 'timeout':
                raise a.Refused('apt_timed_out')
            return self.install_status, ''
        return 0, ''

    def test_installs_with_services_refused_then_neutralises(self):
        out = a.install(['ffmpeg'], now=1000.0)
        self.assertEqual((out['result'], out['new']), ('installed', ['ffmpeg', 'libfoo1']))
        self.assertEqual(self.policy_seen, [a.POLICY_TEXT], 'services refused during the install')
        self.assertFalse(a.POLICY.exists(), 'removed afterwards')
        self.assertFalse((a.STATE / 'pending').exists(), 'finished')
        a.strip_privileges.assert_called_once_with(a.STATE / 'marker')
        a.reapply_denials.assert_called_once()
        a.close_writable.assert_called_once_with(997, 997)
        self.assertEqual(a.inspect.call_count, 2, 'every .deb read before installing')
        install = [c for c in self.calls if c[-3:-1] == ['install', '--'] and '--download-only' not in c and '-s' not in c and '--print-uris' not in c]
        self.assertEqual(install[0][-2:], ['--', 'ffmpeg'], 'names after --, never as options')
        self.assertIn('--no-remove', install[0])

    def test_refusals_install_nothing(self):
        for simulation, code in [('Inst ffmpeg (1)\nRemv nano [7.2-2]\n', 'would_remove_packages'), ('Inst sudo [1.9] (1.9.1)\n', 'would_upgrade_protected'), ('Inst sudo (1.9.1)\n', 'package_not_allowed'),
                                 ('Inst libsystemd0 [255.4] (255.5)\n', 'would_upgrade_protected')]:
            with self.subTest(code=code):
                self.simulation = simulation
                with self.assertRaises(a.Refused) as caught:
                    a.install(['ffmpeg'], now=2000.0 + len(self.calls))
                self.assertEqual(caught.exception.code, code)
        self.simulation = SIMULATION
        with patch.object(a, 'inspect', side_effect=a.Refused('package_not_allowed')), self.assertRaises(a.Refused) as caught:
            a.install(['ffmpeg'], now=2500.0)
        self.assertEqual(caught.exception.code, 'package_not_allowed')
        self.assertEqual(self.policy_seen, [], 'nothing was installed')

    def test_a_failed_or_timed_out_install_is_still_neutralised(self):
        for status, code in [(100, 'apt_failed'), ('timeout', 'apt_timed_out')]:
            with self.subTest(status=status):
                self.install_status = status
                a.strip_privileges.reset_mock(); a.close_writable.reset_mock()
                with self.assertRaises(a.Refused) as caught:
                    a.install(['ffmpeg'], now=3000.0 + len(self.calls))
                self.assertEqual(caught.exception.code, code)
                a.strip_privileges.assert_called_once()
                a.close_writable.assert_called_once()
                self.assertFalse(a.POLICY.exists())
                self.assertFalse((a.STATE / 'pending').exists())

    def test_an_interrupted_install_is_finished_first_and_a_foreign_policy_refused(self):
        (a.STATE / 'snapshot.json').write_text(json.dumps({'sensitive': {}, 'accounts': {}}))
        (a.STATE / 'marker').write_text('1')
        (a.STATE / 'pending').write_text('install')
        a.recover()
        self.assertFalse((a.STATE / 'pending').exists())
        a.strip_privileges.assert_called_once()
        a.POLICY.write_text('#!/bin/sh\nexit 0\n')
        with self.assertRaises(a.Refused) as caught:
            a.install(['ffmpeg'], now=4000.0)
        self.assertEqual(caught.exception.code, 'install_unsafe')
        self.assertEqual(self.policy_seen, [])

    def test_a_few_an_hour_one_at_a_time_and_sized(self):
        for i in range(a.PER_HOUR):
            a.install(['ffmpeg'], now=5000.0 + i)
        with self.assertRaises(a.Refused) as caught:
            a.install(['ffmpeg'], now=5100.0)
        self.assertEqual(caught.exception.code, 'too_many_installs')
        with patch.object(a, 'inspect', return_value=(a.MAX_INSTALLED, 1)), self.assertRaises(a.Refused) as caught:
            a.install(['ffmpeg'], now=9000.0)
        self.assertEqual(caught.exception.code, 'too_large')
        import fcntl
        held = os.open(a.LOCK, os.O_RDWR | os.O_CREAT)
        fcntl.flock(held, fcntl.LOCK_EX)
        try:
            with self.assertRaises(a.Refused) as caught:
                a.install(['ffmpeg'], now=20000.0)
            self.assertEqual(caught.exception.code, 'install_busy')
        finally:
            os.close(held)


class Restore(unittest.TestCase):
    def test_sensitive_files_go_back_and_new_ones_go(self):
        with tempfile.TemporaryDirectory() as root:
            conf = Path(root, 'apt'); conf.mkdir(); (conf / 'sources.list').write_text('archive')
            saved = a.snapshot([str(conf)])
            (conf / 'sources.list').write_text('attacker')
            (conf / 'proxy.conf').write_text('Acquire::http::Proxy "http://127.0.0.1:3142";')
            os.symlink('/etc/passwd', conf / 'link')
            a.restore(saved, [str(conf)])
            self.assertEqual(sorted(p.name for p in conf.iterdir()), ['sources.list'])
            self.assertEqual((conf / 'sources.list').read_text(), 'archive')

    def test_new_accounts_must_be_unprivileged_without_a_shell(self):
        with tempfile.TemporaryDirectory() as root:
            passwd = Path(root, 'passwd')
            passwd.write_text('root:x:0:0::/root:/bin/bash\n')
            saved = {'/etc/passwd': {'data': base64.b64encode(passwd.read_bytes()).decode()}}
            real = Path.read_text
            for added, ok in [('_svc:x:120:120::/nonexistent:/usr/sbin/nologin\n', True), ('evil:x:0:0::/:/usr/sbin/nologin\n', False),
                              ('shell:x:121:121::/:/bin/bash\n', False)]:
                with self.subTest(added=added):
                    content = 'root:x:0:0::/root:/bin/bash\n' + added
                    with patch.object(Path, 'read_text', lambda self, *x, **k: content if str(self) == '/etc/passwd' else real(self, *x, **k)):
                        self.assertEqual(a.safe_accounts(saved), ok)


if __name__ == '__main__':
    unittest.main()
