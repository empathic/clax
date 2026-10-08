#!/usr/bin/env python3
"""A scripted stand-in for the Anthropic Messages API, for
scripts/smoke-claude-ask.sh. No model is called.

Usage: fake-anthropic.py <scratch dir>

Binds 127.0.0.1 on a port the kernel picks and prints it on stdout (one line).
Every request is appended to <scratch dir>/requests.jsonl as one JSON line
{"t", "path", "body", "stop_reason"}: `t` is the Unix time it was answered,
`stop_reason` that of the answer (null for paths other than /v1/messages).

POST /v1/messages:
  - a request offering the AskUserQuestion tool whose messages after the
    last assistant message hold the person's prompt (no tool_result; Claude
    Code may add a `system` message after it) is answered with one tool_use
    block calling AskUserQuestion (id "toolu_smoke1", the question below) and
    stop_reason "tool_use";
  - a request whose messages after the last assistant message carry that
    call's tool_result is answered
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


def tool_result(message):
    """The tool_result block for TOOL_ID in `message`, or None."""
    content = message.get("content")
    if not isinstance(content, list):
        return None
    for block in content:
        if block.get("type") == "tool_result" and block.get("tool_use_id") == TOOL_ID:
            return block
    return None


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
    prompt = any(m.get("role") == "user" and tool_result(m) is None and not any(
        isinstance(b, dict) and b.get("type") == "tool_result" for b in (m.get("content") if isinstance(m.get("content"), list) else []))
        for m in tail)
    if "AskUserQuestion" in tools and prompt:
        return [{"type": "tool_use", "id": TOOL_ID, "name": "AskUserQuestion", "input": QUESTION}], "tool_use"
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

    def log(self, body, stop):
        with LOG_LOCK, open(LOG, "a") as f:
            f.write(json.dumps({"t": time.time(), "path": self.path, "body": body, "stop_reason": stop}) + "\n")

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
        self.log(raw, stop)
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
