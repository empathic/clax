#!/usr/bin/env python3
"""A scripted stand-in for the Anthropic Messages API, for the scripts that
run ClaxBot on a real `clax serve` (scripts/smoke-bot-surfaces.sh): the
Python twin of crates/clax-testkit/src/fake_messages.rs and
web/e2e/fake-messages.ts. No model is called.

Usage: fake-messages.py <turns.json> <scratch dir>

<turns.json> is a JSON list of the assistant turns to answer, in order,
each one of
  {"text": "<narration>"}                                   stop_reason end_turn
  {"script": {"id": "tu_1", "title": "…", "code": "…"}}     narrates, then run_script
  {"finish": "<summary>"}                                   finish {summary}

Binds 127.0.0.1 on a port the kernel picks and prints it on stdout (one
line). POST /v1/messages answers each request with the next turn as the
streaming format's server-sent events, and a 400 once the turns are spent;
each request is appended to <scratch dir>/requests.jsonl as one JSON line
{"t", "turn"} (`turn` is the index of the turn it was answered with, null
for the 400). GET /v1/models lists two models. Every other path is 404.
Python 3's standard library alone.
"""
import http.server
import json
import os
import sys
import threading
import time

TURNS_FILE, SCRATCH = sys.argv[1], sys.argv[2]
LOG = os.path.join(SCRATCH, "requests.jsonl")
LOCK = threading.Lock()
with open(TURNS_FILE, encoding="utf-8") as f:
    TURNS = json.load(f)
NEXT = [0]

MODELS = {
    "data": [
        {"type": "model", "id": "claude-sonnet-5-5", "display_name": "Claude Sonnet 5.5",
         "created_at": "2026-09-01T00:00:00Z"},
        {"type": "model", "id": "claude-haiku-5-5", "display_name": "Claude Haiku 5.5",
         "created_at": "2026-09-01T00:00:00Z"},
    ],
    "has_more": False, "first_id": "claude-sonnet-5-5", "last_id": "claude-haiku-5-5",
}


def blocks_of(turn):
    """(content blocks, stop_reason) of one turn of <turns.json>."""
    if "text" in turn:
        return [{"type": "text", "text": turn["text"]}], "end_turn"
    if "script" in turn:
        s = turn["script"]
        return [
            {"type": "text", "text": f"Running {s['title']}."},
            {"type": "tool_use", "id": s["id"], "name": "run_script",
             "input": {"title": s["title"], "code": s["code"]}},
        ], "tool_use"
    if "finish" in turn:
        return [{"type": "tool_use", "id": "tu_finish", "name": "finish",
                 "input": {"summary": turn["finish"]}}], "tool_use"
    raise ValueError(f"a turn that is none of text, script and finish: {turn}")


def sse(turn):
    blocks, stop = blocks_of(turn)
    out = []

    def ev(name, data):
        out.append(f"event: {name}\ndata: {json.dumps(data)}\n\n")

    ev("message_start", {"type": "message_start", "message": {
        "id": "msg_fake", "model": "claude-sonnet-5-5",
        "usage": {"input_tokens": 1000, "cache_read_input_tokens": 0,
                  "cache_creation_input_tokens": 0, "output_tokens": 1}}})
    for i, b in enumerate(blocks):
        if b["type"] == "text":
            ev("content_block_start", {"type": "content_block_start", "index": i,
                                       "content_block": {"type": "text", "text": ""}})
            ev("content_block_delta", {"type": "content_block_delta", "index": i,
                                       "delta": {"type": "text_delta", "text": b["text"]}})
        else:
            ev("content_block_start", {"type": "content_block_start", "index": i,
                                       "content_block": {"type": "tool_use", "id": b["id"],
                                                         "name": b["name"], "input": {}}})
            ev("content_block_delta", {"type": "content_block_delta", "index": i,
                                       "delta": {"type": "input_json_delta",
                                                 "partial_json": json.dumps(b["input"])}})
        ev("content_block_stop", {"type": "content_block_stop", "index": i})
    ev("message_delta", {"type": "message_delta", "delta": {"stop_reason": stop},
                         "usage": {"output_tokens": 200}})
    ev("message_stop", {"type": "message_stop"})
    return "".join(out).encode()


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def send(self, status, ctype, body):
        self.send_response(status)
        self.send_header("content-type", ctype)
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path.startswith("/v1/models"):
            self.send(200, "application/json", json.dumps(MODELS).encode())
        else:
            self.send(404, "application/json", b"{}")

    def do_POST(self):
        self.rfile.read(int(self.headers.get("content-length") or 0))
        if not self.path.startswith("/v1/messages"):
            self.send(404, "application/json", b"{}")
            return
        with LOCK:
            index = NEXT[0] if NEXT[0] < len(TURNS) else None
            if index is not None:
                NEXT[0] += 1
            with open(LOG, "a", encoding="utf-8") as log:
                log.write(json.dumps({"t": time.time(), "turn": index}) + "\n")
        if index is None:
            self.send(400, "application/json", json.dumps({
                "type": "error",
                "error": {"type": "invalid_request_error", "message": "no scripted turn left"},
            }).encode())
            return
        self.send(200, "text/event-stream", sse(TURNS[index]))


server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
print(server.server_address[1], flush=True)
server.serve_forever()
