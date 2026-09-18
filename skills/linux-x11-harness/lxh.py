#!/usr/bin/env python3
"""Call a linux-x11-harness tool through the daemon's MCP socket.

For agents without MCP support: the daemon exposes the same tools over a
newline-delimited JSON-RPC 2.0 Unix socket.

Usage:
    lxh.py <tool_name> [json_arguments]
    lxh.py lxh_display_create '{"persistent": true}'
    lxh.py lxh_capture_screenshot '{"display_id": "d-..."}'

Prints the tool's result as JSON. Exit code 1 on transport or tool error.
"""

import json
import os
import socket
import sys


def socket_path() -> str:
    """Mirror the daemon's socket resolution exactly."""
    if env := os.environ.get("LXH_SOCKET_PATH"):
        return env
    base = os.environ.get("XDG_RUNTIME_DIR") or "/tmp"
    return os.path.join(base, "linux-x11-harness.sock")


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__, file=sys.stderr)
        return 1
    tool = sys.argv[1]
    arguments = json.loads(sys.argv[2]) if len(sys.argv) > 2 else {}

    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(socket_path())
    stream = sock.makefile("rwb")

    def send(request):
        stream.write((json.dumps(request) + "\n").encode())
        stream.flush()

    def receive():
        return json.loads(stream.readline())

    send(
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-05",
                "capabilities": {},
                "clientInfo": {"name": "lxh-skill", "version": "0.1.0"},
            },
        }
    )
    receive()
    send({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})
    send(
        {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": tool, "arguments": arguments},
        }
    )
    response = receive()

    if "error" in response:
        print(json.dumps(response["error"], ensure_ascii=False), file=sys.stderr)
        return 1

    result = response["result"]
    content = result.get("content") or []
    text = content[0]["text"] if content else ""
    try:
        print(json.dumps(json.loads(text), ensure_ascii=False, indent=2))
    except (ValueError, TypeError):
        print(text)
    return 1 if result.get("isError") else 0


if __name__ == "__main__":
    sys.exit(main())