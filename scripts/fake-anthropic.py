#!/usr/bin/env python3
"""A scripted stand-in for the Anthropic Messages API, for
scripts/smoke-claude-ask.sh. No model is called.

Usage: fake-anthropic.py <scratch dir>

Binds 127.0.0.1 on a port the kernel picks and prints it on stdout (one line).
Every request is appended to <scratch dir>/requests.jsonl as one JSON line
{"t", "path", "body", "stop_reason", "tool"}: `t` is the Unix time it was
answered, `stop_reason` that of the answer (null for paths other than
/v1/messages), `tool` the name of the tool the answer calls (else null).

POST /v1/messages:
  - a request offering the AskUserQuestion tool whose messages after the
    last assistant message hold the person's prompt (no tool_result; Claude
    Code may add a `system` message after it) is answered with one tool_use
    block calling AskUserQuestion (id "toolu_smoke1", the question below) and
    stop_reason "tool_use". While <scratch dir>/watch.id holds an artifact
    ID, it is answered instead with a call of the Clax plugin's `watch` tool
    on that artifact, replies off (id "toolu_smoke_watch"), so the session
    has a page before it asks; while that tool is not offered yet, it first
    calls WaitForMcpServers (id "toolu_smoke_mcp") and, given its result,
    the watch tool (when the tool is still not offered, or the call fails,
    it says so in its text and in <scratch dir>/ask.unwatched);
  - a request carrying that watch call's tool_result is held: the fake
    creates <scratch dir>/ask.held and reads the FIFO <scratch dir>/ask.go
    until the script writes to it (what the script does meanwhile happens
    after the watch and before the question), then answers with the
    AskUserQuestion call;
  - a request whose messages after the last assistant message carry the
    AskUserQuestion call's tool_result is answered
    with a text block echoing the result, and stop_reason "end_turn";
  - any other request (a title or summary request) gets the text "ok".
Answers are server-sent events in the Messages streaming format when the body
asks for "stream": true, else one JSON message. POST /v1/messages/count_tokens
answers {"input_tokens": 1}. Every other path answers {} with 200.
"""
import http.server
import json
import os
import sys
import threading
import time

SCRATCH = sys.argv[1]
LOG = os.path.join(SCRATCH, "requests.jsonl")
LOG_LOCK = threading.Lock()
TOOL_ID = "toolu_smoke1"
WATCH_TOOL_ID = "toolu_smoke_watch"
MCP_WAIT_ID = "toolu_smoke_mcp"
WATCH_FILE = os.path.join(SCRATCH, "watch.id")
HELD = os.path.join(SCRATCH, "ask.held")
UNWATCHED = os.path.join(SCRATCH, "ask.unwatched")
GO = os.path.join(SCRATCH, "ask.go")
QUESTION = {
    "questions": [{
        "question": "Which layout?",
        "header": "Layout",
        "options": [
            {"label": "Two columns", "description": "a"},
            {"label": "One column", "description": "b"},
        ],
        "multiSelect": False,
    }]
}


def text_of(content):
    """The text of a message's content, tool results included."""
    if isinstance(content, str):
        return content
    out = []
    for block in content or []:
        if block.get("type") == "text":
            out.append(block.get("text", ""))
        elif block.get("type") == "tool_result":
            out.append(text_of(block.get("content")))
    return "\n".join(out)


def tool_result(message, tool_id=TOOL_ID):
    """The tool_result block for `tool_id` in `message`, or None."""
    content = message.get("content")
    if not isinstance(content, list):
        return None
    for block in content:
        if block.get("type") == "tool_result" and block.get("tool_use_id") == tool_id:
            return block
    return None


def page_to_watch():
    """The artifact ID in WATCH_FILE, or None."""
    try:
        with open(WATCH_FILE) as f:
            return f.read().strip() or None
    except FileNotFoundError:
        return None


def unwatched(text):
    """The answer when the watch cannot be made, also written to UNWATCHED
    for the script."""
    with open(UNWATCHED, "w") as f:
        f.write(text + "\n")
    return [{"type": "text", "text": text}], "end_turn"


def hold():
    """Blocks until the script writes to the FIFO GO, having created HELD to
    say it waits."""
    open(HELD, "w").close()
    with open(GO) as f:
        f.read()


ASK = [{"type": "tool_use", "id": TOOL_ID, "name": "AskUserQuestion", "input": QUESTION}]


def watch_call(tools, page, waited):
    """The call of the Clax `watch` tool on `page`; while it is not offered
    yet, one call of WaitForMcpServers first (Claude Code may send the
    person's prompt before the plugin's MCP server has connected)."""
    watch = next((t for t in tools if t and "clax" in t and t.endswith("__watch")), None)
    if watch is not None:
        return [{"type": "tool_use", "id": WATCH_TOOL_ID, "name": watch,
                 "input": {"url_or_id": page, "replies": False}}], "tool_use"
    if not waited and "WaitForMcpServers" in tools:
        return [{"type": "tool_use", "id": MCP_WAIT_ID, "name": "WaitForMcpServers", "input": {}}], "tool_use"
    return unwatched("The Clax watch tool was not offered: " + ", ".join(t for t in tools if t))


