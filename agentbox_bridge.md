# 🛡️ MANDATORY PROTOCOL: Local & Remote Execution via AgentBox CLI

> [!IMPORTANT]
> **NEED-TO-KNOW & ZERO-ESCAPING PROTOCOL:**
> - DO NOT read or edit `.agentbox.json` directly (Need-to-Know: use `-t <target>` and `agentbox task`).
> - NEVER read full files, run raw grep/find, cat, or dump noisy git diffs/builds. Use `agentbox` tools.
> - **Zero-Escaping on Windows:** NEVER pass code or scripts inside raw CLI quotes (`"..."`). Prefer `agentbox task <name> [args]`, `--file` (write via native `write_file` -> pass path), or `--base64`.
> - **Working dir & Session Daemon:** Run `agentbox -C "C:\\Users\\user\\Hobby\\utils\\ScreenCompare"` (or `agentbox` from the folder containing this `agentbox.md`) once at session start — it auto-detects non-interactive/AI mode, starts a background session daemon bound to your caller path (keeping SSH connections open across calls for 30 min), and remembers the project directory for all subsequent `agentbox <cmd>` calls.
> - **Multi-host:** Use `agentbox -t <target> <command> [args]` (e.g. `agentbox -t remote tree --depth 1`). Without `-t`, runs on default target.
> - **Read-Only Targets:** Targets marked `(read-only)` STRICTLY forbid any modifications (`write`, `patch`, `sync push`, `pip`) and code executions (`run`, `py`, `spawn`, `test`, `task`). Use ONLY Inspect & Navigate tools (`view`, `outline`, `get-fn`, `find-fn`, `refs`, `tree`, `grep`, `diff`, `git-*`, `sync pull`, `graphify-query`).

<!-- AGENTBOX_TARGETS_START -->
##### 🌐 Available Execution Targets:
* `local` (active)
<!-- AGENTBOX_TARGETS_END -->

### 🧰 AgentBox Toolset:

#### 1. Inspect & Navigate
* `agentbox view <file> --start N --end M` (or `--tail N`) — line slice with 1-indexed numbers.
* `agentbox outline <file>` — AST outline of classes/functions (~120 tokens vs 3000).
* `agentbox get-fn <file> <name>` — extracts exact function or `Class.method` definition.
* `agentbox refs <sym>` / `agentbox find-fn <sym>` — locate symbol definitions and usages across project.
* `agentbox tree [path] [--depth N]` — compact directory tree.
* `agentbox grep "<query>" [path] [-F] [-l]` — text search (`-F`: exact text, `-l`: file names only).
* `agentbox graphify-query "<query>" [path] [--max-tokens N]` — architecture map & symbol relations (auto-builds if missing).
* `agentbox graphify-build [path] [--deep]` — force (re)build knowledge graph of target project.
* `agentbox graphify-install [--upgrade]` — silently provision/upgrade graphify engine.
* `agentbox graphify-open [path]` — open interactive graph in local browser (Remote-Compute, Local-Artifact).

#### 2. Edit, Write & Sync (Safe Payloads: `--file` or `--base64` ONLY)
* `agentbox patch-lines <file> --start N --end M --file <path> | --base64 <b64>` — replaces line slice `[N..M]` with auto-backup & AST check.
* `agentbox write <file> --file <path> | --base64 <b64> [--append]` / `agentbox write-files` — writes/appends single or multiple files safely.
* `agentbox cp <src> <dest>` — copies file directly between targets or local workspace (`[target:]path`).
* `agentbox patch <file> --old-file <old> --new-file <new>` — atomic search-and-replace.
* `agentbox rollback <file>` — restores previous version from backup.
* `agentbox sync status | push | pull [--dry-run] [--force]` — two-way project sync.

#### 3. Test & Execute (Silent Runners)
* `agentbox test "<cmd>" | --file <path> | --base64 <b64>` — silences passing tests, extracts failure tracebacks (prefer `agentbox task <name>` to avoid Windows escaping issues).
* `agentbox run "<cmd>" | --file <path> | --base64 <b64>` — silent build/script runner.
* `agentbox spawn "<cmd>" | --file <path>` / `agentbox logs <id> -f` / `agentbox kill <id>` — background tasks.
* `agentbox pip <args>` — runs pip in isolated project venv.
* `agentbox diff [file]` / `agentbox git-status` / `agentbox git-log -n N` / `agentbox git-view <file> [rev]` / `agentbox git-fn-history <file> <fn>` — token-efficient git info.

#### 4. Verified Project Recipes & Session Daemon
* `agentbox task` — lists available project recipes (`agentbox task <name> [args]`).
* `agentbox batch --file <steps.json>` — runs multiple commands sequentially in a single call.
* `agentbox` / `agentbox --bridge` / `agentbox --no-bridge` — auto-detects AI (starts path-bound background session daemon) vs interactive TTY human (starts Clipboard UI Bridge) and prints mode + reason.
* `agentbox daemon status | stop [host]` — manage active path-bound session daemons & SSH bridges.

<!-- AGENTBOX_TASKS_START -->
##### 📋 Verified Project Recipes (Auto-Memoized):
* _(No custom tasks defined yet — add with `agentbox task add <name> <cmd>`)_
<!-- AGENTBOX_TASKS_END -->

### ⚡ Pre-Flight Checklist before execution:
1. Working Dir: Are you in the folder with `agentbox.md` (or did you initialize session with `agentbox -C "C:\\Users\\user\\Hobby\\utils\\ScreenCompare"`)?
2. Zero-Escaping on Windows: Did you use `agentbox py --base64`, `task`, or `--file` instead of raw shell quotes?
3. Context Guard: Did you use `outline` / `get-fn` / `py` instead of dumping full files into context?
4. Remote Target: Did you pass `-t <target>` (e.g. `agentbox -t remote <cmd>`) when targeting a remote server?
5. Read-Only Guard: Is the target marked `read-only`? If yes, use ONLY Inspect & Navigate tools (no edits/runs/push).

### 📋 Clipboard Bridge Mode (Web Chat Integration):
* Your responses are monitored by a local Windows Clipboard Bridge.
* To inspect files or run checks, output exact `agentbox ...` lines inside a single fenced code block (multiple commands on separate lines are executed sequentially in one pass).
* To apply complex multi-file Python edits in one step, output a single fenced `python` block starting with `# FIX:` on the first line (avoid nested triple quotes inside the block).
* **Max `# FIX:` payload size: <= 7500 characters** (web chat code blocks truncate around ~9000 chars). Split larger edits into sequential `# FIX:` steps.
* Keep explanations concise so the user or bridge can act immediately.
