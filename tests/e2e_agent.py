#!/usr/bin/env python3
"""E2E agent loop: wizard with fake Ollama -> @chat runs with real tool calls.

Proves: orchestrator loop, tool-call protocol, remember->RAG write,
memory_search recall in a later run, and the leash refusing /etc read.

Usage: HOME=<scratch> python3 tests/e2e_agent.py   (fake_ollama.py must be up)
"""
import os, pty, sys, time, select, subprocess, re, re

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

ANSI = re.compile(r"\x1b\[[0-9;]*[A-Za-z]|\x1b\][^\x07]*\x07")

def text():
    return ANSI.sub("", buf.decode(errors="replace"))

FAIL = []
def check(cond, name):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: FAIL.append(name)

DOWN = "\x1b[B"

print("=== 1. wizard: fake ollama backend ===")
m, p = spawn([])
drain(2.5)
send(DOWN + "\r")              # backend: ollama (index 1)
drain(1.5)
send("\r")                     # base url default 127.0.0.1:11434 (fake tags)
drain(1.5)
out = text()
check("qwen2.5" in out, "fake ollama models listed")
send("\r")                     # model: qwen2.5:0.5b-instruct
drain(1.5)
send("\r")                     # memory folders: empty
drain(1.5)
send("\r")                     # leash: leashed
drain(1.5)
send("\r")                     # tools: defaults
drain(1.5)
send("\r")                     # research profile: default skip
drain(2.5)
out = text()
check("Vault password" in out, "fresh vault password prompt")
send("e2e-password-123\r")
drain(2.0)
send("e2e-password-123\r")
drain(3.0)
check("Sealed" in text(), "sealed after wizard")
drain(1.5)
check("UNSEALED" in text(), "first session auto-continues into the @-shell")
send("exit\r")                  # leave cleanly; then re-launch for phase 2
drain(2.5)
p.wait(timeout=10)
check(os.path.exists(VAULT), "vault.dct created")

print("\n=== 2. unlock + agent runs with tool calls ===")
m, p = spawn([])
drain(2.5)
out = text()
check("Vault password" in out, "unlock prompt")
send("e2e-password-123\r")
drain(2.0)
out = text()
check("UNSEALED" in out, "vault unsealed")

print("-- @chat: remember tool call --")
send("@chat remember the access code is CAMEL-42-ALPHA\r")
drain(8.0)
out = text()
check("CAMEL-42-ALPHA is now in long-term memory" in out, "final report after remember tool call")
check("tool calls: 1" in out, "run stats show one tool call")

print("-- @chat: memory recall tool call --")
buf = b""
send("@chat what was the access code?\r")
drain(8.0)
out = text()
check("Source: my long-term memory" in out, "recalled from RAG in a later run")

print("-- @chat: leash denies /etc read --")
buf = b""
send("@chat read /etc/hostname for me\r")
drain(8.0)
out = text()
check("DENIED" in out, "tool result DENIED within run (leash held)")
check("Confirmed, I got the tool result" in out, "agent continued after denial and reported")

print("-- silence + forensics checks --")
out = text()
check("model_request" not in out, "no raw reasoning leaked to screen")
send("@status\r")
drain(1.5)
out = text()
mchunk = re.search(r"RAG chunks\s+(\d+)", out)
check(mchunk is not None and int(mchunk.group(1)) > 0, "RAG chunks recorded in vault (@status)")

send("exit\r")
drain(2.5)
check("Sealed" in text(), "sealed on exit")
p.wait(timeout=10)
check(not os.path.exists(STAGING), "staging wiped")

print(f"\n{'ALL PASS' if not FAIL else 'FAILURES: ' + str(FAIL)}")
sys.exit(0 if not FAIL else 1)
