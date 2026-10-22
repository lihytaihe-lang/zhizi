#!/usr/bin/env python3
"""Create an offline public repository with one fresh initial commit."""

import argparse
import codecs
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys


def git(root, *args, env=None):
    result = subprocess.run(['git', '-C', str(root), *args], capture_output=True,
                            env=env)
    if result.returncode:
        raise ValueError('git operation failed: ' + args[0])
    return result.stdout.decode('utf-8')


def exporter(src, out, report=None, dry=False):
    command = [sys.executable, str(src / 'scripts/tools/public_export.py'),
               '--src', str(src), '--out', str(out), '--config',
               str(src / 'scripts/tools/public_export.json')]
    if report:
        command += ['--report', str(report)]
    if dry:
        command += ['--dry-run']
    result = subprocess.run(command, capture_output=True, text=True)
    if result.returncode not in (0, 1):
        raise ValueError('public exporter failed (exit %d)' % result.returncode)
    data = json.loads(result.stdout)
    for severity in ('blocker', 'warning'):
        for hit in data[severity]:
            print(json.dumps(dict(severity=severity, **hit)))
    return result.returncode, data


def regenerate_derived(src, out):
    """Rebuild every file derived from the stubbed pack dictionaries, inside the exported tree.

    The exporter ships the professional packs as header-only shells; the reading indexes, the heavy-garble
    list, the layer catalog and the membership table are derived from them and would otherwise still carry
    the pack words. The pinned base table lives in .build-deps, which is not exported, so it is linked in
    for the run and removed again before anything is committed."""
    deps = src / '.build-deps'
    link = out / '.build-deps'
    if not deps.is_dir():
        raise ValueError('.build-deps missing in source: cannot regenerate derived files')
    link.symlink_to(deps)
    try:
        for command in (['node', 'scripts/generate_exact_reading_index.mjs'],
                        ['node', 'scripts/generate_heavy_garble_words.mjs'],
                        [sys.executable, 'scripts/lexicon/build_catalog.py', '--out', str(out / '.derived-tmp')]):
            result = subprocess.run(command, cwd=out, capture_output=True, text=True)
            if result.returncode:
                raise ValueError('derived regeneration failed: ' + ' '.join(command[1:2]))
        derived = out / '.derived-tmp'
        shutil.move(str(derived / 'lexicon_catalog.json'), out / 'native/resources/Lexicon/lexicon_catalog.json')
        shutil.move(str(derived / 'lexicon_membership.tsv'),
                    out / 'native/core/agent-ime-core/resources/lexicon_membership.tsv')
        derived.rmdir()
    finally:
        link.unlink()


def scan(out, config):
    rules = json.loads(config.read_text(encoding='utf-8'))['scan']
    findings = {'blocker': [], 'warning': []}
    for path in sorted(out.rglob('*')):
        if not path.is_file():
            continue
        data = path.read_bytes()
        if data.startswith((codecs.BOM_UTF32_LE, codecs.BOM_UTF32_BE)):
            lines = data.decode('utf-32', errors='replace').splitlines()
        elif data.startswith((codecs.BOM_UTF16_LE, codecs.BOM_UTF16_BE)):
            lines = data.decode('utf-16', errors='replace').splitlines()
        else:
            lines = [] if b'\0' in data else data.decode('utf-8-sig', errors='replace').splitlines()
        for severity, entries in rules.items():
            for rule in entries:
                candidates = [(0, path.name)] if rule.get('target', 'text') == 'filename' else enumerate(lines, 1)
                for number, line in candidates:
                    if any(m.group() not in rule.get('allow', []) for m in re.finditer(rule['pattern'], line)):
                        hit = dict(file=path.relative_to(out).as_posix(), line=number, rule=rule['name'])
                        findings[severity].append(hit)
                        print(json.dumps(dict(severity=severity, **hit)))
    return findings


