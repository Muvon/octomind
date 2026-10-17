#!/usr/bin/env python3
"""Minimal MCP stdio server used by tests/mcp_stub_server_test.rs.

Speaks newline-delimited JSON-RPC per the MCP stdio transport. Implements
just enough of the protocol for octomind's rmcp client to complete the
initialize handshake, list tools, and round-trip a tool call:

  initialize            -> echoes the client's protocolVersion, declares tools
  notifications/initialized -> ignored (notification)
  tools/list            -> a single `echo` tool
  tools/call            -> returns the `msg` argument as text content
                           (`spawn_child` starts a long-lived child process
                           and returns "<server pid> <child pid>")
  ping                  -> empty result
"""

import json
import os
import subprocess
import sys
import time

# Children started by `spawn_child`, kept referenced for the server's lifetime.
CHILDREN = []


def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        method = msg.get("method")
        msg_id = msg.get("id")

        if method == "initialize":
            send({
                "jsonrpc": "2.0",
                "id": msg_id,
                "result": {
                    "protocolVersion": msg["params"]["protocolVersion"],
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "stub", "version": "1.0.0"},
                    "instructions": "stub server instructions",
                },
            })
        elif method == "tools/list":
            send({
                "jsonrpc": "2.0",
                "id": msg_id,
                "result": {
                    "tools": [{
                        "name": "echo",
                        "description": "Echo the msg argument back",
                        "inputSchema": {
                            "type": "object",
                            "properties": {"msg": {"type": "string"}},
                            "required": ["msg"],
                        },
                    }]
                },
            })
        elif method == "tools/call":
            params = msg.get("params") or {}
            if params.get("name") == "sleep":
                # Longer than any test server's timeout_seconds — used to
                # exercise the client-side call deadline.
                time.sleep(20)
                send({
                    "jsonrpc": "2.0",
                    "id": msg_id,
                    "result": {"content": [{"type": "text", "text": "overslept"}],
                               "isError": False},
                })
                continue
            if params.get("name") == "spawn_child":
                # A grandchild of octomind, like `node` under `npx`. Its stdio
                # is detached so only process-tree cleanup can end it.
                child = subprocess.Popen(
                    [sys.executable, "-c", "import time; time.sleep(300)"],
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                )
                CHILDREN.append(child)
                send({
                    "jsonrpc": "2.0",
                    "id": msg_id,
                    "result": {"content": [{"type": "text",
                                            "text": f"{os.getpid()} {child.pid}"}],
                               "isError": False},
                })
                continue
            if params.get("name") == "explode":
                send({
                    "jsonrpc": "2.0",
                    "id": msg_id,
                    "error": {"code": -32000, "message": "tool exploded on purpose"},
                })
                continue
            args = params.get("arguments") or {}
            send({
                "jsonrpc": "2.0",
                "id": msg_id,
                "result": {
                    "content": [{"type": "text", "text": str(args.get("msg", ""))}],
                    "isError": False,
                },
            })
        elif method == "ping":
            send({"jsonrpc": "2.0", "id": msg_id, "result": {}})
        elif msg_id is not None:
            send({
                "jsonrpc": "2.0",
                "id": msg_id,
                "error": {"code": -32601, "message": f"method not found: {method}"},
            })
        # Notifications (no id) for unknown methods are silently ignored.


if __name__ == "__main__":
    main()
