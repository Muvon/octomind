#!/usr/bin/env bash
# Schedule smoke test — verifies that scheduled messages fire automatically
# without any additional user prompts.
#
# Usage: ./test_schedule_smoke.sh [--tag assistant:general] [--bin /path/to/octomind]

TAG="assistant:general"
BIN="$(dirname "$0")/../../target/debug/octomind"
SCHEDULE_DELAY=5   # seconds for the schedule timer
WAIT_TIMEOUT=45    # max seconds to wait for auto-response

while [[ $# -gt 0 ]]; do
  case $1 in
    --tag) TAG="$2"; shift 2 ;;
    --bin) BIN="$2"; shift 2 ;;
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
ok()   { echo -e "${GREEN}PASS${NC} $1"; ((PASS++)); }
fail() { echo -e "${RED}FAIL${NC} $1"; ((FAIL++)); }
info() { echo -e "${YELLOW}>>>${NC} $1"; }

echo ""
echo "=== Schedule smoke test (tag: $TAG, delay: ${SCHEDULE_DELAY}s) ==="
echo ""

# Single acp process driven via a named fifo
FIFO=$(mktemp -u)
mkfifo "$FIFO"
trap 'exec 3>&- 2>/dev/null; rm -f "$FIFO" "$TMPOUT"; kill $ACP_PID 2>/dev/null || true' EXIT

"$BIN" acp "$TAG" < "$FIFO" > "$TMPOUT" 2>/dev/null &
ACP_PID=$!
exec 3>"$FIFO"

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

# ── 1. Initialize ────────────────────────────────────────────────────────────
info "Initializing ACP"
send '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientInfo":{"name":"schedule-test","version":"0.1"}}}'
if ! wait_for_id 1 10; then
  fail "initialize — timeout"
  exit 1
fi
ok "initialize"

# ── 2. Create session ────────────────────────────────────────────────────────
info "Creating session"
send "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"session/new\",\"params\":{\"cwd\":\"$CWD\",\"mcpServers\":[]}}"
if ! wait_for_id 2 15; then
  fail "session/new — timeout"
  exit 1
fi
SESSION_ID=$(get_response 2 | jq -r '.result.sessionId // empty' 2>/dev/null)
if [ -z "$SESSION_ID" ]; then
  fail "session/new — no sessionId"
  exit 1
fi
ok "session/new — sessionId=$SESSION_ID"

# ── 3. Send schedule prompt ──────────────────────────────────────────────────
info "Sending schedule prompt (delay: ${SCHEDULE_DELAY}s)"
BEFORE_PROMPT=$(wc -l < "$TMPOUT" | tr -d ' ')
send "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"session/prompt\",\"params\":{\"sessionId\":\"$SESSION_ID\",\"prompt\":[{\"type\":\"text\",\"text\":\"Use the schedule tool to add a schedule with when='in ${SCHEDULE_DELAY}s' and message='SCHEDULE_FIRED_OK'. Only call the tool, no explanation needed.\"}]}}"
if ! wait_for_id 3 60; then
  fail "schedule prompt — timeout"
  echo "--- output so far ---"
  tail -20 "$TMPOUT" | while IFS= read -r line; do echo "$line" | jq '.' 2>/dev/null || echo "$line"; done
  echo "--- end ---"
  exit 1
fi
STOP=$(get_response 3 | jq -r '.result.stopReason // empty' 2>/dev/null)
if [ "$STOP" != "end_turn" ]; then
  fail "schedule prompt — unexpected stopReason='$STOP'"
  exit 1
fi

# Check that a schedule tool_call was made
TOOL_CALLS=$(tail -n +"$BEFORE_PROMPT" "$TMPOUT" | grep '"session/update"' | grep -c 'schedule' 2>/dev/null || echo "0")
if [ "$TOOL_CALLS" -gt 0 ]; then
  ok "schedule created (tool called)"
else
  # Model might not have the tool or refused — check output
  CHUNK=$(tail -n +"$BEFORE_PROMPT" "$TMPOUT" | grep '"session/update"' | jq -r 'select(.params.update.sessionUpdate == "agent_message_chunk") | .params.update.content.text // empty' 2>/dev/null | tr -d '\n')
  fail "schedule tool not called — AI output: '$CHUNK'"
  exit 1
fi

# ── 4. Wait for auto-response (NO further prompts sent) ──────────────────────
info "Waiting for automatic response after schedule fires (up to ${WAIT_TIMEOUT}s)..."
AFTER_SCHEDULE=$(wc -l < "$TMPOUT" | tr -d ' ')

# The monitor should process the schedule message and the AI should respond.
# We look for new session/update notifications after the schedule prompt completed.
elapsed=0
AUTO_RESPONSE=""
while [ $elapsed -lt $((WAIT_TIMEOUT * 5)) ]; do
  # Look for agent_message_chunk notifications that appeared AFTER the schedule prompt
  AUTO_RESPONSE=$(tail -n +"$AFTER_SCHEDULE" "$TMPOUT" \
    | grep '"session/update"' \
    | jq -r 'select(.params.update.sessionUpdate == "agent_message_chunk") | .params.update.content.text // empty' 2>/dev/null \
    | tr -d '\n')
  if [ -n "$AUTO_RESPONSE" ]; then
    break
  fi
  sleep 0.2
  ((elapsed++))
done

if [ -n "$AUTO_RESPONSE" ]; then
  ok "auto-response received: \"$(echo "$AUTO_RESPONSE" | head -c 100)\""
else
  fail "no auto-response within ${WAIT_TIMEOUT}s — schedule monitor is broken"
  echo ""
  echo "--- last 20 lines of output ---"
  tail -20 "$TMPOUT" | while IFS= read -r line; do echo "$line" | jq '.' 2>/dev/null || echo "$line"; done
  echo "--- end ---"
fi

# ── Shutdown ──────────────────────────────────────────────────────────────────
exec 3>&-
wait $ACP_PID 2>/dev/null || true

echo ""
echo "=== Results: ${PASS} passed, ${FAIL} failed ==="
echo ""
[ "$FAIL" -eq 0 ] && exit 0 || exit 1
