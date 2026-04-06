#!/bin/sh
set -eu

# Only enforce when SDL-MCP server is running (PID file exists)
if [ ! -f 'F:/Claude/sdl-mcp/sdl-mcp.pid' ]; then
  exit 0
fi

payload="$(cat)"
tool_name="$(printf '%s' "$payload" | python -c "import json,sys; data=json.load(sys.stdin); print(data.get('tool_name',''))")"
file_path="$(printf '%s' "$payload" | python -c "import json,sys; data=json.load(sys.stdin); tool_input=data.get('tool_input') or {}; print(tool_input.get('file_path') or tool_input.get('path') or '')")"

if [ "$tool_name" != "Read" ]; then
  exit 0
fi

if [ -z "$file_path" ]; then
  exit 0
fi

ext="$(printf '%s' "$file_path" | tr '[:upper:]' '[:lower:]')"
ext=".${ext##*.}"

for blocked_ext in '.ts' '.tsx' '.js' '.jsx' '.mjs' '.cjs' '.py' '.pyw' '.go' '.java' '.cs' '.c' '.h' '.cpp' '.hpp' '.cc' '.cxx' '.hxx' '.php' '.phtml' '.rs' '.kt' '.kts' '.sh' '.bash' '.zsh'; do
  if [ "$ext" = "$blocked_ext" ]; then
    python -c "import json; print(json.dumps({'hookSpecificOutput': {'hookEventName': 'PreToolUse', 'permissionDecision': 'deny', 'permissionDecisionReason': 'Use SDL-MCP tools for indexed source code. Do not use native Read, shell commands, or sdl.workflow/runtimeExecute to print or read indexed source files directly.\n\nFor indexed source:\n1. Start with sdl.repo.status.\n2. Use sdl.context (or sdl.agent.context outside Code Mode) for explain/debug/review/implement work.\n3. If more detail is needed, follow the SDL ladder: symbol.search/getCard -> slice.build -> code.getSkeleton -> code.getHotPath -> code.needWindow.\n4. Use symbolRef when the symbol name is known but the ID is not.\n5. Follow nextBestAction, fallbackTools, and fallbackRationale from SDL responses.\n\nOnly use file.read for non-indexed files such as docs, config, JSON, YAML, TOML, SQL, lockfiles, and templates.\nDo not use runtimeExecute as a workaround to read indexed source.'}}))"
    exit 0
  fi
done
