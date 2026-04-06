#!/bin/sh
set -eu

# Only enforce when SDL-MCP server is running (PID file exists)
if [ ! -f 'F:/Claude/sdl-mcp/sdl-mcp.pid' ]; then
  exit 0
fi

payload="$(cat)"
tool_name="$(printf '%s' "$payload" | python -c "import json,sys; data=json.load(sys.stdin); print(data.get('tool_name',''))")"

if [ "$tool_name" != "Task" ]; then
  exit 0
fi

task_type="$(printf '%s' "$payload" | python -c "import json,sys; data=json.load(sys.stdin); tool_input=data.get('tool_input') or {}; print(tool_input.get('subagent_type') or tool_input.get('description') or '')")"

case "$task_type" in
  *[Ee]xplore*)
    python -c "import json; print(json.dumps({'hookSpecificOutput': {'hookEventName': 'PreToolUse', 'permissionDecision': 'deny', 'permissionDecisionReason': 'Use the explore-sdl subagent instead of the built-in Explore agent when SDL-MCP is active. The explore-sdl agent uses SDL-MCP tools for efficient source code understanding.'}}))"
    exit 0
    ;;
esac
