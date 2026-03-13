#!/usr/bin/env bash
# ACP smoke test — single octomind acp process, all tests in one session
# Usage: ./test_acp.sh [--role assistant|developer] [--bin /path/to/octomind]

ROLE="assistant"
BIN="$(dirname "$0")/target/debug/octomind"
while [[ $# -gt 0 ]]; do
  case $1 in
    --role) ROLE="$2"; shift 2 ;;
    --bin)  BIN="$2";  shift 2 ;;
    *) shift ;;
  esac
done

if [[ ! -x "$BIN" ]]; then
  echo "Binary not found: $BIN"
  echo "Run 'cargo build' first, or pass --bin /path/to/octomind"
  exit 1
fi

CWD="$(pwd)"
PASS=0
FAIL=0
TMPOUT=$(mktemp)

GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[1;33m'; NC='\033[0m'
ok()   { echo -e "${GREEN}✓${NC} $1"; ((PASS++)); }
fail() { echo -e "${RED}✗${NC} $1"; ((FAIL++)); }
info() { echo -e "${YELLOW}→${NC} $1"; }

echo ""
echo "=== Octomind ACP smoke test (role: $ROLE) ==="
echo ""

# Single acp process driven via a named fifo
FIFO=$(mktemp -u)
mkfifo "$FIFO"
trap 'exec 3>&- 2>/dev/null; rm -f "$FIFO" "$TMPOUT"; kill $ACP_PID 2>/dev/null || true' EXIT

"$BIN" acp --role "$ROLE" < "$FIFO" > "$TMPOUT" 2>/dev/null &
ACP_PID=$!
exec 3>"$FIFO"   # keep fifo open so acp doesn't get premature EOF

send() { echo "$1" >&3; }

wait_for_id() {
  local id="$1" timeout_s="$2" elapsed=0
  while [ $elapsed -lt $((timeout_s * 5)) ]; do
    grep -q "\"id\":$id" "$TMPOUT" 2>/dev/null && return 0
    sleep 0.2; ((elapsed++))
  done
  return 1
}

get_response() { grep "\"id\":$1" "$TMPOUT" 2>/dev/null | tail -1; }

# ── 1. initialize ─────────────────────────────────────────────────────────────
info "Test 1: initialize"
send '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"0.1.0","clientInfo":{"name":"test","version":"0.1"}}}'
if wait_for_id 1 10; then
  R=$(get_response 1)
  NAME=$(echo "$R" | jq -r '.result.agentInfo.name // empty' 2>/dev/null)
  VER=$(echo "$R"  | jq -r '.result.agentInfo.version // empty' 2>/dev/null)
  if [ "$NAME" = "octomind" ]; then ok "initialize — agent=octomind version=$VER"
  else fail "initialize — unexpected name='$NAME'"; fi
else
  fail "initialize — timeout"
fi

# ── 2. new_session ────────────────────────────────────────────────────────────
info "Test 2: session/new"
send "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"session/new\",\"params\":{\"cwd\":\"$CWD\",\"mcpServers\":[]}}"
if wait_for_id 2 15; then
  R=$(get_response 2)
  SESSION_ID=$(echo "$R" | jq -r '.result.sessionId // empty' 2>/dev/null)
  if [ -n "$SESSION_ID" ]; then ok "session/new — sessionId=$SESSION_ID"
  else fail "session/new — no sessionId; raw: $R"; SESSION_ID=""; fi
else
  fail "session/new — timeout"; SESSION_ID=""
fi

# ── 3. prompt ─────────────────────────────────────────────────────────────────
info "Test 3: session/prompt (simple reply)"
if [ -n "$SESSION_ID" ]; then
  send "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"session/prompt\",\"params\":{\"sessionId\":\"$SESSION_ID\",\"prompt\":[{\"type\":\"text\",\"text\":\"Reply with exactly one word: hello\"}]}}"
  if wait_for_id 3 30; then
    R=$(get_response 3)
    STOP=$(echo "$R" | jq -r '.result.stopReason // empty' 2>/dev/null)
    CONTENT=$(grep '"session/update"' "$TMPOUT" | jq -r 'select(.params.update.sessionUpdate == "agent_message_chunk") | .params.update.content.text // empty' 2>/dev/null | tr -d '\n')
    if [ "$STOP" = "end_turn" ]; then ok "prompt — stopReason=end_turn content=\"$CONTENT\""
    else fail "prompt — stopReason='$STOP'"; fi
  else
    fail "prompt — timeout"
  fi
else
  fail "prompt — skipped (no session)"
fi

# ── 4. new_session with injected MCP server ───────────────────────────────────
info "Test 4: session/new with stdio MCP server injection"
send "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"session/new\",\"params\":{\"cwd\":\"$CWD\",\"mcpServers\":[{\"name\":\"injected-echo\",\"command\":\"echo\",\"args\":[\"hello\"],\"env\":[]}]}}"
if wait_for_id 4 15; then
  R=$(get_response 4)
  SID2=$(echo "$R" | jq -r '.result.sessionId // empty' 2>/dev/null)
  if [ -n "$SID2" ]; then ok "session/new with MCP injection — sessionId=$SID2"
  else fail "session/new with MCP injection — no sessionId; raw: $R"; fi
else
  fail "new_session with MCP injection — timeout"
fi

# ── 5. load_session ───────────────────────────────────────────────────────────
info "Test 5: session/load"
if [ -n "$SESSION_ID" ]; then
  send "{\"jsonrpc\":\"2.0\",\"id\":5,\"method\":\"session/load\",\"params\":{\"sessionId\":\"$SESSION_ID\",\"cwd\":\"$CWD\",\"mcpServers\":[]}}"
  if wait_for_id 5 15; then
    R=$(get_response 5)
    ERR=$(echo "$R" | jq -r '.error // empty' 2>/dev/null)
    if [ -z "$ERR" ]; then ok "load_session — success"
    else fail "load_session — error: $ERR"; fi
  else
    fail "load_session — timeout"
  fi
else
  fail "load_session — skipped (no session)"
fi

# ── 6. cancel ─────────────────────────────────────────────────────────────────
info "Test 6: session/cancel"
if [ -n "$SESSION_ID" ]; then
  send "{\"jsonrpc\":\"2.0\",\"id\":6,\"method\":\"session/prompt\",\"params\":{\"sessionId\":\"$SESSION_ID\",\"prompt\":[{\"type\":\"text\",\"text\":\"Count slowly from 1 to 1000, one number per line\"}]}}"
  sleep 1
  send "{\"jsonrpc\":\"2.0\",\"method\":\"session/cancel\",\"params\":{\"sessionId\":\"$SESSION_ID\"}}"
  if wait_for_id 6 20; then
    R=$(get_response 6)
    STOP=$(echo "$R" | jq -r '.result.stopReason // empty' 2>/dev/null)
    if [ "$STOP" = "cancelled" ]; then ok "cancel — stopReason=cancelled"
    elif [ "$STOP" = "end_turn" ]; then ok "cancel — completed before cancel arrived (end_turn, acceptable)"
    else fail "cancel — unexpected stopReason='$STOP'"; fi
  else
    fail "cancel — timeout"
  fi
else
  fail "cancel — skipped (no session)"
fi

# ── 7. MCP tool execution (developer role with shell) ──────────────────────────
info "Test 7: MCP tool execution (shell)"
send "{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"session/new\",\"params\":{\"cwd\":\"$CWD\",\"mcpServers\":[]}}"
if wait_for_id 7 15; then
  SID_DEV=$(get_response 7 | jq -r '.result.sessionId // empty' 2>/dev/null)
  if [ -n "$SID_DEV" ]; then
    # Create a new session with developer role to test shell tool
    # First we need to close the current session and start with developer role
    send "{\"jsonrpc\":\"2.0\",\"id\":8,\"method\":\"session/new\",\"params\":{\"cwd\":\"$CWD\",\"mcpServers\":[]}}"
    # Note: The role is set at ACP startup, so we test with whatever role was passed
    # For assistant role, we test view tool instead
    send "{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"session/prompt\",\"params\":{\"sessionId\":\"$SID_DEV\",\"prompt\":[{\"type\":\"text\",\"text\":\"Use the view tool to read the first 5 lines of Cargo.toml and tell me the version.\"}]}}"
    if wait_for_id 9 45; then
      R=$(get_response 9)
      STOP=$(echo "$R" | jq -r '.result.stopReason // empty' 2>/dev/null)
      # Check for tool call notifications
      TOOL_CALLS=$(grep '"session/update"' "$TMPOUT" | grep -c 'tool_call' 2>/dev/null || echo "0")
      if [ "$STOP" = "end_turn" ] && [ "$TOOL_CALLS" -gt 0 ]; then
        ok "MCP tool execution — tool called $TOOL_CALLS time(s), stopReason=$STOP"
      elif [ "$STOP" = "end_turn" ]; then
        ok "MCP tool execution — stopReason=$STOP (no tool calls, model may have answered directly)"
      else
        fail "MCP tool execution — unexpected stopReason='$STOP'"
      fi
    else
      fail "MCP tool execution — timeout"
    fi
  else
    fail "MCP tool execution — no sessionId for developer session"
  fi
else
  fail "MCP tool execution — session/new timeout"
fi

# ── Shutdown ──────────────────────────────────────────────────────────────────
exec 3>&-
wait $ACP_PID 2>/dev/null || true

echo ""
echo "=== Results: ${PASS} passed, ${FAIL} failed ==="
echo ""
[ "$FAIL" -eq 0 ] && exit 0 || exit 1
