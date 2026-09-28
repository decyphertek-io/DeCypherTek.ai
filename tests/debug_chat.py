#!/usr/bin/env python3
"""Debug: one @chat run, dump the raw session buffer."""
import os, pty, sys, time, select, subprocess, re

BIN = os.path.join(os.path.dirname(__file__), "..", "target", "release", "decyphertek")
HOME = os.environ["HOME"]

def spawn(args):
    env = dict(os.environ); env["TERM"] = "xterm-256color"
    m, s = pty.openpty()
    p = subprocess.Popen([BIN] + args, stdin=s, stdout=s, stderr=s, env=env, cwd=HOME, close_fds=True)
    os.close(s)
    return m, p

buf = b""
def drain(t=1.0):
    global buf
    end = time.time() + t
    while time.time() < end:
        r, _, _ = select.select([m], [], [], 0.2)
        if r:
            try: buf += os.read(m, 65536)
            except OSError: break

def send(t):
    os.write(m, t.encode())

m, p = spawn([])
drain(2.5)
send("e2e-password-123\r")
drain(2.0)
buf = b""
send("@chat remember the access code is CAMEL-42-ALPHA\r")
drain(8.0)
print("=== RAW SESSION ===")
import re as _re
print(_re.compile(r"\x1b\[[0-9;]*[A-Za-z]").sub("", buf.decode(errors="replace")))
send("exit\r")
p.wait(timeout=5)