def reply_for(body):
    """(content blocks, stop_reason) for a Messages request body. Claude Code
    may end the list with a `system` message (hook context) after the
    person's prompt, so the turn is judged by the messages after the last
    assistant message, not by the last message alone."""
    messages = body.get("messages") or []
    tools = [t.get("name") for t in body.get("tools") or []]
    last_assistant = max((i for i, m in enumerate(messages) if m.get("role") == "assistant"), default=-1)
    tail = messages[last_assistant + 1:]
    for message in tail:
        result = tool_result(message)
        if result is not None:
            prefix = "ERROR " if result.get("is_error") else ""
            return [{"type": "text", "text": "Tool result: " + prefix + text_of(result.get("content"))}], "end_turn"
    for message in tail:
        watched = tool_result(message, WATCH_TOOL_ID)
        if watched is not None:
            if watched.get("is_error"):
                return unwatched("Watch failed: " + text_of(watched.get("content")))
            hold()
            return ASK, "tool_use"
    page = page_to_watch()
    if page is not None and any(tool_result(m, MCP_WAIT_ID) is not None for m in tail):
        return watch_call(tools, page, True)
    prompt = any(m.get("role") == "user" and tool_result(m) is None and not any(
        isinstance(b, dict) and b.get("type") == "tool_result" for b in (m.get("content") if isinstance(m.get("content"), list) else []))
        for m in tail)
    if "AskUserQuestion" in tools and prompt:
        if page is None:
            return ASK, "tool_use"
        return watch_call(tools, page, False)
    return [{"type": "text", "text": "ok"}], "end_turn"


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def body(self):
        n = int(self.headers.get("content-length") or 0)
        raw = self.rfile.read(n) if n else b""
        try:
            parsed = json.loads(raw) if raw else None
        except ValueError:
            parsed = raw.decode("utf-8", "replace")
        return parsed

    def log(self, body, stop, tool=None):
        with LOG_LOCK, open(LOG, "a") as f:
            f.write(json.dumps({"t": time.time(), "path": self.path, "body": body, "stop_reason": stop, "tool": tool}) + "\n")

    def json(self, value):
        data = json.dumps(value).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self.log(None, None)
        self.json({})

    def do_POST(self):
        raw = self.body()
        body = raw if isinstance(raw, dict) else {}
        path = self.path.split("?", 1)[0]
        if path != "/v1/messages":
            self.log(raw, None)
            return self.json({"input_tokens": 1} if path == "/v1/messages/count_tokens" else {})
        content, stop = reply_for(body)
        self.log(raw, stop, next((b["name"] for b in content if b["type"] == "tool_use"), None))
        model = body.get("model") or "claude-fake"
        usage = {"input_tokens": 1, "output_tokens": 1}
        msg_id = "msg_fake_%d" % threading.get_ident()
        if not body.get("stream"):
            return self.json({"id": msg_id, "type": "message", "role": "assistant", "model": model,
                              "content": content, "stop_reason": stop, "stop_sequence": None, "usage": usage})
        events = [("message_start", {"type": "message_start", "message": {
            "id": msg_id, "type": "message", "role": "assistant", "model": model, "content": [],
            "stop_reason": None, "stop_sequence": None, "usage": usage}})]
        for i, block in enumerate(content):
            if block["type"] == "text":
                events.append(("content_block_start", {"type": "content_block_start", "index": i,
                                                       "content_block": {"type": "text", "text": ""}}))
                events.append(("content_block_delta", {"type": "content_block_delta", "index": i,
                                                       "delta": {"type": "text_delta", "text": block["text"]}}))
            else:
                events.append(("content_block_start", {"type": "content_block_start", "index": i,
                                                       "content_block": {**block, "input": {}}}))
                events.append(("content_block_delta", {"type": "content_block_delta", "index": i,
                                                       "delta": {"type": "input_json_delta",
                                                                 "partial_json": json.dumps(block["input"])}}))
            events.append(("content_block_stop", {"type": "content_block_stop", "index": i}))
        events.append(("message_delta", {"type": "message_delta",
                                         "delta": {"stop_reason": stop, "stop_sequence": None},
                                         "usage": {"output_tokens": 1}}))
        events.append(("message_stop", {"type": "message_stop"}))
        data = "".join("event: %s\ndata: %s\n\n" % (name, json.dumps(ev)) for name, ev in events).encode()
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("cache-control", "no-cache")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


def main():
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    print(server.server_address[1], flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
