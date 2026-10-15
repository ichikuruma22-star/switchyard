# Local workspace API

This document describes the current app/CLI source. The gateway remains independently installable. The app uses its own agent engine; existing-agent adapters are a separate integration boundary.

The app host binds only to `127.0.0.1` on an allocated port. Every `/api/` request requires `Authorization: Bearer <random host-session token>`. The launch URL supplies this token in a fragment, which the UI removes before navigation and retains in session storage for reloads. Tokens expire when the host exits. Host and browser Origin checks restrict requests to the exact loopback origin. No remote origin receives CORS access. Tokens and provider keys never appear in logs, event data, or URL query parameters.

All mutations accept JSON. Errors have shape `{ "error": { "code": "conflict", "message": "..." } }` with an appropriate HTTP status. File content, tool output and model output must be rendered as text, never as trusted HTML. The service exposes only explicitly opened projects, never an arbitrary file-serving route.

## Endpoints

| Method and path | Input | Response |
|---|---|---|
| `GET /api/status` | — | `{status: EngineStatus, version, gateway_config_path}` |
| `GET /api/models` | — | `{models: [...model summaries...]}` |
| `GET /api/projects` | — | `{projects: Project[]}` |
| `POST /api/projects` | `{path}` | `{project: Project}` |
| `GET /api/sessions?project_id=...` | Optional exact project id | `{sessions: Session[]}` |
| `POST /api/sessions` | `{project_id, model}` | `{session: Session}` |
| `GET /api/sessions/:id` | — | `SessionView` |
| `POST /api/sessions/:id/turns` | `{command_id, text}`; command id is a client-generated UUID retained across retries | `{run: Run}` |
| `GET /api/sessions/:id/events?after_seq=0&limit=200` | Strict bounded cursor and count | `{events: Event[]}` |
| `POST /api/sessions/:id/operations/:operation_id/decision` | `{expected_hash, decision: "allow_once" or "deny"}` | `{ok: true}` |
| `POST /api/sessions/:id/interrupt` | `{run_id}` | `{ok: true}` |
| `POST /api/sessions/:id/recovery` | `{expected_revision, note}`; a human review note of 1–2000 UTF-8 bytes | `{session: Session}` |

Serialize Rust data using snake_case. `SessionView` contains `session`, `project`, `pending_operation`, and a bounded `events` window. The engine owns the detailed public types in `crates/agent`; their definitions are authoritative. The frontend should use the event `kind` plus its structured `payload`, and tolerate unfamiliar event kinds as neutral activity entries.

The UI needs project names/paths, session titles/model/state, and each event's schema_version, session_id, run_id, seq, at_ms, kind, payload. Models must expose their real IDs; no invented available-provider list. An empty model list or only mock models must clearly lead to provider setup rather than imply a working coding model.

## Execution and approvals

Only `submit_turn` starts a model run. Reading state or reconnecting cannot replay a model call or side effect. One active run is allowed per session. Read/search tools operate inside the opened project; patches and commands require a human decision for the exact persisted operation and argument hash. Approval grants only that operation once. Approved writes and commands are serialized within the native engine across all of this host's projects, including overlapping project folders. Model requests, reads and approval waits remain concurrent. This does not lock out external editors, external agent processes or other programs.

Commands execute on the user's machine with its current OS permissions. A working directory restriction is not an OS sandbox. The UI must show the exact command, working directory, and this permission boundary before approval. File edits show the proposed diff and detect stale file contents before applying.

Interrupted operations with uncertain outcomes are marked `recovery_required`. They are not silently replayed. A model's completion is labeled completed, not verified: checks and exit statuses remain separate evidence.

After inspecting the affected files or command results, a user can acknowledge recovery for the exact session revision. This records the review note and moves the session to `interrupted`, allowing a new turn with a new command ID. The uncertain operation stays `outcome_unknown`; acknowledgement never certifies success or repeats the operation. Stale and duplicate acknowledgements fail with a conflict.

## Existing-agent runs

These routes drive the Codex, Claude Code and Antigravity adapters described in [the adapter contract](AGENT-ADAPTERS.md). They use the same authentication, Origin and JSON body rules. `adapter_id` is `codex`, `claude` or `antigravity`. Claude requires a selected profile for that agent: built-in `system-claude` (Existing Claude CLI) or a managed profile with a confirmed sign-in. Antigravity also requires a selected profile: built-in `system-antigravity` (Existing Antigravity CLI) or any managed Antigravity profile. Antigravity profiles are never gated on sign-in, because the CLI has no account-status command. The built-in `system-codex`, `system-claude` and `system-antigravity` accounts are selectable and inspectable, but reject login actions.

| Method and path | Input | Response |
|---|---|---|
| `GET /api/adapters` | — | `{adapters: AdapterStatus[]}` |
| `GET /api/adapter-runs` | — | `{runs: AdapterRun[]}`, newest first |
| `POST /api/adapter-runs` | `{adapter_id, project_id, prompt, command_id, profile_id?, continue_run_id?}` | `202 {run: AdapterRun}` |
| `GET /api/adapter-runs/:id` | — | `{run: AdapterRun}` |
| `GET /api/adapter-runs/:id/events?after_seq=0&limit=200` | Strict bounded cursor and count | `{events: AdapterEvent[]}` |
| `POST /api/adapter-runs/:id/decisions` | `{approval_id, expected_hash, decision: "allow_once" or "deny"}` | `{ok: true}` |
| `POST /api/adapter-runs/:id/interrupt` | `{}` | `{ok: true}` |
| `POST /api/adapter-runs/:id/recovery` | `{}` | `{run: AdapterRun}` |

