#!/usr/bin/env python3
"""E2E: drive the full first-launch wizard over a pty with settle-based pacing.

Usage: HOME=<scratch> python3 tests/e2e_wizard.py
Must run with a FRESH HOME (the script asserts no vault exists yet).
"""
import os, pty, sys, time, select, subprocess, re

BIN = os.path.join(os.path.dirname(__file__), "..", "target", "release", "decyphertek")
HOME = os.environ["HOME"]
VAULT = os.path.join(HOME, ".decyphertek.ai", "vault.dct")
STAGING = os.path.join(HOME, ".decyphertek.ai", "staging")

if os.path.exists(VAULT):
    print("vault already exists — wipe HOME first"); sys.exit(2)

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

def send(text):
    os.write(m, text.encode())

FAIL = []
def check(cond, name):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: FAIL.append(name)

ANSI = re.compile(r"\x1b\[[0-9;]*[A-Za-z]|\x1b\][^\x07]*\x07")

def text():
    return ANSI.sub("", buf.decode(errors="replace"))

print("=== 1. first launch: fresh HOME -> wizard ===")
m, p = spawn([])
drain(2.5)                     # banner + welcome + backend select
out = text()
check("ADMINOTAUR" in out, "welcome names the adminotaur agent")
check("Choose the brain" in out, "backend prompt")
send("\r")                     # OpenRouter
drain(1.5)                     # api key password prompt
send("sk-or-v1-dummy-key-e2e\r")
drain(6.0)                     # model select + live probe
out = text()
check("Default model" in out, "model prompt shown")
send("\r")                     # ~z-ai/glm-flash-latest (default model)
drain(1.5)                     # memory folders question
send("\r")                     # empty -> no grant question, no leash skip
drain(1.5)
out = text()
check("The Leash" in out or "leash" in out.lower(), "leash prompt appeared")
send("\r")                     # leashed
drain(1.5)                     # tool multiselect
send("\r")                     # defaults: web_search + read_files
drain(1.5)                     # research profile confirm
out = text()
check("research profile" in out.lower(), "research profile prompt appeared")
send("\r")                     # default: skip creating one now
drain(3.0)                     # next: fresh-install password
out = text()
check("Vault password" in out, "new vault password prompt")
send("e2e-password-123\r")
drain(2.0)
out = text()
check("Repeat" in out, "password repeat prompt")
send("e2e-password-123\r")
drain(4.0)                     # seal, then straight into the @-shell
out = text()
check("decyphertek.ai:~$" in out, "first session auto-continues into the classic prompt")

send("exit\r")                  # seal on exit from the first session
drain(2.5)
out = text()
check("sealed" in out.lower(), "seal on first-session exit")

rc = p.wait(timeout=10)
check(rc == 0, "process exited cleanly")
check(os.path.exists(VAULT), "vault.dct exists")
check(not os.path.exists(STAGING), "staging wiped")

with open(VAULT, "rb") as f:
    hdr = f.read(8)
check(hdr == b"DCTVAULT", "vault magic header")

print("\n=== 2. second launch: unlock, shell, seal ===")
m, p = spawn([])
drain(2.5)
out = text()
check("Vault password" in out, "unlock prompt")
send("wrong-password-XXX\r")
drain(2.0)
out = text()
check("Wrong password" in out, "wrong password rejected")
send("e2e-password-123\r")
drain(2.0)
out = text()
check("decyphertek.ai:~$" in out, "vault unsealed — classic prompt")

send("@status\r")
drain(1.5)
out = text()
check("adminotaur" in out and "openrouter" in out, "@status shows agent + backend")
check("leashed" in out, "@status shows leash")

send("@wiki list\r")
drain(1.5)
out = text()
check("operative-handbook" in out, "baseline wiki present in vault")

send("@leash unleashed\r")
drain(1.5)
out = text()
check("off" in out, "@leash switches to unleashed")
send("@leash leashed\r")
drain(1.0)

print("-- classic terminal: passthrough + cd built-in --")
send("printf passthrough-ok\\n\r")
drain(1.0)
out = text()
check("passthrough-ok" in out, "regular commands run untouched")
send("cd /\r")
drain(1.0)
out = text()
check("decyphertek.ai:/$" in out, "cd built-in moves the prompt path")
send("cd\r")
drain(1.0)
out = text()
check("decyphertek.ai:~$" in out, "bare cd returns home")
send("cd ~ && printf ct-fallthrough\\n\r")
drain(1.0)
out = text()
check("ct-fallthrough" in out, "cd with shell syntax still runs via the shell")

send("exit\r")
drain(2.5)
out = text()
check("sealed" in out.lower(), "seal on exit")
rc = p.wait(timeout=10)
check(rc == 0, "clean exit")
check(not os.path.exists(STAGING), "staging wiped after exit")

print(f"\n{'ALL PASS' if not FAIL else 'FAILURES: ' + str(FAIL)}")
sys.exit(0 if not FAIL else 1)
