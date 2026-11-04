#!/usr/bin/env python3
"""Line-delimited JSON-RPC MCP server used by the integration tests."""

import json
import sys

TOOLS = [
    {
        "name": "echo",
        "description": "Echo the provided text.",
        "inputSchema": {
            "type": "object",
            "properties": {"text": {"type": "string"}},
            "required": ["text"],
            "additionalProperties": False,
        },
    }
]


def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        message = json.loads(line)
        method = message.get("method")
        identifier = message.get("id")
        if method == "initialize":
            send(
                {
                    "jsonrpc": "2.0",
                    "id": identifier,
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "mock", "version": "0"},
                    },
                }
            )
        elif method == "tools/list":
            send({"jsonrpc": "2.0", "id": identifier, "result": {"tools": TOOLS}})
        elif method == "tools/call":
            arguments = message.get("params", {}).get("arguments", {})
            send(
                {
                    "jsonrpc": "2.0",
                    "id": identifier,
                    "result": {
                        "content": [
                            {"type": "text", "text": "echo: " + str(arguments.get("text", ""))}
                        ]
                    },
                }
            )
        elif identifier is not None:
            send({"jsonrpc": "2.0", "id": identifier, "result": None})
    return 0


if __name__ == "__main__":
    sys.exit(main())
