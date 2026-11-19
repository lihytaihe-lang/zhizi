import importlib.util
from pathlib import Path
import unittest
p=Path(__file__).resolve().parents[1]/'ibus-zhizi/messages.py'
s=importlib.util.spec_from_file_location('messages',p);m=importlib.util.module_from_spec(s);s.loader.exec_module(m)
class Messages(unittest.TestCase):
    def test_error_never_echoes_private_detail(self):
        for detail in ['invalid backup /home/example/private-words.json 秘密词','unknown error: 秘密词','permission denied /home/example/owner']:
            response=m.describe_error(detail)
            self.assertNotIn('/home/',response);self.assertNotIn('秘密词',response)
    def test_capacity_rejects_bool_and_negative_counts(self):
        text=m.format_capacity({'entries':True,'choices':-3,'estimatedBytes':1024,'maxBytes':1048576,'maxEntries':4096,'maxAgeDays':45})
        self.assertIn('学习词 0 / 4096',text);self.assertIn('1,024',text);self.assertIn('1,048,576',text);self.assertNotIn('-3',text)
if __name__=='__main__':unittest.main()
