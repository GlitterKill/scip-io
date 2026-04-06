#!/bin/sh
set -eu

# Only enforce when SDL-MCP server is running (PID file exists)
if [ ! -f 'F:/Claude/sdl-mcp/sdl-mcp.pid' ]; then
  exit 0
fi

payload="$(cat)"
tool_name="$(printf '%s' "$payload" | python -c "import json,sys; data=json.load(sys.stdin); print(data.get('tool_name',''))")"
command="$(printf '%s' "$payload" | python -c "import json,sys; data=json.load(sys.stdin); tool_input=data.get('tool_input') or {}; print(tool_input.get('command') or tool_input.get('cmd') or '')")"

if [ "$tool_name" != "Bash" ]; then
  exit 0
fi

trimmed="$(printf '%s' "$command" | tr '[:upper:]' '[:lower:]' | sed 's/^[[:space:]]*//')"

for prefix in 'npm test' 'npm run test' 'npm run lint' 'npm run build' 'pnpm test' 'pnpm lint' 'pnpm build' 'yarn test' 'yarn lint' 'yarn build' 'bun test' 'bun run test' 'bun run lint' 'bun run build' 'pytest' 'python -m pytest' 'python -m unittest' 'bundle exec rspec' 'bundle exec rake' 'rake test' 'phpunit' 'vendor/bin/phpunit' 'composer test' 'go test' 'cargo test'; do
  case "$trimmed" in
    "$prefix"|"$prefix "*) 
      python -c "import json; print(json.dumps({'hookSpecificOutput': {'hookEventName': 'PreToolUse', 'permissionDecision': 'deny', 'permissionDecisionReason': 'Run repo-local test/build/lint commands through SDL runtime instead of native Bash. Use sdl.workflow with runtimeExecute so command execution stays in SDL-MCP and avoids redundant token spend.'}}))"
      exit 0
      ;;
  esac
done
