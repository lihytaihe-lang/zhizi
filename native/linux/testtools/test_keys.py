import importlib.util
from pathlib import Path
import unittest
p=Path(__file__).resolve().parents[1]/'ibus-zhizi/keys.py'
s=importlib.util.spec_from_file_location('keys',p);k=importlib.util.module_from_spec(s);s.loader.exec_module(k)
class Keys(unittest.TestCase):
    def test_shift_tap_requires_matching_uninterrupted_release(self):
        m=k.KeyMapper();self.assertIsNone(m.map(0xffe1,0));self.assertEqual(m.map(0xffe1,k.RELEASE),{'key':'ShiftTap','modifiers':0})
        m.map(0xffe1,0);m.map(ord('A'),k.SHIFT);self.assertIsNone(m.map(0xffe1,k.RELEASE))
        m.map(0xffe1,0);m.map(0xffe2,k.SHIFT);self.assertIsNone(m.map(0xffe1,k.RELEASE));self.assertIsNone(m.map(0xffe2,k.RELEASE))
    def test_focus_reset_prevents_old_shift_release_toggling_new_field(self):
        m=k.KeyMapper();m.map(0xffe1,0);m.reset();self.assertIsNone(m.map(0xffe1,k.RELEASE))
    def test_shortcuts_pass_but_explicit_delete_commands_are_distinct(self):
        m=k.KeyMapper()
        for mask in [k.CONTROL,k.ALT,k.MOD4,k.SUPER,k.META]:self.assertIsNone(m.map(ord('v'),mask))
        self.assertEqual(m.map(0xff08,k.CONTROL)['key'],'DeleteCandidate')
        self.assertEqual(m.map(0xff08,k.CONTROL|k.SHIFT)['key'],'BlacklistCandidate')
        self.assertIsNone(m.map(0xff08,k.CONTROL|k.ALT))
    def test_modifiers_and_page_keys_preserve_shifted_symbols(self):
        m=k.KeyMapper();self.assertEqual(m.map(ord('='),0)['key'],'PageDown');self.assertEqual(m.map(ord('-'),0)['key'],'PageUp')
        self.assertEqual(m.map(ord('+'),k.SHIFT),{'key':'Text','text':'+','modifiers':1})
        self.assertEqual(m.map(ord('A'),k.SHIFT|k.LOCK)['modifiers'],17)
        self.assertIsNone(m.map(ord('a'),k.RELEASE))
    def test_unicode_and_unknown_keysyms(self):
        m=k.KeyMapper();self.assertEqual(m.map(0x01000000|ord('你'),0)['text'],'你')
        for n in [0x0100d800,0x01110000,0xff09,0xffff,0xff50,0x0100000a]:self.assertIsNone(m.map(n,0))
    def test_secure_purpose_and_hints(self):
        self.assertTrue(k.secure_content(8,0));self.assertTrue(k.secure_content(9,0));self.assertTrue(k.secure_content(0,1<<11));self.assertTrue(k.secure_content(0,1<<12));self.assertFalse(k.secure_content(0,0))
    def test_cursor_counts_characters_and_clamps_inside_multibyte(self):
        text='你a😊';self.assertEqual([k.utf8_cursor_to_char(text,i) for i in [-1,0,1,2,3,4,5,8,99]],[0,0,0,0,1,2,2,3,3])
class RepeatBoundary(unittest.TestCase):
    def test_only_repeats_after_emptying_composition_are_swallowed(self):
        g=k.BackspaceRepeatGuard()
        self.assertFalse(g.swallow(0xff08,0));g.observe(True,True)
        self.assertFalse(g.swallow(0xff08,0));g.observe(True,False)
        self.assertTrue(g.swallow(0xff08,0))
        self.assertFalse(g.swallow(0xff08,k.RELEASE))
        self.assertFalse(g.swallow(0xff08,0));g.observe(False,False)
        self.assertFalse(g.swallow(0xff08,0))
        g.reset();self.assertFalse(g.swallow(0xff08,0))
if __name__=='__main__':unittest.main()
