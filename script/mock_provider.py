#!/usr/bin/env python3
"""Minimal OpenAI-compatible mock used for manual Re:Code smoke tests.

Serves streaming chat completions on POST /v1/chat/completions:
  request 1 -> tool call read(file_path="a.txt")
  request 2 -> final text
Then repeats the sequence.

Usage:
  python script/mock_provider.py --port 18888
  RECODE_CONFIG=... recode run "read a.txt" --auto
"""

import argparse
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

COUNTER = {"n": 0}


def sse(payload):
    return f"data: {json.dumps(payload)}\n\n".encode()


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        length = int(self.headers.get("content-length", "0"))
        body = self.rfile.read(length)
        request = json.loads(body or b"{}")
        turn = COUNTER["n"] % 2
        COUNTER["n"] += 1
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("connection", "close")
        self.end_headers()
        if turn == 0:
            self.wfile.write(
                sse(
                    {
                        "choices": [
                            {
                                "delta": {
                                    "tool_calls": [
                                        {
                                            "index": 0,
                                            "id": "call_1",
                                            "function": {
                                                "name": "read",
                                                "arguments": json.dumps({"file_path": "a.txt"}),
                                            },
                                        }
                                    ]
                                }
                            }
                        ]
                    }
                )
            )
            self.wfile.write(
                sse(
                    {
                        "choices": [{"delta": {}, "finish_reason": "tool_calls"}],
                        "usage": {"prompt_tokens": 50, "completion_tokens": 10, "total_tokens": 60},
                    }
                )
            )
        else:
            text = "The file says: hello from the mock provider."
            if "messages" in request and "tools" in request:
                text = "Done. I read the file."
            self.wfile.write(
                sse({"choices": [{"delta": {"content": text}, "finish_reason": "stop"}]})
            )
            self.wfile.write(
                sse(
                    {
                        "choices": [],
                        "usage": {"prompt_tokens": 80, "completion_tokens": 12, "total_tokens": 92},
                    }
                )
            )
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=18888)
    parser.add_argument("--host", default="127.0.0.1")
    args = parser.parse_args()
    server = ThreadingHTTPServer((args.host, args.port), Handler)
    print(f"mock provider on http://{args.host}:{args.port}/v1", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
