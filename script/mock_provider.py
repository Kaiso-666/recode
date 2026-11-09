#!/usr/bin/env python3
"""Minimal OpenAI-compatible mock used for manual Re:Code smoke tests.

Serves streaming chat completions on POST /v1/chat/completions:
  normal turn 1 -> tool call read(file_path="a.txt")
  normal turn 2 -> final text
  summarizer   -> a canned compaction summary (does not advance the turn)

Requests are optionally recorded with --log for debugging what the agent sent.

Usage:
  python script/mock_provider.py --port 18888 --log requests.jsonl
  recode run "read a.txt" --auto
"""

import argparse
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

COUNTER = {"n": 0}
LOCK = threading.Lock()
LOG_PATH = None
SCENARIO = "read"


def sse(payload):
    return f"data: {json.dumps(payload)}\n\n".encode()


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        length = int(self.headers.get("content-length", "0"))
        body = self.rfile.read(length)
        request = json.loads(body or b"{}")
        system = request.get("system", "")
        messages = request.get("messages", [])
        if not system:
            for message in messages:
                if message.get("role") == "system":
                    system = message.get("content", "")
                    break
        if isinstance(system, list):
            system = " ".join(part.get("text", "") for part in system)
        summarize = "context compactor" in system
        if LOG_PATH:
            with open(LOG_PATH, "a", encoding="utf-8") as log:
                log.write(
                    json.dumps(
                        {
                            "summarize": summarize,
                            "messages": len(messages),
                            "roles": [message.get("role") for message in messages],
                            "first": str(messages[0].get("content"))[:120] if messages else "",
                            "last": str(messages[-1].get("content"))[:300] if messages else "",
                            "system": system[:200],
                        }
                    )
                    + "\n"
                )

        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("connection", "close")
        self.end_headers()

        if summarize:
            self.wfile.write(
                sse(
                    {
                        "choices": [
                            {
                                "delta": {
                                    "content": "SUMMARY: the user worked in a small project and read a.txt."
                                },
                                "finish_reason": "stop",
                            }
                        ]
                    }
                )
            )
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
            return

        with LOCK:
            turn = COUNTER["n"] % 2
            COUNTER["n"] += 1

        if turn == 0:
            if SCENARIO == "question":
                arguments = {
                    "questions": [
                        {
                            "question": "Which database should the project use?",
                            "header": "Database",
                            "options": [
                                {"label": "SQLite", "description": "Embedded, zero setup."},
                                {"label": "Postgres", "description": "Server, needs a service."},
                            ],
                        }
                    ]
                }
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
                                                    "name": "question",
                                                    "arguments": json.dumps(arguments),
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
                            "usage": {
                                "prompt_tokens": 50,
                                "completion_tokens": 10,
                                "total_tokens": 60,
                            },
                        }
                    )
                )
                self.wfile.write(b"data: [DONE]\n\n")
                self.wfile.flush()
                return
            if SCENARIO == "write":
                tool_name = "write"
                arguments = {"file_path": "a.txt", "content": "changed by mock"}
            else:
                tool_name = "read"
                arguments = {"file_path": "a.txt"}
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
                                                "name": tool_name,
                                                "arguments": json.dumps(arguments),
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
            text = {
                "question": "Done. I asked the question.",
                "write": "Done. I wrote the file.",
            }.get(SCENARIO, "Done. I read the file.")
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
    global LOG_PATH, SCENARIO
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=18888)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--log")
    parser.add_argument("--scenario", choices=["read", "question", "write"], default="read")
    args = parser.parse_args()
    LOG_PATH = args.log
    SCENARIO = args.scenario
    server = ThreadingHTTPServer((args.host, args.port), Handler)
    print(
        f"mock provider on http://{args.host}:{args.port}/v1 ({args.scenario})",
        flush=True,
    )
    server.serve_forever()


if __name__ == "__main__":
    main()