Runs are saved by the host and survive restarts. `continue_run_id` continues the saved agent conversation of that run with only the new prompt; it must name the conversation's latest run that attached to its CLI session, and the run keeps that conversation's agent and account profile. A different `profile_id` is a conflict, and `profile_changed` means the original profile is no longer available unchanged. A run that was active when a host stopped is `recovery_required` if its input might have been sent; it cannot be continued until `/recovery` records the human review. Acknowledgement never replays or certifies the uncertain work. Each `AdapterRun` reports `conversation_id`, `continued_from`, `title`, `ephemeral` and, when saved history cannot be read or written, `history_error`.

## Workspace inspection and handoffs

These routes share the same host authentication, Origin and strict JSON/query
rules. None starts a model or approves an agent operation.

| Method and path | Input | Response |
|---|---|---|
| `GET /api/workspace` | — | `{selection}` from durable local navigation state |
| `POST /api/workspace` | `{project_id, session_id, conversation_id, mode, tool, theme}` | `{selection}` after project membership validation |
| `GET /api/projects/:id/files?path=.` | Relative folder | Bounded entries, omissions and truncation |
| `GET /api/projects/:id/file?path=...` | Relative file | Content, hash and truncation |
| `GET /api/projects/:id/search?query=...&path=.&limit=30` | Literal query, optional folder/count | Bounded `matches` and scan statistics: `files_searched`, `bytes_read`, `omitted` (entries skipped because they could not be validated or read) and `truncated` |
| `GET /api/projects/:id/changes` | — | Current Git branch and file states |
| `GET /api/projects/:id/diff?path=...&scope=unstaged` | Exact relative tracked path; staged or unstaged | Bounded diff |
| `GET /api/projects/:id/checkpoints?limit=20&before=...` | Optional prior page's cursor | `{checkpoints, next_before}` |
| `POST /api/projects/:id/checkpoints` | `{id, title, summary, source, verification?, next_steps?, risks?}` | `{checkpoint}`; exact UUID retries are idempotent, conflicts never overwrite |
| `GET /api/projects/:id/checkpoints/:note` | Note UUID | `{checkpoint}` only within this project |
| `GET /api/projects/:id/conversations?limit=20&before=...` | Optional prior page's cursor | `{conversations, next_before}` |
| `GET /api/projects/:id/conversations/:kind/:conversation?after_seq=0&limit=20` | Native session or adapter run; bounded sequence cursor | Project-validated metadata, events, next cursor, has_more and history_gap |
| `GET /api/projects/:id/mcp-config` | — | MCP JSON, Codex TOML and host-lifetime requirement; no bearer token |

The workspace `tool` values are `conversation`, `files`, `changes` and
`handoffs`. Checkpoints are immutable author-reported context. They are not
approval records or independent verification evidence. Conversation context
pages cap payloads and report truncation explicitly. See [MCP](MCP.md).

## Interactive terminals

These authenticated routes control a user-operated shell with the current
OS account's permissions. Opening a workspace view does not start a shell.
Terminals are separate from agent tools and their approval records.

| Method and path | Input | Response |
|---|---|---|
| `GET /api/terminals?project_id=...` | Optional exact project ID | `{terminals}` with metadata and output cursors |
| `POST /api/terminals` | `{id, project_id, cols, rows}`; stable client UUID | `{terminal}`; retrying the same ID in the project reuses its terminal |
| `GET /api/terminals/:id?after_seq=0` | Output sequence cursor | `{terminal}` including up to 32 base64 output chunks |
| `POST /api/terminals/:id/input` | `{sequence, data}`; UTF-8 input of 1–8,192 bytes | `{sequence}`; an exact retry never delivers the input twice |
| `POST /api/terminals/:id/resize` | `{cols, rows}` | Updated dimensions |
| `POST /api/terminals/:id/close` | — | `{terminal}` after requesting shutdown |

Each terminal retains up to 1 MiB of output. Compare the requested cursor
with `first_seq` to detect trimmed history, and keep reading through
`last_seq`; `output_done` identifies a finished output stream. There are at
most eight active terminals and 64 retained terminal entries per host.
Dimensions are 2–500 columns and 1–300 rows. Input is ordered; after an
uncertain or partial write, the shell is stopped and that input is never
replayed. Terminals do not survive a host restart. Deliberately detached
programs are outside the Unix shell-job cleanup guarantee.

## Product and platform boundaries

The shared Rust engine backs the local HTTP app and terminal CLI. A Tauri desktop shell belongs outside the gateway workspace so headless gateway builds do not require GUI libraries. Cloudflare is the selected platform for the public website and future authenticated coordination; the local execution API is not exposed through a public tunnel by default.

Product brand: Switchya, powered by the independently installable Switchyard Gateway. The selected domain is switchya.com. Authentication and update endpoints must be backed by actual configured services; domain ownership alone does not provide them.
