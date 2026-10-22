import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name('make_public_repo.py')
WORK = SCRIPT.parents[2] / 'tmp/phase01-20261018/B2work'


class MakePublicRepoTests(unittest.TestCase):
    def setUp(self):
        WORK.mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(dir=WORK)
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.src = self.root / 'src'
        self.src.mkdir()
        self.out = self.root / 'out'
        self.env = dict(os.environ, GIT_CONFIG_GLOBAL=str(self.root / 'global'), GIT_CONFIG_NOSYSTEM='1', ZHIZI_PUBLIC_AUTHOR_NAME='Example Maintainer', ZHIZI_PUBLIC_AUTHOR_EMAIL='maintainer@example.invalid')
        (self.root / 'global').write_text('[user]\n name = Sentinel\n')
        self.git(self.src, 'init', '-q')
        for name in ['README.md', 'LICENSE', 'LICENSE-LEXICON', 'LICENSE-SKINS', 'SECURITY.md', 'CODE_OF_CONDUCT.md', 'docs/public/README.md', 'docs/public/gitignore.public']:
            self.put(name, 'mapped readme' if name == 'docs/public/README.md' else 'safe\n')
        config = {'include': ['**'], 'exclude': ['scripts/'], 'rename': {'docs/public/README.md': 'README.md', 'docs/public/gitignore.public': '.gitignore'}, 'scan': {'blocker': [{'name': 'synthetic', 'pattern': 'SYNTHETIC_BLOCKER'}], 'warning': []}}
        self.put('scripts/tools/public_export.json', json.dumps(config))
        shutil.copyfile(SCRIPT.with_name('public_export.py'), self.src / 'scripts/tools/public_export.py')
        for i in range(3):
            self.put('VERSION', str(i))
            self.git(self.src, 'add', '-A')
            self.git(self.src, '-c', 'user.name=bot', '-c', 'user.email=bot@local', 'commit', '-qm', 'private bot history ' + str(i))

    def put(self, name, content):
        path = self.src / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)

    def git(self, root, *args):
        return subprocess.check_output(['git', '-C', str(root), *args], env=self.env, stderr=subprocess.PIPE).decode()

    def run_cli(self, *args, env=None, lex=False):
        options = ['lexicon', '--src-dir', str(self.src), '--scan-config', str(self.root / 'scan.json')] if lex else ['ime', '--src', str(self.src), '--report', str(self.root / 'report.json')]
        return subprocess.run([sys.executable, str(SCRIPT), *options, '--out', str(self.out), *args], env=env or self.env, capture_output=True, text=True)

    def test_github_noreply_identity_is_allowed_but_other_noreply_is_not(self):
        self.lexicon()
        env = self.env.copy()
        env.update(ZHIZI_PUBLIC_AUTHOR_NAME='Example Maintainer', ZHIZI_PUBLIC_AUTHOR_EMAIL='12345+example-login@users.noreply.github.com')
        ok = self.run_cli(lex=True, env=env)
        self.assertEqual(ok.returncode, 0, ok.stderr)
        self.assertEqual(self.git(self.out, 'log', '-1', '--format=%ae').strip(), '12345+example-login@users.noreply.github.com')
        shutil.rmtree(self.out)
        for email in ('noreply@anthropic.com', 'bot@noreply.example.org', '12345+x@users.noreply.github.com.evil.local'):
            env['ZHIZI_PUBLIC_AUTHOR_EMAIL'] = email
            self.assertNotEqual(self.run_cli(lex=True, env=env).returncode, 0, email)
            self.assertFalse((self.out / '.git').exists(), email)

    def test_missing_identity(self):
        for key in ['ZHIZI_PUBLIC_AUTHOR_NAME', 'ZHIZI_PUBLIC_AUTHOR_EMAIL']:
            env = self.env.copy()
            del env[key]
            self.assertNotEqual(self.run_cli(env=env).returncode, 0)
            self.assertFalse(self.out.exists())

    def test_nonempty(self):
        self.out.mkdir()
        (self.out / 'keep').touch()
        self.assertNotEqual(self.run_cli().returncode, 0)

    def test_dirty(self):
        self.put('VERSION', 'changed')
        self.assertNotEqual(self.run_cli().returncode, 0)
        self.assertEqual(self.run_cli('--allow-dirty').returncode, 0)

    def test_excluded_dirty(self):
        self.put('scripts/tools/public_export.py', SCRIPT.with_name('public_export.py').read_text() + '\n')
        self.assertEqual(self.run_cli().returncode, 0)

    def test_blocker(self):
        self.put('VERSION', 'SYNTHETIC_BLOCKER')
        self.assertEqual(self.run_cli('--allow-dirty').returncode, 1)
        self.assertFalse((self.out / '.git').exists())

    def test_success(self):
        before = (self.root / 'global').read_bytes()
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout.splitlines()[-1])['commit_count'], 1)
        history = self.git(self.out, 'log', '--format=%an <%ae>%n%cn <%ce>%n%B')
        for text in ['bot', 'local', 'Co-Authored-By', 'private']:
            self.assertNotIn(text, history)
        self.assertIn('Example Maintainer <maintainer@example.invalid>', history)
        self.assertEqual(self.git(self.out, 'remote'), '')
        self.assertEqual((self.out / 'README.md').read_text(), 'mapped readme')
        self.assertEqual((self.out / '.gitignore').read_text(), 'safe\n')
        self.assertEqual(before, (self.root / 'global').read_bytes())
        exported = json.loads((self.root / 'report.json').read_text())['files']
        self.assertEqual(sorted(exported), sorted(self.git(self.out, 'ls-files').splitlines()))

    def lexicon(self):
        shutil.rmtree(self.src / '.git')
        shutil.copyfile(self.src / 'scripts/tools/public_export.json', self.root / 'scan.json')
        shutil.rmtree(self.src / 'scripts')
        self.put('LICENSE', 'Creative Commons Attribution-ShareAlike 4.0')
        self.put('LICENSE-CODE', 'Apache License Version 2.0')
        for name in ['THIRD_PARTY_NOTICES.md', 'CONTRIBUTING.md', '.github/ISSUE_TEMPLATE/question.md', 'dist/package', '__pycache__/a.pyc', 'a.pyc', '.DS_Store']:
            self.put(name, 'safe')
        (self.src / '.build-deps').symlink_to('missing')

    def test_lexicon(self):
        self.lexicon()
        result = self.run_cli(lex=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        for name in ['dist', '.build-deps', '__pycache__', 'a.pyc', '.DS_Store']:
            self.assertFalse((self.out / name).exists())
        self.assertEqual(self.git(self.out, 'remote'), '')
        self.assertEqual((self.out / 'LICENSE').read_bytes(), (self.src / 'LICENSE').read_bytes())

    def test_lexicon_requires_apache_code_license(self):
        self.lexicon()
        self.put('LICENSE-CODE', 'Creative Commons Attribution-ShareAlike 4.0')
        self.assertNotEqual(self.run_cli(lex=True).returncode, 0)
        self.assertFalse((self.out / '.git').exists())

    def test_lexicon_symlink(self):
        self.lexicon()
        (self.src / 'link').symlink_to('README.md')
        self.assertNotEqual(self.run_cli(lex=True).returncode, 0)
        self.assertFalse((self.out / '.git').exists())

    def test_real_scan_rules(self):
        self.lexicon()
        shutil.copyfile(SCRIPT.with_name('public_export.json'), self.root / 'scan.json')
        self.put('README.md', 'sk-' + 'abcdefghijklmnop')
        result = self.run_cli(lex=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        shutil.rmtree(self.out)
        self.put('README.md', 'sk-' + 'A' * 16)
        result = self.run_cli(lex=True)
        self.assertEqual(result.returncode, 1)
        self.assertNotIn('sk-' + 'A' * 16, result.stdout + result.stderr)
        self.assertFalse((self.out / '.git').exists())

    def test_lexicon_blocker(self):
        self.lexicon()
        self.put('README.md', 'SYNTHETIC_BLOCKER')
        self.assertEqual(self.run_cli(lex=True).returncode, 1)
        self.assertFalse((self.out / '.git').exists())


if __name__ == '__main__':
    unittest.main()
