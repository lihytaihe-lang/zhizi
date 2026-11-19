#!/usr/bin/python3
"""Synthetic native GTK controls. The controller injects real XTest key/mouse
requests; this probe only resets fixtures and reads its own control contents."""
import argparse
import json
import os
from pathlib import Path
import socket
import threading
import gi
gi.require_version('Gtk','3.0')
gi.require_version('GdkX11','3.0')
from gi.repository import Gtk, GdkX11, GLib

class Probe(Gtk.Window):
    def __init__(self, directory):
        super().__init__(title='知字 Ubuntu 实际窗口验收')
        self.directory=directory
        self.set_default_size(740,500)
        self.move(100,100)
        box=Gtk.Box(orientation=Gtk.Orientation.VERTICAL,spacing=12,margin=20)
        self.add(box)
        box.pack_start(Gtk.Label(label='普通输入（实际GTK TextView）',xalign=0),False,False,0)
        self.normal=Gtk.TextView()
        self.normal.set_wrap_mode(Gtk.WrapMode.WORD_CHAR)
        self.normal.set_left_margin(8)
        self.normal.set_top_margin(8)
        box.pack_start(self.normal,True,True,0)
        box.pack_start(Gtk.Label(label='第二个普通输入框',xalign=0),False,False,0)
        self.secondary=Gtk.Entry()
        box.pack_start(self.secondary,False,False,0)
        box.pack_start(Gtk.Label(label='密码框（PASSWORD，不显示明文）',xalign=0),False,False,0)
        self.secure=Gtk.Entry()
        self.secure.set_visibility(False)
        self.secure.set_input_purpose(Gtk.InputPurpose.PASSWORD)
        box.pack_start(self.secure,False,False,0)
        self.connect('destroy',Gtk.main_quit)
        self.show_all()
        self.present()
        self.normal.grab_focus()
        self.path=directory/'probe.sock'
        if self.path.exists():raise RuntimeError('probe socket already exists')
        self.server=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM)
        self.server.bind(str(self.path));os.chmod(self.path,0o600);self.server.listen(8)
        threading.Thread(target=self.listen,daemon=True).start()

    def state(self):
        buffer=self.normal.get_buffer()
        return {'window':self.get_window().get_xid(),'normal':buffer.get_text(buffer.get_start_iter(),buffer.get_end_iter(),False),
                'secondary':self.secondary.get_text(),'secure':self.secure.get_text(),
                'focus':next((name for name in ['normal','secondary','secure'] if getattr(self,name).has_focus()),None)}

    def dispatch(self, request, finished, response):
        try:
            op=request.get('op','read')
            if op=='reset':
                self.normal.get_buffer().set_text(request.get('prefix',''))
                self.secondary.set_text('');self.secure.set_text('')
                self.present();self.normal.grab_focus()
            elif op=='focus':
                target=request['target']
                if target not in ('normal','secondary','secure'):raise ValueError('invalid fixture focus')
                self.present();getattr(self,target).grab_focus()
            elif op=='close':GLib.timeout_add(100,lambda:(self.destroy(),GLib.SOURCE_REMOVE)[1])
            elif op!='read':raise ValueError('invalid fixture operation')
            response.update(self.state())
        except Exception as error:response['error']=str(error)
        finally:finished.set()
        return GLib.SOURCE_REMOVE

    def listen(self):
        while True:
            connection,_=self.server.accept()
            with connection:
                connection.settimeout(4)
                line=connection.makefile('rb').readline(8193)
                if len(line)>8192:continue
                finished=threading.Event();response={}
                GLib.idle_add(self.dispatch,json.loads(line),finished,response)
                if not finished.wait(4):response={'error':'GTK main loop timeout'}
                connection.sendall((json.dumps(response,ensure_ascii=False)+'\n').encode())

if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('directory',type=Path);args=parser.parse_args()
    args.directory.mkdir(parents=True,exist_ok=True)
    probe=Probe(args.directory)
    try:Gtk.main()
    finally:probe.path.unlink(missing_ok=True)
