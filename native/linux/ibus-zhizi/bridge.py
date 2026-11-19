"""Private, bounded bridge. Only GLib's UI thread requests page mutations."""
import json
import queue
import subprocess
import threading


class Bridge:
    def __init__(self, command, schedule, on_event):
        self.process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.DEVNULL, text=True, encoding="utf-8", bufsize=1)
        self.pending = {}
        self.lock = threading.Lock()
        self.next_id = 0
        self.closed = False
        self.schedule = schedule
        self.on_event = on_event
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        try:
            while True:
                line = self.process.stdout.readline(65537)
                if not line or len(line) > 65536:
                    break
                message = json.loads(line)
                if "id" in message:
                    with self.lock:
                        waiter = self.pending.get(message["id"])
                    if waiter:
                        waiter.put(message)
                else:
                    self.schedule(self.on_event, message)
        except (OSError, ValueError):
            pass
        finally:
            with self.lock:
                self.closed = True
                waiters = list(self.pending.values())
            for waiter in waiters:
                waiter.put({"error": "bridge_closed"})
            self.schedule(self.on_event, {"event": "closed"})

    def request(self, operation, epoch, **fields):
        with self.lock:
            if self.closed:
                raise RuntimeError("bridge_closed")
            self.next_id += 1
            number = self.next_id
            waiter = queue.Queue(maxsize=2)
            self.pending[number] = waiter
        try:
            message = dict(fields, op=operation, id=number, epoch=epoch)
            encoded = json.dumps(message, ensure_ascii=False, separators=(",", ":")) + "\n"
            if len(encoded.encode("utf-8")) > 65536:
                raise ValueError("request_too_large")
            self.process.stdin.write(encoded)
            self.process.stdin.flush()
            reply = waiter.get(timeout=5)
            if reply.get("error"):
                raise RuntimeError(reply["error"])
            return reply
        except (OSError, queue.Empty):
            self.close()
            raise RuntimeError("bridge_unavailable") from None
        finally:
            with self.lock:
                self.pending.pop(number, None)

    def close(self):
        if self.process.poll() is None:
            # EOF lets the core confirm ordinary committed learning and finish
            # its private writes. A stuck child still has a bounded shutdown.
            try:
                self.process.stdin.close()
                self.process.wait(timeout=1)
            except (OSError, subprocess.TimeoutExpired):
                self.process.terminate()
                try:
                    self.process.wait(timeout=1)
                except subprocess.TimeoutExpired:
                    self.process.kill()
                    self.process.wait(timeout=1)
        self.closed = True
