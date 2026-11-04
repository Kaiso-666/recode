#!/usr/bin/env python3
"""Tiny LSP server used by the integration tests and manual smoke tests.

It speaks Content-Length framed JSON-RPC on stdio and publishes one diagnostic
for every line containing the word `bad`.
"""

import json
import sys


def read_message():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.decode("utf-8", "replace").strip()
        if not line:
            break
        name, _, value = line.partition(":")
        if name.lower() == "content-length":
            length = int(value.strip())
    if length is None:
        return None
    return json.loads(sys.stdin.buffer.read(length))


def send(message):
    body = json.dumps(message).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("ascii"))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()


def diagnostics_for(uri, text):
    diagnostics = []
    for line_number, line in enumerate(text.splitlines()):
        column = line.find("bad")
        if column >= 0:
            diagnostics.append(
                {
                    "range": {
                        "start": {"line": line_number, "character": column},
                        "end": {"line": line_number, "character": column + 3},
                    },
                    "severity": 1,
                    "source": "mock-lsp",
                    "message": "mock: found bad token",
                }
            )
    send(
        {
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": {"uri": uri, "diagnostics": diagnostics},
        }
    )


def main():
    while True:
        message = read_message()
        if message is None:
            return 0
        method = message.get("method")
        identifier = message.get("id")
        if identifier is not None and method:
            if method == "initialize":
                result = {"capabilities": {"textDocumentSync": 1}}
            elif method == "shutdown":
                result = None
            else:
                result = None
            send({"jsonrpc": "2.0", "id": identifier, "result": result})
            continue
        if method == "exit":
            return 0
        if method in ("textDocument/didOpen", "textDocument/didChange"):
            params = message.get("params", {})
            document = params.get("textDocument", {})
            uri = document.get("uri")
            if method == "textDocument/didOpen":
                text = document.get("text", "")
            else:
                changes = params.get("contentChanges", [])
                text = changes[-1].get("text", "") if changes else ""
            if uri:
                diagnostics_for(uri, text)


if __name__ == "__main__":
    sys.exit(main())
