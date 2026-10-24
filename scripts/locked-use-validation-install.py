#!/usr/bin/env python3
"""Local validation installer. Default is read-only --plan, not production setup.

Run --install/--uninstall with administrator privileges only after reviewing the
plan. A validation grant always requires fresh native owner authentication.
"""
import argparse
import copy
import json
import os
from pathlib import Path
import plistlib
import shutil
import stat
import subprocess
import sys

RIGHT = 'ai.snth.connect.LockedUse.AuthorizationPlugin.remote'
SYSTEM_RIGHT = 'system.login.screensaver'
LABEL = 'ai.snth.connect.locked-use-broker'
TEAM = 'CZAP38JFAE'
ROOT = Path('/Library/Application Support/SNTH Connect/Locked Use')
BACKUP = Path('/Library/Application Support/SNTH Connect/Locked Use Validation Backup')
PLUGIN = Path('/Library/Security/SecurityAgentPlugins/SNTHConnectAuthorizationPlugin.bundle')
BROKER = Path('/Library/PrivilegedHelperTools') / LABEL
PLIST = Path('/Library/LaunchDaemons') / (LABEL + '.plist')
GUARDIAN = ROOT / 'SNTHLockedUseGuardian.app'
VALIDATOR = ROOT / 'SNTHLockedUseValidation.app'
CODE = [
    ('SNTHConnectAuthorizationPlugin.bundle', PLUGIN, 'ai.snth.connect.LockedUse.AuthorizationPlugin'),
    (LABEL, BROKER, LABEL),
    ('SNTHLockedUseGuardian.app', GUARDIAN, 'ai.snth.connect.locked-use-guardian'),
    ('SNTHLockedUseValidation.app', VALIDATOR, 'ai.snth.connect.locked-use-validation'),
]


def command(*args, data=None, check=True):
    return subprocess.run(args, input=data, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                          check=check, timeout=20)


def read_right(name):
    result = command('/usr/bin/security', 'authorizationdb', 'read', name, check=False)
    if result.returncode:
        if b'-60005' in result.stderr:  # errAuthorizationDenied: nonexistent right
            return None
        raise RuntimeError('Cannot read authorization right: ' + name)
    return plistlib.loads(result.stdout)


def write_right(name, value):
    command('/usr/bin/security', 'authorizationdb', 'write', name, data=plistlib.dumps(value))


def with_alternative(original, adding):
    if not isinstance(original, dict) or original.get('class') != 'rule' or original.get('k-of-n') != 1:
        raise ValueError('Unexpected screensaver authorization rule; refusing to change it')
    rules = original.get('rule')
    if not isinstance(rules, list) or not rules or not all(isinstance(r, str) and r for r in rules):
        raise ValueError('Expected a nonempty list of existing authorization alternatives')
    changed = copy.deepcopy(original)
    if adding:
        if RIGHT in rules:
            raise ValueError('SNTH authorization alternative already exists')
        changed['rule'] = [RIGHT, *rules]
    else:
        changed['rule'] = [r for r in rules if r != RIGHT]
        if not changed['rule']:
            raise ValueError('Refusing to remove the only unlock alternative')
    return changed


def own_right():
    return {'class': 'evaluate-mechanisms', 'mechanisms': ['SNTHConnectAuthorizationPlugin:allow'],
            'shared': False, 'tries': 1, 'version': 1,
            'identifier': 'com.apple.security', 'requirement': 'identifier "com.apple.security" and anchor apple',
            'comment': 'SNTH local authenticated locked-use validation; production tasks disabled'}


def launchd_plist():
    return {'Label': LABEL, 'ProgramArguments': [str(BROKER)], 'UserName': 'root',
            'MachServices': {LABEL + suffix: True for suffix in ('', '.app', '.guardian', '.validation')},
            'ProcessType': 'Interactive', 'RunAtLoad': True, 'ThrottleInterval': 5}


def verify(path, identifier):
    requirement = f'anchor apple generic and identifier "{identifier}" and certificate leaf[subject.OU] = "{TEAM}"'
    command('/usr/bin/codesign', '--verify', '--strict', '-R', '=' + requirement, str(path))


def regular_tree(path):
    for entry in [path, *path.rglob('*')] if path.is_dir() else [path]:
        mode = entry.lstat().st_mode
        if not (stat.S_ISDIR(mode) or stat.S_ISREG(mode)):
            raise ValueError('Package contains a symlink or special file: ' + str(entry))


def secure_parent(path):
    """Refuse writable/symlink ancestors of all root-owned destinations."""
    for parent in reversed([path, *path.parents]):
        if not parent.exists():
            parent.mkdir(mode=0o755)
        st = parent.lstat()
        if not stat.S_ISDIR(st.st_mode) or st.st_uid != 0 or st.st_mode & 0o022:
            raise ValueError('Unsafe destination directory: ' + str(parent))


def root_permissions(path):
    for entry in [path, *path.rglob('*')] if path.is_dir() else [path]:
        st = entry.lstat()
        if not (stat.S_ISDIR(st.st_mode) or stat.S_ISREG(st.st_mode)):
            raise ValueError('Invalid staged file')
        os.chown(entry, 0, 0)
        os.chmod(entry, 0o755 if stat.S_ISDIR(st.st_mode) or st.st_mode & 0o111 else 0o644)


def remove_path(path):
    if path.is_symlink():
        raise ValueError('Refusing to follow replacement symlink: ' + str(path))
    if path.is_dir():
        shutil.rmtree(path)
    elif path.exists():
        path.unlink()


