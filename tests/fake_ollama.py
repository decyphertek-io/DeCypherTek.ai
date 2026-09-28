#!/usr/bin/env python3
"""Fake Ollama-compatible server for e2e: OpenAI-style /v1/chat/completions
plus /api/tags. Drives a deterministic tool-call conversation:

  run 1 "remember the access code ..."  -> tool call: remember(...)
  run 2 "what was the access code?"     -> tool call: memory_search(...)
  run 3 "read /etc/hostname for me"     -> tool call: read_file(/etc/hostname)

Each conversation continues until a role=tool message arrives, then final text.
"""
import json
from http.server import BaseHTTPRequestHandler, HTTPServer

def finish(content):
    return {
        "choices": [{
            "message": {
                "role": "assistant",
                "content": content,
                "tool_calls": None,
            },
            "finish_reason": "stop",
        }]
    }

def call(name, args, cid):
    return {
        "choices": [{
            "message": {
                "role": "assistant",
                "content": None,
                "tool_calls": [{
                    "id": cid,
                    "type": "function",
                    "function": {"name": name, "arguments": json.dumps(args)},
                }],
            },
            "finish_reason": "tool_calls",
        }]
    }

class Handler(BaseHTTPRequestHandler):
    def _send(self, obj, code=200):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == "/api/tags":
            self._send({"models": [{"name": "qwen2.5:0.5b-instruct"}, {"name": "llama3.2:1b"}]})
        elif self.path == "/v1/models":
            self._send({"data": [{"id": "qwen2.5:0.5b-instruct"}]})
        else:
            self._send({}, 404)

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        req = json.loads(self.rfile.read(length) or b"{}")
        messages = req.get("messages", [])
        tools_enabled = bool(req.get("tools"))
        user_text = " ".join(m.get("content") or "" for m in messages if m.get("role") == "user")
        has_tool_results = any(m.get("role") == "tool" for m in messages)

        if not tools_enabled:
            self._send(finish("tools not offered but replying anyway")); return

        # Continuations: a tool result message exists -> produce the final.
        # NOTE: recall runs feed chatlog chunks back via RAG, so raw word
        # matching is fragile — match the specific task phrasing first.
        if has_tool_results:
            last_result = next(m.get("content") or "" for m in reversed(messages) if m.get("role") == "tool")
            if "what was the access code" in user_text.lower():
                self._send(finish("The access code is CAMEL-42-ALPHA. (Source: my long-term memory — you told me earlier.)"))
            elif "remember the access code" in user_text.lower():
                self._send(finish("The access code CAMEL-42-ALPHA is now in long-term memory. Mission accomplished."))
            else:
                self._send(finish("Confirmed, I got the tool result: " + last_result[:200]))
            return

        # First turn of each conversation: pick the tool to exercise.
        if "what was the access code" in user_text.lower():
            self._send(call("memory_search", {"query": "access code"}, "call-2"))
        elif "remember the access code" in user_text.lower():
            self._send(call("remember", {"knowledge": "the access code is CAMEL-42-ALPHA"}, "call-1"))
        elif "/etc/hostname" in user_text:
            self._send(call("read_file", {"path": "/etc/hostname"}, "call-3"))
        else:
            self._send(finish("No tool needed for: " + user_text[:120]))

    def log_message(self, *a):
        pass

if __name__ == "__main__":
    HTTPServer(("127.0.0.1", 11434), Handler).serve_forever()
