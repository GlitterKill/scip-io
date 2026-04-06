Use SDL-MCP as the default path for repository scip-io.

## Required Workflow

1. Start with sdl.repo.status.
2. Use sdl.action.search when the correct SDL action is unclear.
3. Use sdl.manual(query|actions|format) for focused reference instead of loading the full manual.
4. Use sdl.context for Code Mode context retrieval, or sdl.agent.context on the agent action surface (contextMode: "precise" for targeted lookups, "broad" for exploration).
5. Use sdl.workflow for multi-step operations (runtime execution, data transforms, batch mutations) - not for context retrieval.
6. Use symbolRef / symbolRefs when you know a symbol name but not the canonical symbolId.
7. Follow nextBestAction, fallbackTools, fallbackRationale, and candidate guidance from SDL responses instead of retrying blocked native tools.

## Native Tool Restrictions

- Never use native Read for indexed source-code extensions: .ts, .tsx, .js, .jsx, .mjs, .cjs, .py, .pyw, .go, .java, .cs, .c, .h, .cpp, .hpp, .cc, .cxx, .hxx, .php, .phtml, .rs, .kt, .kts, .sh, .bash, .zsh.
- Never use native Bash for repo-local test, build, lint, or diagnostic commands when SDL runtime can execute them.
- If native Read or Bash is denied by a hook, switch to SDL-MCP immediately and do not retry the denied tool.
- Use the explore-sdl subagent for codebase exploration instead of the built-in Explore agent.
- Native Read is allowed for non-indexed file types (Markdown, JSON, YAML, TOML, config) even when SDL-MCP is active.

## Conditional Enforcement

All SDL-MCP enforcement is conditional on the server being active (PID file exists). When SDL-MCP is not running, all native tools work normally with no restrictions.

## Context Retrieval

Use sdl.context - not sdl.workflow - for Code Mode understanding tasks. On the agent action surface, use sdl.agent.context:
- contextMode: "precise" — targeted symbol/file lookups
- contextMode: "broad" — exploratory codebase understanding
Provide focusSymbols and/or focusPaths to scope the retrieval. Always set a budget (maxTokens, maxActions).

## Runtime Execution

- Use runtimeExecute inside sdl.workflow with outputMode: "minimal" (default) for ~50-token responses.
- Parameters: use args (string array) or code (inline string). There is no command field.
- Use runtimeQueryOutput with artifactHandle and queryTerms to retrieve output details after minimal-mode execution.
- Set timeoutMs on all runtime executions to prevent hangs.

## Non-Indexed File Access

- Use file.read inside sdl.workflow for reading non-indexed files with targeted modes (search, jsonPath, offset/limit).
- Prefer search or jsonPath over full reads.
