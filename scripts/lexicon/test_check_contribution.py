"""Run with python3 -m unittest scripts.lexicon.test_check_contribution -v."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).with_name('check_contribution.py')


class ContributionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.rime = self.root / 'Rime'
        self.rime.mkdir()
        (self.rime / 'test.dict.yaml').write_text(
            '---\nname: test\n...\n大\tda\t10\n关\tguan\t10\n'
            '重庆\tchong qing\t10\n热爱\\tre ai\\t10\n', encoding='utf-8')

    def run_check(self, *contents):
        paths = []
        for i, content in enumerate(contents):
            path = self.root / f'contrib{i}.tsv'
            path.write_text(content, encoding='utf-8')
            paths.append(str(path))
        report = self.root / 'report.json'
        result = subprocess.run(
            [sys.executable, str(SCRIPT), *paths, '--rime', str(self.rime),
             '--report', str(report)], capture_output=True, text=True)
        self.assertTrue(report.exists(), result.stderr)
        return result, json.loads(report.read_text(encoding='utf-8'))

    def assert_rule(self, content, level, rule):
        result, issues = self.run_check(content)
        self.assertIn((level, rule), [(i['level'], i['rule']) for i in issues])
        self.assertEqual(result.returncode, 1 if level == 'error' else 0)
        return issues

    def test_valid_and_optional_weights(self):
        result, issues = self.run_check('# comment\n\n你好\tni hao\n女儿\tnv er\t1\n绿叶\tlv ye\t1000\n𠀀好\the hao\n')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(issues, [])

    def test_inventory(self):
        from scripts.lexicon.check_contribution import SYLLABLES
        self.assertEqual(len(SYLLABLES), 415)
        self.assertIn('len', SYLLABLES)

    def test_without_report(self):
        path = self.root / 'valid.tsv'
        path.write_text('你好\tni hao\n', encoding='utf-8')
        result = subprocess.run([sys.executable, str(SCRIPT), str(path),
                                 '--rime', str(self.rime)], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(json.loads(result.stdout), [])

    def test_word_format(self):
        for word in ['好', '你好世界你好世界好', 'hello', '你1', '你🙂', '你〇', '你\ufaff']:
            with self.subTest(word=word):
                self.assert_rule(f'{word}\tni hao\n', 'error', 'word_format')

    def test_pinyin_count(self):
        self.assert_rule('你好\tni\n', 'error', 'syllable_count')

    def test_invalid_syllable_and_spacing(self):
        for reading in ['ni xyz', 'nǐ hao', 'ni hǎo', 'ni  hao', 'ni\u3000hao', 'ni hao ', 'nü er', 'NI hao']:
            with self.subTest(reading=reading):
                self.assert_rule(f'你好\t{reading}\n', 'error', 'pinyin_format')

    def test_existing_pair(self):
        for entry in ['重庆\tchong qing', '热爱\tre ai']:
            self.assert_rule(entry + '\n', 'error', 'existing_entry')

    def test_duplicate_in_file(self):
        issues = self.assert_rule('# comment\n你好\tni hao\n你好\tni hao\t20\n', 'error', 'duplicate_entry')
        self.assertEqual(next(i['line'] for i in issues if i['rule'] == 'duplicate_entry'), 3)

    def test_duplicate_is_scoped_to_file(self):
        result, issues = self.run_check('你好\tni hao\n', '你好\tni hao\n')
        self.assertEqual(result.returncode, 0)
        self.assertEqual(issues, [])

    def test_weight(self):
        for weight in ['', '0', '1001', '-1', '1.0', '+1', '１２']:
            with self.subTest(weight=weight):
                self.assert_rule(f'你好\tni hao\t{weight}\n', 'error', 'weight_range')

    def test_columns(self):
        for row in ['你好', '你好\tni hao\t1\tnote']:
            self.assert_rule(row + '\n', 'error', 'column_count')

    def test_privacy_and_no_echo(self):
        for value in ['13800138000', 'person@example.com', 'https://example.com', 'www.example.com', 'example.com']:
            with self.subTest(value=value):
                result, issues = self.run_check(f'你好\tni hao\t1\t{value}\n')
                self.assertEqual(result.returncode, 1)
                self.assertIn('privacy_shape', [i['rule'] for i in issues])
                self.assertNotIn(value, json.dumps(issues))
                self.assertNotIn(value, result.stdout + result.stderr)
        result, issues = self.run_check('你好\tni hao\t13800138000\n')
        self.assertIn('privacy_shape', [i['rule'] for i in issues])

    def test_repeated_characters_warning(self):
        self.assert_rule('啊啊啊啊\ta a a a\n', 'warning', 'repeated_characters')

    def test_new_reading_warning(self):
        self.assert_rule('重庆\tzhong qing\n', 'warning', 'new_reading')

    def test_large_submission_across_files(self):
        # Unique Han words with structurally valid readings; comments do not count.
        rows = [f'你{chr(0x4e00 + i)}\tni hao\n' for i in range(501)]
        result, issues = self.run_check('# comment\n' + ''.join(rows[:250]), ''.join(rows[250:]))
        self.assertEqual(result.returncode, 0)
        self.assertIn('submission_size', [i['rule'] for i in issues])
        result, issues = self.run_check(''.join(rows[:500]))
        self.assertEqual(result.returncode, 0)
        self.assertNotIn('submission_size', [i['rule'] for i in issues])

    def test_io_errors_are_reported(self):
        (self.rime / 'bad.dict.yaml').write_bytes(b'\xff')
        self.assert_rule('你好\tni hao\n', 'error', 'rime_read')

    def test_missing_rime_fails(self):
        for path in self.rime.iterdir():
            path.unlink()
        self.rime.rmdir()
        self.assert_rule('你好\tni hao\n', 'error', 'rime_read')


if __name__ == '__main__':
    unittest.main()
