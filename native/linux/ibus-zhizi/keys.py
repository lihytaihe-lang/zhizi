"""X11/IBus events to the host's small key vocabulary; no GI dependency."""
import unicodedata

SHIFT, LOCK, CONTROL, ALT, MOD4 = 1, 2, 4, 8, 64
SUPER, META, RELEASE = 1 << 26, 1 << 28, 1 << 30
SHIFTS = (0xffe1, 0xffe2)
SPECIAL = {0xff08: "Backspace", 0xff0d: "Enter", 0xff8d: "Enter", 0xff1b: "Escape",
           0xff51: "Left", 0xff52: "Up", 0xff53: "Right", 0xff54: "Down",
           0xff55: "PageUp", 0xff56: "PageDown", 0x20: "Space"}


def secure_content(purpose, hints):
    return purpose in (8, 9) or bool(hints & ((1 << 11) | (1 << 12)))


def utf8_cursor_to_char(text, offset):
    encoded = text.encode("utf-8")
    return len(encoded[:max(0, min(len(encoded), offset))].decode("utf-8", errors="ignore"))


class KeyMapper:
    def __init__(self):
        self.reset()

    def reset(self):
        self.held_shifts = set()
        self.tap = None

    def map(self, keyval, state):
        if not isinstance(keyval, int) or not isinstance(state, int) or keyval < 0 or state < 0:
            self.reset()
            return None
        shortcut = state & (CONTROL | ALT | MOD4 | SUPER | META)
        if keyval in SHIFTS:
            if state & RELEASE:
                eligible = self.tap == keyval and not shortcut
                self.held_shifts.discard(keyval)
                self.tap = None
                return {"key": "ShiftTap", "modifiers": 0} if eligible else None
            if keyval not in self.held_shifts:
                self.tap = keyval if not self.held_shifts and not shortcut else None
                self.held_shifts.add(keyval)
            return None
        if state & RELEASE:
            return None
        self.tap = None
        if keyval == 0xff08 and state & CONTROL and not state & (ALT | MOD4 | SUPER | META):
            return {"key": "BlacklistCandidate" if state & SHIFT else "DeleteCandidate", "modifiers": 0}
        if shortcut:
            return None
        if keyval == 0xffe5:
            return {"key": "CapsLock", "modifiers": 0}
        modifiers = (1 if state & SHIFT else 0) | (16 if state & LOCK else 0)
        if keyval in SPECIAL:
            return {"key": SPECIAL[keyval], "modifiers": modifiers}
        if keyval in (0x2d, 0x3d) and not state & SHIFT:
            return {"key": "PageUp" if keyval == 0x2d else "PageDown", "modifiers": modifiers}
        scalar = keyval & 0xffffff if keyval & 0xff000000 == 0x01000000 else keyval
        if not (0x21 <= scalar <= 0x7e or 0xa0 <= scalar <= 0xff or keyval & 0xff000000 == 0x01000000):
            return None
        if scalar > 0x10ffff or 0xd800 <= scalar <= 0xdfff:
            return None
        text = chr(scalar)
        if unicodedata.category(text).startswith("C"):
            return None
        return {"key": "Text", "text": text, "modifiers": modifiers}


class BackspaceRepeatGuard:
    """A held key stops at the composition boundary. A fresh press may edit text."""
    def __init__(self):
        self.reset()

    def reset(self):
        self.held = False
        self.emptied = False

    def swallow(self, keyval, state):
        if keyval == 0xff08:
            if state & RELEASE:
                self.reset()
                return False
            repeated = self.held
            self.held = True
            return repeated and self.emptied
        if not state & RELEASE:
            self.reset()
        return False

    def observe(self, before, after):
        if self.held and before and not after:
            self.emptied = True
