#!/usr/bin/python3
"""IBus adapter: key events in; only the core's page and commit out."""
import os
from pathlib import Path
import sys

import gi
gi.require_version("IBus", "1.0")
from gi.repository import GLib, IBus
from bridge import Bridge
from keys import BackspaceRepeatGuard, KeyMapper, secure_content, utf8_cursor_to_char

RESOURCE_ROOT = Path(os.environ.get("ZHIZI_RESOURCES", "/usr/share/zhizi"))
LIB_ROOT = Path(os.environ.get("ZHIZI_LIB", "/usr/lib/zhizi"))
DATA_ROOT = Path(os.environ.get("XDG_DATA_HOME", str(Path.home() / ".local/share"))) / "zhizi"


class ZhiziEngine(IBus.Engine):
    __gtype_name__ = "ZhiziIBusEngine"

    def __init__(self, connection, object_path):
        super().__init__(connection=connection, object_path=object_path, has_focus_id=True)
        self.mapper = KeyMapper()
        self.backspace = BackspaceRepeatGuard()
        self.pages = {}
        self.epoch = 0
        self.focused = False
        self.secure = True
        self.failed = False
        self.view = {}
        self.app = "ibus.unknown"
        self.model_state = {"ranking": False, "generation": False}
        self.bridge = Bridge([str(LIB_ROOT / "zhizi-ibus-host"), str(RESOURCE_ROOT),
                              str(DATA_ROOT), str(LIB_ROOT / "libonnxruntime.so")],
                             GLib.idle_add, self._event)

    def _event(self, message):
        event = message.get("event")
        if event == "poll" and self.focused and not self.secure and not self.failed:
            self._send("poll")
        elif event == "models":
            self.model_state = message
            self._properties()
        elif event == "closed":
            self._failure()
        return GLib.SOURCE_REMOVE

    def _failure(self):
        self.failed = True
        if hasattr(self, "bridge"):
            self.bridge.close()
        self.mapper.reset()
        self.backspace.reset()
        self._render({})

    def _send(self, op, **fields):
        if self.failed:
            return None
        self.epoch += 1
        try:
            reply = self.bridge.request(op, self.epoch, **fields)
            if "view" in reply:
                self._render(reply["view"])
            return reply
        except (RuntimeError, ValueError):
            self._failure()
            return None

    def _render(self, view):
        self.view = view
        commit = view.get("commit", "")
        if commit and self.focused and not self.secure:
            self.commit_text(IBus.Text.new_from_string(commit))
        preedit = view.get("preedit", "") if self.focused and not self.secure else ""
        self.update_preedit_text_with_mode(IBus.Text.new_from_string(preedit),
            utf8_cursor_to_char(preedit, view.get("cursor_utf8", 0)), bool(preedit), IBus.PreeditFocusMode.CLEAR)
        candidates = view.get("candidates", []) if preedit else []
        page = view.get("page", 0)
        if not candidates or page == 0:
            self.pages = {}
        self.pages[page] = candidates[:5]
        prefix = [item for previous in range(page) for item in self.pages.get(previous, [])]
        following = view.get("following_candidates", []) if candidates else []
        rows = prefix + candidates[:5] + following[:5]
        table = IBus.LookupTable.new(5, 0, True, False)
        table.set_orientation(IBus.Orientation.VERTICAL)
        for index in range(5):
            table.append_label(IBus.Text.new_from_string(str(index + 1)))
        for candidate in rows:
            text = candidate["text"]
            if candidate.get("comment"):
                text += "  " + candidate["comment"]
            table.append_candidate(IBus.Text.new_from_string(text))
        if candidates:
            table.set_cursor_pos(len(prefix) + min(view.get("highlighted", 0), len(candidates) - 1))
        self.update_lookup_table(table, bool(candidates))
        self.update_auxiliary_text(IBus.Text.new_from_string(f"第{page + 1}页"), bool(candidates and page))

    def _content_type(self):
        purpose, hints = self.get_content_type()
        secure = secure_content(int(purpose), int(hints))
        if secure != self.secure:
            self.secure = secure
            self.mapper.reset()
            self.backspace.reset()
            self._send("security", secure=secure)
            if secure:
                self._render({})

    def _properties(self):
        if not self.focused:
            return
        properties = IBus.PropList()
        for name, label in [("settings", "知字设置与个人词")]:
            properties.append(IBus.Property(key=name, type=IBus.PropType.NORMAL,
                label=IBus.Text.new_from_string(label), tooltip=IBus.Text.new_from_string(label),
                sensitive=True, visible=True))
        self.register_properties(properties)

    def do_focus_in_id(self, object_path, client):
        self.focused = True
        self.secure = True
        self.mapper.reset()
        self.backspace.reset()
        self.app = (client or "ibus.unknown")[:256]
        self._send("focus", app=self.app)
        self._send("security", secure=True)
        self._content_type()
        self.get_surrounding_text()  # Request surrounding updates, without logging.
        self._properties()

    def do_focus_out_id(self, object_path):
        self.focused = False
        self.secure = True
        self.mapper.reset()
        self.backspace.reset()
        self._send("security", secure=True)
        self._send("reset")
        self._render({})

    def do_set_content_type(self, purpose, hints):
        # The signal's values are authoritative; get_content_type's cache may
        # still refer to the previous field during a transition.
        self.secure = secure_content(int(purpose), int(hints))
        self.mapper.reset()
        self.backspace.reset()
        self._send("security", secure=self.secure)
        if self.secure:
            self._render({})

    def do_set_surrounding_text(self, text, cursor_pos, anchor_pos):
        if self.focused and not self.secure:
            surrounding = text.get_text()
            # IBus positions count characters, not UTF-8 bytes.
            self._send("context", text=surrounding[:max(0, cursor_pos)][-32:])

    def do_process_key_event(self, keyval, keycode, state):
        if self.failed or not self.focused or self.secure:
            self.mapper.reset()
            self.backspace.reset()
            return False
        if self.backspace.swallow(keyval, state):
            return True
        before = bool(self.view.get("preedit"))
        event = self.mapper.map(keyval, state)
        if event is None:
            return False
        reply = self._send("key", **event)
        if keyval == 0xff08 and reply:
            self.backspace.observe(before, bool(reply.get("view", {}).get("preedit")))
        return bool(reply and reply.get("view", {}).get("consumed"))

    def _key(self, name, **fields):
        if self.focused and not self.secure:
            self._send("key", key=name, modifiers=0, **fields)

    def do_candidate_clicked(self, index, button, state):
        if button == 1 and 0 <= index < len(self.view.get("candidates", [])) and index < 5:
            self._key("Text", text=str(index + 1))

    def do_page_up(self):
        self._key("PageUp")

    def do_page_down(self):
        self._key("PageDown")

    def do_cursor_up(self):
        self._key("Up")

    def do_cursor_down(self):
        self._key("Down")

    def do_reset(self):
        self.mapper.reset()
        self.backspace.reset()
        self._send("reset")

    def do_disable(self):
        self.do_focus_out_id("")

    def do_enable(self):
        self._properties()

    def do_property_activate(self, name, state):
        if name == "settings":
            import subprocess
            subprocess.Popen(["/usr/bin/zhizi-settings"], stdin=subprocess.DEVNULL,
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    def do_destroy(self):
        if hasattr(self, "bridge"):
            self.bridge.close()
        super().do_destroy()


class Factory(IBus.Factory):
    def __init__(self, connection):
        super().__init__(connection=connection, object_path=IBus.PATH_FACTORY)
        self.serial = 0

    def do_create_engine(self, name):
        if name != "zhizi":
            raise ValueError("unknown engine")
        self.serial += 1
        return ZhiziEngine(self.get_connection(), "/org/freedesktop/IBus/Engine/Zhizi/" + str(self.serial))


def main():
    IBus.init()
    bus = IBus.Bus()
    loop = GLib.MainLoop()
    bus.connect("disconnected", lambda *_: loop.quit())
    factory = Factory(bus.get_connection())
    bus.request_name("org.freedesktop.IBus.Zhizi", 0)
    loop.run()


if __name__ == "__main__":
    main()
