#!/usr/bin/env python3
"""Drive the Re:Code TUI in a real ConPTY and assert on the rendered screen.

Usage:
  python script/tui_smoke.py --project <dir> --config <dir> --data <dir> [--permission]

Requires pywinpty. The provider must be running (script/mock_provider.py).
"""

import argparse
import os
import re
import sys
import threading
import time

from winpty import PtyProcess

ANSI = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]|\x1b\][^\x07]*\x07|\x1b[()][A-Z0-9]|\r")


class Session:
    def __init__(self, command, cwd, env, rows=40, cols=120):
        self.process = PtyProcess.spawn(
            command, cwd=cwd, env=env, dimensions=(rows, cols)
        )
        self.buffer = []
        self.lock = threading.Lock()
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()

    def _read(self):
        while self.process.isalive():
            try:
                data = self.process.read(4096)
            except EOFError:
                break
            except Exception:
                break
            if not data:
                break
            with self.lock:
                self.buffer.append(data)

    def screen(self):
        with self.lock:
            return ANSI.sub("", "".join(self.buffer))

    def send(self, text):
        self.process.write(text)

    def wait_for(self, needle, timeout=15.0):
        deadline = time.time() + timeout
        while time.time() < deadline:
            if needle in self.screen():
                return True
            time.sleep(0.15)
        return False

    def close(self):
        try:
            self.process.terminate()
        except Exception:
            pass


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--project", required=True)
    parser.add_argument("--config", required=True)
    parser.add_argument("--data", required=True)
    parser.add_argument("--binary", default=r"C:\Users\meow\projects\recode\target\debug\recode.exe")
    parser.add_argument("--permission", action="store_true")
    args = parser.parse_args()

    env = os.environ.copy()
    env["RECODE_CONFIG"] = args.config
    env["RECODE_DATA"] = args.data
    env["RECODE_LOG"] = "warn"

    session = Session(args.binary, args.project, env)
    failures = []
    try:
        if not session.wait_for("Welcome to Re:Code"):
            failures.append("welcome screen did not render")
        session.send("read a.txt\r")
        if args.permission:
            if not session.wait_for("permission", timeout=15):
                failures.append("permission modal did not appear")
            session.send("y")
        if not session.wait_for("Done. I read the file.", timeout=20):
            failures.append("assistant reply did not render")
        # Help overlay.
        session.send("\x1bOP")  # F1
        if not session.wait_for("send the message", timeout=5):
            failures.append("help overlay did not render")
        session.send("\x1b")  # close overlay
        time.sleep(0.4)
        session.send("\x03")  # quit when idle
        time.sleep(0.8)
    finally:
        screen = session.screen()
        session.close()

    if failures:
        print("FAIL:")
        for failure in failures:
            print(" -", failure)
        print("---- screen ----")
        print(screen[-6000:])
        return 1
    print("TUI smoke test passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
