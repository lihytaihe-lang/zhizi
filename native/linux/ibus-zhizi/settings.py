#!/usr/bin/python3
"""Chinese settings and management of the current native learning store."""
import json
import os
from pathlib import Path
import subprocess
import tempfile

import gi
gi.require_version("Gtk", "3.0")
from gi.repository import Gtk
from messages import describe_error, format_capacity

DATA = Path(os.environ.get("XDG_DATA_HOME", str(Path.home() / ".local/share"))) / "zhizi"
LIB = Path(os.environ.get("ZHIZI_LIB", "/usr/lib/zhizi"))


def atomic_private(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix=".zhizi-", dir=path.parent)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
            stream.write(text)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


class Settings(Gtk.Window):
    def __init__(self):
        super().__init__(title="知字设置与个人词")
        self.set_default_size(760, 560)
        self.connect("destroy", Gtk.main_quit)
        outer = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12, margin=16)
        self.add(outer)
        title = Gtk.Label(label="知字 · 本地输入", xalign=0)
        outer.pack_start(title, False, False, 0)
        description = Gtk.Label(label="Shift 切换中英文；数字或鼠标选词；减号 / 等号翻页。设置在下次切换窗口时生效。", xalign=0)
        description.set_line_wrap(True)
        outer.pack_start(description, False, False, 0)
        notebook = Gtk.Notebook()
        outer.pack_start(notebook, True, True, 0)
        switches = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12, margin=16)
        notebook.append_page(switches, Gtk.Label(label="输入设置"))
        try:
            self.settings = json.loads((DATA / "settings.json").read_text(encoding="utf-8"))
            if not isinstance(self.settings, dict):
                self.settings = {}
        except (OSError, ValueError):
            self.settings = {}
        for key, label in [("contextRanking", "开启顺手·知字候选排序"), ("zhiju", "开启知句整句生成"), ("typoCorrection", "开启键位纠错")]:
            toggle = Gtk.CheckButton(label=label)
            toggle.set_active(self.settings.get(key, True) is not False)
            toggle.connect("toggled", self.toggle, key)
            switches.pack_start(toggle, False, False, 0)
        note = Gtk.Label(label="模型在本机运行；缺少模型时继续使用基础候选。密码、PIN及隐私字段原样输入，不参与学习。", xalign=0)
        note.set_line_wrap(True)
        switches.pack_start(note, False, False, 0)
        self.store = Gtk.ListStore(str, str, int)
        self.blacklist = Gtk.ListStore(str, str, int)
        self.views = {}
        for kind, name, model in [("learning", "当前学习", self.store), ("blacklist", "永久屏蔽", self.blacklist)]:
            box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8, margin=12)
            notebook.append_page(box, Gtk.Label(label=name))
            bar = Gtk.Box(spacing=8)
            box.pack_start(bar, False, False, 0)
            buttons = [("刷新", lambda _b, k=kind: self.refresh(k))]
            if kind == "learning":
                buttons += [("删除所选", self.delete), ("撤销上次删除", self.undo), ("导出备份", self.export), ("导入备份", self.import_backup)]
            else:
                buttons += [("解除所选屏蔽", self.unblock)]
            for label, callback in buttons:
                button = Gtk.Button(label=label)
                button.connect("clicked", callback)
                bar.pack_start(button, False, False, 0)
            view = Gtk.TreeView(model=model)
            view.get_selection().set_mode(Gtk.SelectionMode.MULTIPLE)
            for index, label in [(0, "词语"), (1, "拼音 / 原输入")]:
                column = Gtk.TreeViewColumn(label, Gtk.CellRendererText(), text=index)
                column.set_expand(True)
                view.append_column(column)
            self.views[kind] = view
            scroll = Gtk.ScrolledWindow()
            scroll.set_policy(Gtk.PolicyType.AUTOMATIC, Gtk.PolicyType.AUTOMATIC)
            scroll.add(view)
            box.pack_start(scroll, True, True, 0)
        self.status = Gtk.Label(xalign=0)
        self.status.set_line_wrap(True)
        outer.pack_start(self.status, False, False, 0)
        self.refresh("learning")
        self.refresh("blacklist")

    def message(self, text, confirm=False):
        dialog = Gtk.MessageDialog(transient_for=self, modal=True, message_type=Gtk.MessageType.QUESTION if confirm else Gtk.MessageType.INFO,
            buttons=Gtk.ButtonsType.OK_CANCEL if confirm else Gtk.ButtonsType.OK, text=text)
        response = dialog.run()
        dialog.destroy()
        return response == Gtk.ResponseType.OK

    def call(self, op, **fields):
        result = subprocess.run([str(LIB / "zhizi-learning"), str(DATA)], input=json.dumps(dict(fields, op=op), ensure_ascii=False),
            text=True, encoding="utf-8", stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=15)
        reply = json.loads(result.stdout)
        if result.returncode or reply.get("error"):
            raise RuntimeError(reply.get("error", "本地学习操作未完成"))
        return reply

    def guarded(self, action):
        try:
            action()
        except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as error:
            self.message(describe_error(str(error)))

    def toggle(self, button, key):
        def save():
            self.settings[key] = button.get_active()
            atomic_private(DATA / "settings.json", json.dumps(self.settings, ensure_ascii=False) + "\n")
            self.status.set_text("已保存；下次切换输入窗口时生效。")
        self.guarded(save)

    def refresh(self, kind="learning"):
        def update():
            if kind == "learning":
                reply = self.call("list")
                self.items = reply["snapshot"]["items"]
                self.store.clear()
                for index, item in enumerate(self.items):
                    self.store.append([item["key"]["text"], item["key"]["reading"], index])
                status = reply["snapshot"]["status"]
                self.status.set_text(format_capacity(status))
            else:
                self.blocked = self.call("blacklist_list")["items"]
                self.blacklist.clear()
                for index, item in enumerate(self.blocked):
                    self.blacklist.append([item["text"], item["reading"], index])
        self.guarded(update)

    def selected(self, kind):
        model, rows = self.views[kind].get_selection().get_selected_rows()
        items = self.items if kind == "learning" else self.blocked
        return [items[model[row][2]]["key"] if kind == "learning" else items[model[row][2]] for row in rows]

    def delete(self, _button):
        keys = self.selected("learning")
        if keys and self.message(f"删除所选 {len(keys)} 个学习词？可在30分钟内撤销上次删除。", True):
            self.guarded(lambda: self.call("delete", keys=keys))
            self.refresh()

    def undo(self, _button):
        self.guarded(lambda: self.call("undo"))
        self.refresh()

    def unblock(self, _button):
        keys = self.selected("blacklist")
        if keys and self.message(f"解除所选 {len(keys)} 个词的永久屏蔽？", True):
            self.guarded(lambda: self.call("blacklist_remove", keys=keys))
            self.refresh("blacklist")

    def choose_file(self, save):
        dialog = Gtk.FileChooserDialog(title="导出学习备份" if save else "导入学习备份", transient_for=self,
            action=Gtk.FileChooserAction.SAVE if save else Gtk.FileChooserAction.OPEN)
        dialog.add_buttons("取消", Gtk.ResponseType.CANCEL, "保存" if save else "打开", Gtk.ResponseType.OK)
        if save:
            dialog.set_current_name("知字学习备份.json")
            dialog.set_do_overwrite_confirmation(True)
        result = Path(dialog.get_filename()) if dialog.run() == Gtk.ResponseType.OK else None
        dialog.destroy()
        return result

    def export(self, _button):
        path = self.choose_file(True)
        if path:
            def save():
                atomic_private(path, self.call("export")["text"])
                self.status.set_text("已导出。备份包含学习词和选择记录，请自行妥善保存。")
            self.guarded(save)

    def import_backup(self, _button):
        path = self.choose_file(False)
        if path:
            def load():
                if path.stat().st_size > 8 * 1024 * 1024:
                    raise ValueError("backup too large")
                text = path.read_text(encoding="utf-8")
                preview = self.call("preview", text=text)["preview"]
                if self.message(f"合并 {preview['entries']} 个词和 {preview['choices']} 条选择记录？重复事实取较大计数，现有数据保留。", True):
                    self.call("import", text=text)
                    self.refresh()
            self.guarded(load)


if __name__ == "__main__":
    window = Settings()
    window.show_all()
    Gtk.main()
