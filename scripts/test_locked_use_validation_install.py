import copy
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('installer', Path(__file__).with_name('locked-use-validation-install.py'))
installer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(installer)


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        root = Path(self.temp.name)
        self.package = root / 'package'
        self.package.mkdir()
        self.database = {installer.SYSTEM_RIGHT: {'class': 'rule', 'k-of-n': 1,
            'rule': ['com.openai.sky.CUAService.AuthorizationPlugin.remote', 'use-login-window-ui'],
            'comment': 'original', 'created': 1.0, 'modified': 2.0}}
        self.before = copy.deepcopy(self.database)
        values = {'ROOT': root / 'installed', 'BACKUP': root / 'backup',
                  'PLIST': root / 'daemon.plist', 'BROKER': root / 'broker'}
        values['CODE'] = [(name, values['ROOT'] / name, identity) for name, _, identity in installer.CODE]
        for name, _, _ in values['CODE']:
            (self.package / name).write_text('signed fixture')
        self.code = values['CODE']
        self.writes = []

        def write(name, value):
            self.writes.append(name)
            self.database[name] = copy.deepcopy(value)

        def command(*args, **kwargs):
            if args[:3] == ('/usr/bin/security', 'authorizationdb', 'remove'):
                self.database.pop(args[3], None)
            return subprocess.CompletedProcess(args, 1 if args[0] == '/usr/bin/pgrep' else 0, b'', b'')

        values.update(read_right=lambda name: copy.deepcopy(self.database.get(name)), write_right=write,
                      command=command, verify=lambda *args: None, root_permissions=lambda path: None,
                      secure_parent=lambda path: path.mkdir(parents=True, exist_ok=True))
        for name, value in values.items():
            mocked = patch.object(installer, name, value)
            mocked.start()
            self.addCleanup(mocked.stop)

    def test_install_remove_preserves_codex_and_later_third_party_rule(self):
        installer.install(self.package)
        self.assertEqual(self.database[installer.SYSTEM_RIGHT]['rule'][1:], self.before[installer.SYSTEM_RIGHT]['rule'])
        self.database[installer.SYSTEM_RIGHT]['rule'].append('another.plugin')
        installer.uninstall()
        self.assertEqual(self.database[installer.SYSTEM_RIGHT]['rule'], [*self.before[installer.SYSTEM_RIGHT]['rule'], 'another.plugin'])
        self.assertNotIn(installer.RIGHT, self.database)
        self.assertTrue(all(not path.exists() for _, path, _ in self.code))

    def test_signature_failure_rolls_back_staged_files_without_auth_changes(self):
        verified = []

        def verify(path, identity):
            verified.append(identity)
            if len(verified) == 3:
                raise ValueError('signature mismatch')

        with patch.object(installer, 'verify', verify), self.assertRaises(ValueError):
            installer.install(self.package)
        self.assertEqual(self.database, self.before)
        self.assertFalse(self.writes)
        self.assertTrue(all(not path.exists() for _, path, _ in self.code))

    def test_registration_failure_rolls_back_without_removing_other_rules(self):
        write = installer.write_right

        def fail_system_write(name, value):
            if name == installer.SYSTEM_RIGHT:
                raise RuntimeError('write failed')
            write(name, value)

        with patch.object(installer, 'write_right', fail_system_write), self.assertRaises(RuntimeError):
            installer.install(self.package)
        self.assertEqual(self.database, self.before)
        self.assertTrue(all(not path.exists() for _, path, _ in self.code))

    def test_rejects_unknown_shape_existing_right_and_symlink(self):
        for bad in ({'class': 'allow'}, {'class': 'rule', 'k-of-n': 2, 'rule': ['x']},
                    {'class': 'rule', 'k-of-n': 1, 'rule': 'x'}):
            with self.assertRaises(ValueError):
                installer.with_alternative(bad, True)
        self.database[installer.RIGHT] = {'class': 'deny'}
        with self.assertRaises(ValueError):
            installer.install(self.package)
        path = self.package / 'symlink'
        path.symlink_to(self.package / self.code[0][0])
        with self.assertRaises(ValueError):
            installer.regular_tree(path)

    def test_will_not_remove_changed_own_right(self):
        installer.install(self.package)
        self.database[installer.RIGHT]['mechanisms'] = ['something:else']
        with self.assertRaises(ValueError):
            installer.uninstall()
        self.assertTrue(all(path.exists() for _, path, _ in self.code))
        self.assertEqual(self.database[installer.SYSTEM_RIGHT]['rule'], self.before[installer.SYSTEM_RIGHT]['rule'])


if __name__ == '__main__':
    unittest.main()