def make(args):
    name = os.environ.get('ZHIZI_PUBLIC_AUTHOR_NAME', '').strip()
    email = os.environ.get('ZHIZI_PUBLIC_AUTHOR_EMAIL', '').strip()
    if not name or not email:
        raise ValueError('ZHIZI_PUBLIC_AUTHOR_NAME and ZHIZI_PUBLIC_AUTHOR_EMAIL are required')
    if any(c in name + email for c in '\n\r<>'):
        raise ValueError('invalid public identity')
    message = args.message or ('Initial public release' if args.mode == 'ime' else 'Initial lexicon release')
    # Reject unsafe identity/message before creating a repository; scan again after commit.
    forbidden = r'local|zhenvis|co-authored-by|noreply@|(?:users\.)?noreply\.[\w.-]+|\bbot@'
    # GitHub's own per-account address (<id>+<login>@users.noreply.github.com) keeps the real mailbox private and is allowed.
    github_noreply = re.compile(r'\b\d+\+[A-Za-z0-9-]+@users\.noreply\.github\.com\b', re.I)
    def unsafe(text):
        return re.search(forbidden, github_noreply.sub('', text), re.I)
    if unsafe(name + '\n' + email + '\n' + message):
        raise ValueError('forbidden public history metadata')
    out = args.out.absolute()
    if out.is_symlink() or (out.exists() and (not out.is_dir() or any(out.iterdir()))):
        raise ValueError('output directory must be absent or empty')
    if args.mode == 'ime':
        if not args.src or not args.report:
            raise ValueError('ime requires --src and --report')
        src = args.src.resolve(strict=True)
        if git(src, 'rev-parse', '--show-toplevel').strip() != str(src):
            raise ValueError('source must be repository root')
        report = args.report.resolve()
        if report == src or src in report.parents:
            raise ValueError('report must be outside source repository')
        if report == out or out in report.parents:
            raise ValueError('report must be outside output repository')
        code, data = exporter(src, out, dry=True)
        if code:
            return code
        rename = data.get('rename', {})
        selected = (set(data['files']) - set(rename.values())) | set(rename)
        dirty = set()
        for options in [('diff', '--name-only', '-z', 'HEAD'), ('diff', '--cached', '--name-only', '-z')]:
            dirty.update(filter(None, git(src, *options).split('\0')))
        dirty &= selected
        if dirty and not args.allow_dirty:
            raise ValueError('exported tracked files have uncommitted changes: ' + ', '.join(sorted(dirty)))
        code, data = exporter(src, out, report)
        if code:
            return code
        if data.get('stubbed'):
            regenerate_derived(src, out)
        expected = sorted(data['files'])
        required = ['README.md', 'LICENSE', 'LICENSE-LEXICON', 'LICENSE-SKINS', 'SECURITY.md', 'CODE_OF_CONDUCT.md', '.gitignore']
        excluded = ['releases', 'tmp', 'docs/current/decisions', 'quality/mined', '.github/workflows']
    else:
        if not args.src_dir:
            raise ValueError('lexicon requires --src-dir')
        src = args.src_dir.resolve(strict=True)
        if not src.is_dir() or (src / '.git').exists():
            raise ValueError('lexicon source must be a directory without .git')
        if out.resolve() == src or src in out.resolve().parents:
            raise ValueError('output must be outside lexicon source')
        expected = []
        def copy(directory, relative=Path()):
            for path in sorted(directory.iterdir()):
                rel = relative / path.name
                if path.name in ('dist', '__pycache__', '.DS_Store') or path.suffix == '.pyc':
                    continue
                if path.name == '.build-deps' and path.is_symlink():
                    continue
                if path.is_symlink():
                    raise ValueError('lexicon symlink refused: ' + rel.as_posix())
                if path.name == '.git':
                    raise ValueError('nested git metadata refused')
                if path.is_dir():
                    copy(path, rel)
                elif path.is_file():
                    target = out / rel
                    target.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(path, target)
                    expected.append(rel.as_posix())
                else:
                    raise ValueError('nonregular lexicon entry')
        out.mkdir(parents=True, exist_ok=True)
        copy(src)
        findings = scan(out, args.scan_config)
        if findings['blocker']:
            return 1
        required = ['LICENSE', 'LICENSE-CODE', 'README.md', 'THIRD_PARTY_NOTICES.md', 'SECURITY.md', 'CONTRIBUTING.md', '.github/ISSUE_TEMPLATE']
        excluded = ['dist', '.build-deps']
        license_text = (out / 'LICENSE').read_text(encoding='utf-8')
        if not ('4.0' in license_text and ('Attribution-ShareAlike' in license_text or 'CC BY-SA' in license_text)):
            raise ValueError('lexicon LICENSE must identify CC BY-SA 4.0')
        if 'Apache License' not in (out / 'LICENSE-CODE').read_text(encoding='utf-8'):
            raise ValueError('lexicon LICENSE-CODE must be the Apache License 2.0')
    for item in required:
        if not ((out / item).is_dir() if item == '.github/ISSUE_TEMPLATE' else (out / item).is_file()):
            raise ValueError('required public path missing: ' + item)
    for item in excluded:
        if (out / item).exists():
            raise ValueError('excluded public path exists: ' + item)
    version = out / 'VERSION'
    if args.mode == 'ime' and version.is_file():
        message += '\n\nVersion ' + version.read_text(encoding='utf-8').strip()
    if unsafe(message):
        raise ValueError('forbidden public commit message')
    # Remove inherited Git routing, identity, config and date overrides.
    env = {key: value for key, value in os.environ.items() if not key.startswith('GIT_')}
    env.update(GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM='1',
               GIT_AUTHOR_NAME=name, GIT_AUTHOR_EMAIL=email,
               GIT_COMMITTER_NAME=name, GIT_COMMITTER_EMAIL=email)
    git(out, 'init', '-b', args.branch, env=env)
    git(out, 'config', '--local', 'user.name', name, env=env)
    git(out, 'config', '--local', 'user.email', email, env=env)
    git(out, 'add', '-f', '-A', env=env)
    git(out, '-c', 'core.hooksPath=/dev/null', '-c', 'commit.gpgsign=false', 'commit', '-m', message, env=env)
    count = int(git(out, 'rev-list', '--count', '--all', env=env).strip())
    files = sorted(filter(None, git(out, 'ls-files', '-z', env=env).split('\0')))
    history = git(out, 'log', '--all', '--format=%H%n%an%n%ae%n%cn%n%ce%n%s%n%b', env=env)
    identity = git(out, 'log', '--format=%an <%ae>%n%cn <%ce>', env=env).splitlines()
    clean = not git(out, 'status', '--porcelain', '--untracked-files=all', env=env)
    if count != 1 or files != sorted(expected) or identity != [name + ' <' + email + '>'] * 2 or not clean or git(out, 'remote', env=env) or unsafe(history):
        raise ValueError('public repository self-check failed; output retained')
    print(json.dumps(dict(mode=args.mode, commit_count=count, file_count=len(files), author=identity[0], clean=clean, history_scan='passed', remote_count=0)))
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=['ime', 'lexicon'])
    for option in ('src', 'src-dir', 'out', 'report'):
        parser.add_argument('--' + option, type=Path, required=option == 'out')
    parser.add_argument('--scan-config', type=Path, default=Path(__file__).with_name('public_export.json'))
    parser.add_argument('--message')
    parser.add_argument('--branch', default='main')
    parser.add_argument('--allow-dirty', action='store_true', help='testing only')
    try:
        return make(parser.parse_args())
    except (OSError, ValueError, KeyError, TypeError, re.error, subprocess.SubprocessError) as error:
        print('make public repository failed: ' + str(error), file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())
