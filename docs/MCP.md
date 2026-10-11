# Shared project context through MCP

The current source includes a stdio MCP server in the `switchya` executable.
It gives a compatible client current project context and durable handoffs.
These features are newer than the published `0.1.0-preview.2` archive.
MCP support belongs to the client/harness; the model name alone does not
establish that its official application can connect.

## Connect to the open app

1. Open the project in Switchya and choose **Handoffs → Connect agent**.
2. Choose **MCP JSON** for clients using `mcpServers`, or **Codex TOML** for
   Codex's `mcp_servers` configuration.
3. Review and add that entry to the client's MCP configuration. Switchya
   does not edit another app's settings or install a server globally.
4. Keep Switchya open, connect the client, and ask it to call
   `switchya_project_context` before continuing the work.

The entry uses the companion `switchya` binary when it is next to the running
host. Otherwise it uses `switchya` from PATH; install the CLI or replace that
command with its absolute path. The native Windows portable archive includes
both executables.

The selected folder is fixed in the server's `--project` argument. Tool
arguments cannot select another project. Configure a separate entry if you
want another folder. The configuration contains paths, not bearer tokens.
Its private host-record path stays the same across app restarts, but a client
must reconnect to start a fresh MCP process after the app restarts.

## Start a standalone context server

With the current CLI on PATH, configure your MCP client to start:

```text
switchya mcp --project /absolute/path/to/project
```

For a Windows project, pass its absolute Windows path as one argument. Normal
global `--config` and `--data-dir` options select an isolated workspace. The
server discovers a live host for that data directory; otherwise it starts
its own. The first process that starts a host owns its lifetime. Closing its
stdin stops that host; clients that joined it do not extend its lifetime.
Use an independently running app when several agents need a lasting host.

`--launch-info PATH` explicitly connects to an existing private launch record
and cannot be combined with local configuration overrides. Closing a client
that joined another host never shuts that host down.

Stdout contains only MCP messages. Diagnostics go to stderr. No model is
started by connecting, inspecting context or saving a handoff.

## Tools

Successful results contain `{project_id, data}` as both structured content
and a text representation. Tool errors set `isError` with an actionable
message. All tools except saving a checkpoint are read-only.

| Tool | Purpose and bounds |
|---|---|
| `switchya_project_context` | Project metadata, root entries, current Git state, five recent checkpoint summaries, and root AGENTS.md/README.md/CLAUDE.md previews of at most 12,000 characters each. Individual unavailable sections are reported. Read nested instructions before changing a subfolder. |
| `switchya_list_files` | One relative folder. Scans at most 2,000 entries; reports omissions and truncation. Follow subfolders or a known path when a broad listing is incomplete. |
| `switchya_read_file` | UTF-8 content and hash, optionally starting at a line and returning 1–1,000 lines. The underlying file preview is capped at 256 KiB; a truncated preview is not the complete file. |
| `switchya_search` | Literal, case-sensitive text and filename search. Query is 1–512 UTF-8 bytes, limit is 1–100 matches. Scan budgets and truncation are explicit; narrow the folder when incomplete. |
| `switchya_git_changes` | Current branch and staged, unstaged and untracked paths. Reports when the project is not in a repository. |
| `switchya_git_diff` | One tracked path's staged or unstaged diff, capped at 256 KiB. Read an untracked file through `switchya_read_file`. |
| `switchya_list_conversations` | Project's native sessions and external turns, 1–100 at a time. Pass `next_before` for an older page. External turns use `conversation_id` and `continued_from` to link their conversation. |
| `switchya_read_conversation` | Events after a sequence cursor. Up to 100 events and approximately 256 KiB per page; large payloads are marked and excerpted. `next_after_seq`, `has_more` and `history_gap` expose continuation and retention limits. |
| `switchya_list_checkpoints` | Newest checkpoint summaries, 1–100 per page. Use `next_before` for an older page. |
| `switchya_read_checkpoint` | One full checkpoint by UUID in the selected project. |
| `switchya_save_checkpoint` | Append an immutable handoff. Requires a canonical lowercase UUID, title, summary and source; accepts reported verification, up to 20 next steps and up to 20 risks. Total note is at most 64 KiB. |

## Reliable handoffs

Generate one UUID for a new checkpoint. If its result is uncertain, retry
the identical note with the same UUID. An exact retry returns the saved note
and timestamp; conflicting content is rejected. Use a new UUID for a new
report. Notes are stored in the local project database and survive host
and port changes. They do not modify repository files.

Separate observed results from assumptions. Include the change or revision
checked, commands actually run, their outcomes, and unfinished work. Do not
include passwords, API keys or unrelated personal data. A note's `source`
and `verification` fields are author-reported context. Neither can approve
an operation, establish success independently or authorize publication.

## Access and protocol limits

The implementation uses the official Rust MCP SDK. Protocol tests cover
the `2025-11-25` initialization lifecycle and `2026-07-28` discovery with
per-request metadata. These protocol fixtures do not prove integration with
every vendor application or model.

MCP messages are capped at 256 KiB before JSON parsing; four context tool
calls may run concurrently. HTTP requests remain authenticated, loopback-only,
bounded and subject to the app's existing Origin checks. Files reuse the
anchored read policy that rejects traversal, links, repository metadata and
conventional secret paths. This filename policy is not a guarantee that all
sensitive content has been identified.

The active-host record is under the data directory's private `.switchya`
folder, which file tools exclude. Unix launch files use mode 0600; Windows
uses a protected current-user/SYSTEM DACL. The current host's bearer token is
also redacted from successful MCP results if a copy appears in readable
project material. Processes with the same OS account can still read that
account's files: this is not isolation against a hostile local user.

There are no MCP tools for shell execution, file edits, model calls, approvals,
account management or publishing. Clients retain their own action controls.
Repository text, saved conversations and notes remain untrusted content.

## Repeatable evaluation scenarios

Use a disposable project with known files and a controlled host. These
questions are test scenarios, not claims about comparative model quality.

1. Which root instructions apply, and which paths have current changes?
2. What exact line window contains a selected function, and was its full
   file available or truncated?
3. Where does a literal symbol occur, and did the search exhaust its budget?
4. How does one tracked path differ in the index and working tree?
5. Which saved native session or external turn belongs to this project?
6. Can a guessed conversation ID from another project reveal any events?
7. Can one client save a handoff that a second client reads unchanged?
8. Does retrying the same note preserve its ID and original timestamp,
   while changed content under that ID is rejected?
9. After a host restart, do the same checkpoints remain available through
   a fresh client and private host record?
10. Do traversal, protected-file reads, unknown fields and oversized protocol
    messages fail without exposing content or running a model?

Run `cargo test -p switchyard-app --locked` for the app, MCP protocol and real
stdio-process fixtures, and `node --test app-ui/tests/*.test.mjs` for UI checks.