def equivalent(a, b):
    return {k: v for k, v in a.items() if k not in ('created', 'modified')} == {
        k: v for k, v in b.items() if k not in ('created', 'modified')}


def install(package):
    before = read_right(SYSTEM_RIGHT)
    replacement = with_alternative(before, True)
    if read_right(RIGHT) is not None:
        raise ValueError('An existing SNTH right must be reviewed before installing')
    for _, destination, _ in CODE:
        if destination.exists() or destination.is_symlink():
            raise ValueError('Refusing to overwrite existing installation: ' + str(destination))
        secure_parent(destination.parent)
    if PLIST.exists() or PLIST.is_symlink() or BACKUP.exists() or BACKUP.is_symlink():
        raise ValueError('Existing service or backup; review/uninstall first')
    secure_parent(PLIST.parent)
    secure_parent(BACKUP.parent)
    BACKUP.mkdir(mode=0o700)  # also an exclusive install lock
    (BACKUP / 'screensaver-before.plist').write_bytes(plistlib.dumps(before))
    installed = []
    right_written = rule_written = service_loaded = False
    try:
        stage = BACKUP / 'staging'
        stage.mkdir(mode=0o700)
        for name, destination, identifier in CODE:
            source = package / name
            regular_tree(source)
            staged = stage / name
            if source.is_dir():
                shutil.copytree(source, staged)
            else:
                shutil.copy2(source, staged)
            root_permissions(staged)
            verify(staged, identifier)  # check the private, root-owned copy
            staged.rename(destination)
            installed.append(destination)
        PLIST.write_bytes(plistlib.dumps(launchd_plist()))
        os.chmod(PLIST, 0o644)
        installed.append(PLIST)
        command('/bin/launchctl', 'bootstrap', 'system', str(PLIST))
        service_loaded = True
        if not equivalent(read_right(SYSTEM_RIGHT), before):
            raise ValueError('Screensaver rules changed during staging; retry after review')
        write_right(RIGHT, own_right())
        right_written = True
        write_right(SYSTEM_RIGHT, replacement)
        rule_written = True
        if RIGHT not in read_right(SYSTEM_RIGHT).get('rule', []):
            raise RuntimeError('Registration verification failed')
        (BACKUP / 'installed.json').write_text(json.dumps({'version': 1, 'validation_only': True}) + '\n')
        print('Validation components installed. Production locked use is still disabled.')
    except Exception:
        # Preserve third-party modifications made since our backup. If removing
        # our rule fails, retain binaries so authdb never points at missing code.
        if rule_written or RIGHT in (read_right(SYSTEM_RIGHT) or {}).get('rule', []):
            write_right(SYSTEM_RIGHT, with_alternative(read_right(SYSTEM_RIGHT), False))
        if right_written:
            command('/usr/bin/security', 'authorizationdb', 'remove', RIGHT)
        if service_loaded:
            command('/bin/launchctl', 'bootout', 'system/' + LABEL, check=False)
        for destination in reversed(installed):
            remove_path(destination)
        remove_path(BACKUP)
        raise


def uninstall():
    if not (BACKUP / 'installed.json').is_file():
        raise ValueError('No completed validation installation backup found')
    secure_parent(BACKUP)
    secure_parent(ROOT)
    # Do not interrupt an active protected session during this development
    # uninstall. Neither process is started by the product application.
    if command('/usr/bin/pgrep', '-f', r'/SNTHLockedUse(Guardian|Validation)( |$)', check=False).returncode == 0:
        raise ValueError('Finish the validation session before removing components')
    current = read_right(SYSTEM_RIGHT)
    if RIGHT in current.get('rule', []):
        write_right(SYSTEM_RIGHT, with_alternative(current, False))
    current_own = read_right(RIGHT)
    if current_own is not None:
        if not equivalent(current_own, own_right()):
            raise ValueError('SNTH authorization right changed; retained for manual review')
        command('/usr/bin/security', 'authorizationdb', 'remove', RIGHT)
    command('/bin/launchctl', 'bootout', 'system/' + LABEL, check=False)
    for _, destination, _ in reversed(CODE):
        remove_path(destination)
    remove_path(PLIST)
    remove_path(BACKUP)
    if ROOT.exists() and not any(ROOT.iterdir()):
        ROOT.rmdir()
    print('Validation components removed; current third-party unlock alternatives preserved.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument('--plan', action='store_true')
    modes.add_argument('--install', action='store_true')
    modes.add_argument('--uninstall', action='store_true')
    parser.add_argument('--package', type=Path, default=Path(__file__).resolve().parents[1] / 'target/locked-use')
    args = parser.parse_args()
    if not args.install and not args.uninstall:
        before = read_right(SYSTEM_RIGHT)
        after = with_alternative(before, True)
        for name, _, identifier in CODE:
            verify(args.package / name, identifier)
        print(json.dumps({'mode': 'validation_only', 'production_unlock_ready': False,
                          'before': before['rule'], 'after': after['rule'],
                          'install': [str(p) for _, p, _ in CODE] + [str(PLIST)],
                          'backup': str(BACKUP), 'signatures_verified': True}, indent=2))
        return
    if os.geteuid() != 0:
        parser.error('Installation/removal requires macOS administrator authentication')
    if args.install:
        install(args.package.resolve())
    else:
        uninstall()


if __name__ == '__main__':
    try:
        main()
    except (ValueError, RuntimeError, OSError, subprocess.SubprocessError) as error:
        print('Validation setup failed: ' + str(error), file=sys.stderr)
        sys.exit(1)
