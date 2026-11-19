#!/usr/bin/python3
"""Real bridge/core persistence regression. Requires prepared public resources."""
import argparse
import json
from pathlib import Path
import subprocess
import threading
import queue


def scenario(binary, resources, root, name, raw, target, security_transition):
    user = root / name
    user.mkdir(mode=0o700)
    child = subprocess.Popen([str(binary), str(resources), str(user), str(user/'absent-onnx.so')],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding='utf-8')
    messages = queue.Queue()
    def read():
        for line in child.stdout:
            messages.put(json.loads(line))
    threading.Thread(target=read,daemon=True).start()
    number = 0
    def call(op, **fields):
        nonlocal number
        number += 1
        child.stdin.write(json.dumps(dict(op=op,id=number,epoch=number,**fields),ensure_ascii=False)+'\n')
        child.stdin.flush()
        while True:
            reply = messages.get(timeout=5)
            if reply.get('id') == number:
                assert 'error' not in reply, reply
                return reply
    try:
        call('focus',app='gtk3-im:learning-regression')
        call('security',secure=False)
        for letter in raw:
            call('key',key='Text',text=letter)
        selected=call('key',key='Text',text='1')
        assert selected['view']['commit']==target,selected
        if security_transition:
            call('security',secure=True)
            # A private key must pass through without adding a learning fact.
            private=call('key',key='Text',text='x')
            assert not private['view']['consumed']
            before=json.loads((user/'Rime/agent_zhixu_learning_v1.json').read_text())
            assert any(item['text']==target for item in before['entries'])
        child.stdin.close()
        assert child.wait(timeout=5)==0
        document=json.loads((user/'Rime/agent_zhixu_learning_v1.json').read_text())
        assert any(item['text']==target for item in document['entries'])
        assert all(item['text']!='x' for item in document['entries'])
        return {'id':name,'committed':target,'entries':len(document['entries']),'choices':len(document['choices']),'passed':True}
    finally:
        if child.poll() is None:
            child.terminate();child.wait(timeout=3)


if __name__=='__main__':
    ap=argparse.ArgumentParser();ap.add_argument('binary',type=Path);ap.add_argument('resources',type=Path);ap.add_argument('fresh_directory',type=Path);args=ap.parse_args()
    args.fresh_directory.mkdir(mode=0o700)
    cases=[scenario(args.binary,args.resources,args.fresh_directory,'ordinary-to-secure','nihao','你好',True),
           scenario(args.binary,args.resources,args.fresh_directory,'ordinary-to-eof','ceshi','测试',False)]
    print(json.dumps({'passed':True,'cases':cases},ensure_ascii=False,indent=2))
