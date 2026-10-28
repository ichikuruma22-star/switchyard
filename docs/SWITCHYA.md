# Switchya user guide

Switchya is a local coding workspace. Its browser interface, terminal CLI and
desktop shell use the same built-in agent and saved project sessions. The
independently installable [Switchyard Gateway](../README.md#switchyard-gateway)
provides model access and routing.

The [**Windows x64 portable preview**](https://github.com/ichikuruma22-star/switchyard/releases/tag/switchya-v0.1.0-preview.4)
is tagged `switchya-v0.1.0-preview.4`. Its archive contains the native desktop
shell and `switchya` CLI, targeting Windows 10/11 x64. It is unsigned and has
no installer or bundled runtime DLLs. Install the
[Microsoft Visual C++ v14 x64 Redistributable](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist?view=msvc-170)
and Microsoft Edge WebView2 Runtime; WebView2 is required for the desktop
shell. Extract the ZIP and open `switchya-desktop.exe`, or run
`switchya.exe serve` for the browser workspace. Configuration and saved sessions
remain in your application-data directory; they are not stored inside the ZIP.

Gateway downloads remain a separate product. The gateway's Windows and Linux
release does not establish Switchya desktop availability on those platforms.
Separate Linux checks are not a Linux desktop release. Linux and macOS
desktop downloads are not included. Native workspace tests pass on Linux
x64/ARM64 and macOS Apple Silicon/Intel; those tests do not exercise the
separate desktop shell. Source-build instructions remain below.

## Published preview verification status

The source verification snapshot is from 11 October 2026. The [platform report](PLATFORM-VERIFICATION.md)
separates current source checks from the earlier preview's runtime observations.
The release notes bundled with the ZIP record checks of the published binaries.
Later development work used real Codex and Claude Code to edit and check Switchya
through its own CLI and workspace. See the [current adapter evidence](AGENT-ADAPTERS.md#protocol-evidence).
Those development changes are newer than the `preview.2` archive and ship in
`preview.3`.

| Area | Evidence and limits |
|---|---|
| Windows automated checks | 55 app tests and 30 adapter tests passed locally. One live adapter test is opt-in and was not run in this release verification. Native whole-workspace CI passed, as did formatting and warnings-denied Clippy. |
| UI checks | All 66 workspace UI tests passed; the gateway dashboard suite also passed in CI. |
| Browser workflow | Earlier controlled-provider checks exercised file reads, patch and command approvals, denial without file changes, saved-session reload and CLI handoff. Current automated tests cover saved Codex conversations and activity summaries. These checks do not establish live-model coding quality. |
| Native Windows shell | The published ZIP's release notes record its startup and host checks. Native webview interaction is a separate boundary from browser and host tests. |
| Portable release package | The release includes a SHA-256 file and a manifest of every packaged file. Verify the downloaded archive against those files. |
| Accounts and model quality | On 4 October, six small live tasks used one existing Codex CLI account; coding, denial and interruption checks passed, while the restart check exposed the history loss addressed by this release. Preview release checks used controlled fixtures. Later development checks verified one managed Claude sign-in and real Codex/Claude coding work; cross-account switching and comparative model quality remain unverified. Claude subscription quota is unavailable because its documented status command does not expose it. |
| Other platforms | Native workspace CI passed on Linux x64/ARM64 and both macOS architectures. Linux and macOS desktop downloads are not included; see the platform report for the separate desktop build/runtime limits. |

See [Accounts](ACCOUNTS.md) for the current sign-in, usage and native-terminal
boundaries. A source check, fixture run or successful build does not establish
an end-to-end live account workflow.

## Build and open the workspace

Build the app host and CLI from the repository root:

```powershell
cargo build --release --locked -p switchyard-app
.\target\release\switchya.exe serve
```

The default command is also `serve`. It starts a local host on an allocated
`127.0.0.1` port; open the launch URL printed in the interactive terminal. Keep
that terminal running. Ctrl+C stops the host. The URL grants access to this
host session, so keep it private. A plain URL without the launch token is not
a sign-in link. Redirected output deliberately omits the private launch URL.

The Windows desktop shell has its own Cargo workspace and needs Rust 1.90+
and the [Tauri platform prerequisites](../desktop/README.md):

```powershell
cargo build --manifest-path desktop/Cargo.toml --release --locked
.\desktop\target\release\switchya-desktop.exe
```

These paths assume the default Cargo target directories. The desktop embeds
the local host, opens its authenticated page in an operating-system webview,
and stops the host and its managed agent work when the app closes. It does not
need a separately running gateway server. Building the headless gateway does
not require the desktop manifest or GUI dependencies. See
[desktop/README.md](../desktop/README.md) for the shell's current platform boundary.

## Connect a coding model

On first start, Switchya creates a gateway configuration only if one does not
already exist. Its default Windows location is
`%LOCALAPPDATA%\Switchya\gateway.toml`; saved session data is under
`%LOCALAPPDATA%\Switchya\sessions`. It falls back to `%APPDATA%` if local
application data is unavailable. This configuration is separate from a
standalone gateway's default `switchyard.toml`.

1. Choose **Connect a provider** in the initial prompt, or **Providers** in
   the project rail to reopen setup. If only starter mock models are
   available, the workspace prompts for a real provider.
2. Choose **OpenAI**, **Anthropic**, **Gemini**, or **Ollama — local**, and give
   the connection a unique configuration name.
3. For a cloud provider, choose **Save an API key** or **Use an environment
   variable**. Enter the variable's name, not its value, for the latter. The
   app process must inherit it; set a missing variable and restart the app.
   The setup dialog uses the cloud provider's official endpoint.
4. For Ollama, start the local server separately and enter its loopback `/v1`
   endpoint, normally `http://127.0.0.1:11434/v1`. This setup path uses no key.
5. Choose **Save provider**, read its configuration and discovery status,
   then **Discover models** or **Refresh status** as needed. If the main
   workspace still prompts for a coding model, choose **Refresh models** there.

A saved provider or fetched model list does not prove generation access,
billing readiness or successful coding. Saving and model discovery do not
send a generation request. Starting a task with a cloud model can consume
provider usage. API keys are stored through the local gateway configuration
and are not displayed back in the setup dialog; keep that file private.

The model selector lists real IDs available to the app's selected gateway
client key. It respects that key's model permissions. With an existing
configuration containing several keys, use `--client-key-name NAME` if the
app cannot select exactly one enabled key. An admin secret is not an app
model credential. Other gateway provider types and advanced routing remain
configuration features; the app's setup dialog exposes only the four choices
above.

## Work in a project and session

Choose **Open a project** and enter an existing local folder path. Switchya
does not clone a repository or create a new project folder through this
dialog. You can keep editing the same files in your usual editor.

Select a model, describe the task and choose **Send task**. Ctrl+Enter, or
Command+Enter where applicable, sends the task from the composer. The first
task creates a saved session. Choose an existing session in the project rail
to continue it, or **New session** for separate work. A session keeps its
selected model; choose a new session to use another model.

Conversation text and actual tool activity appear together. Read and search
tools are bounded to the opened project. File writes and local commands wait
in **Actions & results** for your decision. Only one run can be active in a
session. Opening or refreshing a session does not start another model call.

Saved built-in sessions persist locally across app restarts. Keep the session
data directory on local storage. Only one host may own it at a time; connect
the CLI to that host for concurrent app/terminal access, as described below.

## Project workbench

These additions are included since `preview.3`; the `preview.2` archive does
not have them.

The top bar keeps **Conversation**, **Files**, **Changes** and **Handoffs** in
one project workspace. Your selected project, conversation, view and appearance
are restored when the host restarts, including when it uses a different port.

- **Files** browses project folders and previews UTF-8 source with line numbers.
  Enter a known relative path or use Ctrl+P. **Use in task** adds its path to
  the current draft. Previews are read-only and bounded; protected files and
  generated folders are omitted.
- **Changes** shows Git's current staged, unstaged and untracked paths. Select
  a tracked file to inspect either diff, or an untracked file for a preview.
  This view does not stage or commit. Git hooks, external diff programs,
  text converters, filesystem monitors and configured content filters are
  disabled for these reads.
- **Terminal** opens a real PowerShell or Bash shell in the project. Ctrl+
  backtick toggles the panel. Hiding it keeps the shell running; **Stop** ends
  it. Commands you type execute directly with your OS permissions. They do
  not pass through agent approval, and the project directory is not a sandbox.
  Terminals stop with the host and are not replayed on restart. Deliberately
  detached Unix processes can outlive their terminal.
- **Handoffs** keeps immutable project notes: current state, reported checks,
  next steps and open questions. Save a note yourself or let a connected MCP
  client save one. **Use in task** adds the note to a draft for your review;
  it does not send a task. Saved source labels and verification are reports
  from the author, not authenticated identity or independently proven results.

**Handoffs → Connect agent** provides MCP JSON and Codex TOML for this project.
Keep Switchya open while using that configuration. See the [MCP guide](MCP.md)
for the tools, limits, standalone operation and connection lifecycle.

## Review edits, commands and results

For an edit, review the proposed diff and file paths. **Exact request details**
shows the operation and argument hash; full diffs can be copied or downloaded.
Choose **Allow once** to apply that proposal or **Deny** to refuse it. If you
or another tool changed a file after the preview was prepared, the built-in
engine refuses the stale edit. Ask for a fresh proposal against the current
file instead of assuming the earlier approval still applies.

For a command, review its exact text, working directory and displayed
execution details. Approval applies only to that operation once. The command
runs with your account's operating-system permissions and network access;
its project working directory is not an OS sandbox. Commands may affect files
or services outside the project if their normal permissions allow it.

### Approve automatically (opt-in)

Every edit and command asks for a decision by default. For a project you
trust, select the approval note beside the model picker (it reads **Writes
and commands need approval**) to open **Approval settings**:

- **Apply file edits without asking** applies the agent's file edits as soon as
  they are proposed.
- **Run shell commands without asking** runs proposed commands immediately,
  with your account's full machine and network access. The project folder is
  not a sandbox.

Choose either or both for the whole project. In an open session you can
instead **Set for this session only**, which overrides the project setting for
that session, or return to **Use the project setting**. While anything is
automatic, the composer note turns amber (edits) or red (commands) and the
header shows **Auto-approve on**.

Automatic approval only replaces the click. Each operation is still proposed,
recorded with its exact arguments and diff, and marked **Auto-approved** in the
transcript and review pane, followed by its real result. The tools still
refuse repository metadata (`.git`), likely secrets such as `.env` and `.ssh`,
links and Windows short-name aliases. Some edits always ask, even in a
trusted project: edits to Switchya's own data directory or gateway
configuration, to files other programs load by themselves (shell startup files
and PowerShell profiles, `.gitconfig`, `.husky`, `.githooks`,
`.pre-commit-config.yaml`, `.envrc`, `.vscode`, `.mcp.json`, `.claude.json`),
and every edit in a project that is, or contains, your home folder.
**Interrupt** stops the run at any time; nothing is rolled back. A change
applies at once to every operation that has not started, even one waiting
behind another session's change; one already waiting for your decision still
needs it. Running sessions that follow the project show the change in their
history.

The same project setting applies to **Existing agents** runs, including ones
already running: Switchya reads it when each request arrives. Switchya then
answers complete single-operation requests from Codex (commands, file changes)
and Claude Code (Write, Edit, MultiEdit, NotebookEdit for edits; Bash and
PowerShell for commands) itself and records them as **Approved
automatically**, with the files or command they covered. A file change
qualifies only when every file it writes is an ordinary file inside the
project, under the same rules as the built-in agent, and outside the agent
account's own settings. Requests that cannot be allowed once (deny-only, such
as a broad Codex write grant or Claude's plan-mode and question tools) and any
other tool, including MCP and web tools, still wait for you. The agents keep
their own policies: Codex stays in its read-only sandbox with on-request
approvals and Claude Code in default permission mode.

After an operation, inspect the recorded result, output, file changes and
command exit code. **Completed** means the model finished the run. It does
not mean the project passed tests, the change was reviewed or a release was
published. Read the observed checks separately and request missing checks
when needed.

## Interrupt and recover

**Interrupt** requests cancellation of the active model call or tool. It does
not undo completed file edits or other side effects. An interrupted session
keeps its conversation; submit an explicit follow-up with **Resume with task**.

If an operation's outcome cannot be established after an interruption or
restart, the session shows **Recovery required**. Inspect the affected files,
command output and any external effects. Record what you checked in **What
did you check?**, then acknowledge the review. This permits a new follow-up
task; it does not certify success, undo effects or replay the uncertain action.

When a connection fails during submission, use **Refresh state** first.
**Retry same request** retains the original request ID so the host can return
the existing run instead of creating another one. Do not reconstruct an
uncertain submission as a new task until its state is clear.

## Continue from the CLI

The commands below assume `switchya` is on your PATH. Otherwise use the full
path to the built `switchya.exe`. For terminal-only work, with no app host
using the same session directory:

```powershell
switchya models
switchya chat --project 'C:\projects\my-app' --model 'MODEL_ID'
switchya run --project 'C:\projects\my-app' --model 'MODEL_ID' --prompt 'Explain how this project is tested.'
switchya sessions
switchya chat --session 'SESSION_ID'
```

Use an actual model ID from `models` and an actual session ID from `sessions`
or the `Session:` line printed when the CLI creates one. `chat` is interactive;
enter `/exit` to leave. `run` submits one prompt. Without an approval setting,
a noninteractive run that needs an edit or command decision stops instead of
approving it.

To skip the prompts, opt in explicitly. `trust` changes the project setting
shown in the app; `--auto-approve` saves a setting for one session:

```powershell
switchya trust --project 'C:\projects\my-app'
switchya trust --project 'C:\projects\my-app' --edits on --commands on
switchya run --project 'C:\projects\my-app' --model 'MODEL_ID' --auto-approve all --prompt 'Fix the failing test and run the suite.'
switchya chat --session 'SESSION_ID' --auto-approve edits
```

`--auto-approve` accepts `edits`, `commands`, `all`, `off` (ask for
everything, even if the project is trusted) and `inherit` (follow the project
again). `chat` and `run` print the setting in force, and each automatically
approved operation is printed with its command or file paths. The limits in
[Approve automatically](#approve-automatically-opt-in) apply unchanged.

In an interactive terminal, review the displayed operation and type
`allow_once OPERATION_ID` or `deny OPERATION_ID` exactly. Attaching `chat` to
a session with active work shows that run and its pending decisions before
asking for a new task.

### Use the CLI while the app stays open

Start the host with an explicit, new connection file in an existing private
directory. For the browser host:

```powershell
switchya serve --write-launch-info "$env:LOCALAPPDATA\Switchya\cli-connection.json"
```

For the desktop, pass the same `--write-launch-info` option to
`switchya-desktop.exe`. Then, in another terminal:

```powershell
switchya --launch-info "$env:LOCALAPPDATA\Switchya\cli-connection.json" sessions
switchya --launch-info "$env:LOCALAPPDATA\Switchya\cli-connection.json" chat --session 'SESSION_ID'
```

The file contains the host's bearer token. Keep it private, do not commit or
share it, and use a new filename for a new host: export never overwrites an
existing file and the old credentials stop working when their host exits.
`--launch-info` uses that host's configuration; do not combine it with
`--config`, `--data-dir` or `--client-key-name`. Without a launch file, close
the app before opening the same state directory directly in the CLI.

Both surfaces accept explicit `--config PATH`, `--data-dir PATH` and
`--client-key-name NAME` for a separately configured workspace. For recovery
from the CLI, after inspecting the uncertain effects, use
`switchya recover --session SESSION_ID --expected-revision REVISION --note 'What I checked'`.
Use the current `revision` from `sessions`; recovery never reruns an action.

## Existing agents and accounts

**Existing agents** runs installed Codex, Claude Code or Antigravity CLIs with
their own model, account and configuration. Select the agent and account, then
choose **Start new conversation**. Starting a turn may consume that account's
usage. Review its tool activity and exact permission requests in **Agent
review**. Antigravity runs in plan mode: it can inspect and answer but cannot
apply edits, and Switchya cannot check its sign-in status (see
[Accounts](ACCOUNTS.md#antigravity-accounts)).

Existing-agent conversations are saved on this computer and remain after
Switchya restarts. They are separate from built-in Switchya sessions. Choose
one under **Conversations in this project** and **Send** a message to continue
it; it keeps the account that started it. Choose **New conversation** for
separate work. Opening or refreshing a conversation never starts work.

If Switchya stopped while a run was active, that run shows **Review the
interrupted run**. The agent may already have run a command or changed a file.
Inspect the project, then choose **I reviewed the project** before continuing.
Nothing is replayed. Continuing requires at least one confirmed turn and the
same project and account. Claude continuation has been checked with the real
CLI against a local mock API after restarting Switchya. This does not establish
live sign-in or model quality.

Codex applies its own patch validation and
permission rules. Switchya requests and checks Codex's read-only shell policy
with network disabled, but does not wrap the external CLI or its integrations
in an additional OS sandbox. Read the permission boundary shown for the run
and for each decision. Unsupported or incomplete approval requests can be
deny-only.

Claude starts in **default** permission mode. Existing Claude allow/deny rules,
hooks and integrations still apply, so some tools can run without prompting.
**Allow once** returns only the displayed tool input and does not save a
permission rule. Write and Edit requests show their exact content; commands
show the supplied command text. Structured questions and plan-mode changes
are currently deny-only; ask Claude to continue in ordinary chat. Claude's
tools run with your OS permissions. See [the adapter contract](AGENT-ADAPTERS.md).

### Continue an agent conversation from the terminal

Preview 2 provides `switchya agent` commands for the same saved Codex
conversations shown in **Existing agents**. Update both the CLI and its host
when moving from preview 1, which does not include these commands.
Since preview 3, `--agent claude` and `--agent antigravity` are also supported,
along with the `add`, `login`, `use` and `remove` account commands; omitting
`--agent` selects Codex.

```powershell
switchya agent accounts
switchya agent add --agent codex --name "Work"
switchya agent login --profile PROFILE_ID --method device
switchya agent use --profile PROFILE_ID
switchya agent remove --profile PROFILE_ID
switchya agent run --project C:\work\my-project --profile system-codex --prompt 'Explain this project'
switchya agent run --agent claude --project C:\work\my-project --profile system-claude --prompt 'Explain this project'
switchya agent sessions
switchya agent show --run RUN_ID
switchya agent continue --run RUN_ID --prompt 'Now fix the failing test'
switchya agent watch --run RUN_ID
```

`continue` takes the latest finished run ID from `agent sessions` and preserves
its agent, project and account. It sends only the new prompt. A managed account must
already be signed in — in the app, or with `agent login` above; starting a run checks
its status when needed. Managed Antigravity profiles are not gated on a status
check, because the Antigravity CLI has none; it reports a missing sign-in itself.
Listing accounts shows cached status and does not inspect credentials.

While the app stays open, put `--launch-info PATH_TO_PRIVATE_LAUNCH_FILE` before
`agent` to connect to its host. Without that option the CLI owns a temporary
host using the same configured data directory, and shuts it down on exit. Close
the app first if it owns that directory. Reading saved runs never starts a
model turn.

`agent login` first checks a managed Codex or Claude Code profile's status
unless it is already known to be signed out, and starts a sign-in only for a
signed-out profile. An already signed-in profile is reported, not signed in
again. The command exits with an error unless the sign-in completes. Without
`--launch-info`, the sign-in runs in the command's temporary host, so Ctrl-C
cancels it. With the app started with `--write-launch-info` (see
[Use the CLI while the app stays open](#use-the-cli-while-the-app-stays-open)),
use `--launch-info` to sign in through its host: Ctrl-C then only stops
watching, and the sign-in continues there. Either way, a sign-in that is not
completed within ten minutes times out. `agent login` refuses managed
Antigravity profiles: choose **Open Antigravity** on the profile's card in the
app and sign in in that terminal (see
[Accounts](ACCOUNTS.md#antigravity-accounts)).

`watch` attaches to a run the app or another terminal already owns, using the
same host, saved events and approval handling. It only reads activity and
answers approvals: it never starts, continues or replays a model turn. Use it
after a viewer exits while the run is still active — losing a viewer does not
stop the agent. Ctrl-C detaches and leaves the agent running; the CLI then names
`agent show`, `agent watch` and `agent interrupt` for that run. Without an
interactive terminal, an approval stays pending for the app or another terminal
instead of being approved, denied or interrupted. Only `run` and `continue` own
the runs they start.

Interactive approval requires typing `allow_once APPROVAL_ID` or
`deny APPROVAL_ID` for the exact displayed request. Deny-only requests cannot
be approved. When the project auto-approves edits or commands, `agent run` and
`agent continue` print that setting and each request Switchya approved
automatically; see [Approve automatically](#approve-automatically-opt-in). Without an interactive terminal, approval-required work that this
CLI started stops without approval. For a started run, Ctrl-C requests
interruption; the CLI reads back the outcome and reports when it is still
uncertain.

Run and event reads are repeated a small bounded number of times when a read
fails to complete or returns an undecodable response. These requests do not
change saved work. Starts, continuations, decisions, recovery and
interruption are never repeated: their outcome may be unknown rather than
absent. A failed read does not establish why the host was slow to answer, so
inspect the run with `agent show` or the app before assuming anything about it.

Completed activity is shown as readable command/tool status with bounded output.
When a display is shortened, `agent show --run RUN_ID` provides the full saved
event. Pending approval previews retain the exact proposed input.

For an uncertain run, inspect `agent show` and the project before explicitly
using `switchya agent recover --run RUN_ID --reviewed`. Recovery does not replay
work or establish whether an earlier command completed. To stop an active run
from another terminal, use `switchya agent interrupt --run RUN_ID` with the
active host's `--launch-info` file.

Every start prints a command UUID. If submission cannot be confirmed, inspect
`agent sessions` before trying again. `--command-id UUID` can reconcile the
exact same start request; changing its prompt, project, profile or continued
run is rejected. Starts and approval decisions are never automatically retried.

### Manage CLI sign-ins

Codex and Claude CLI account profiles belong to external-agent workflows.
They do not become provider API keys for the built-in agent or Switchyard
Gateway. The gateway continues to use configured API keys or service accounts.
Install the relevant official CLI separately before using its account profile.

If you already use that CLI, select **Existing Codex CLI** or **Existing Claude
CLI** in Accounts or the run form. No separate sign-in is needed. **Refresh
account** reads status through the official CLI when requested. Existing saved
defaults remain unchanged; choose **Use as default** to select a built-in entry.

To add a separate managed account:

1. Open **Accounts** in the project rail. Under **Add an account profile**,
   choose Codex or Claude Code, enter a label and choose **Create profile**.
2. A new profile starts with **Sign-in status unknown**. Choose **Refresh
   account** to request a status check from the official CLI. Starting
   Switchya, opening Accounts, refreshing the list and creating a profile
   only read or create cached metadata; they do not inspect CLI sign-ins.
3. After a check confirms **Signed out**, choose **Sign in** for that managed
   profile and complete the official provider flow. Use **Continue in browser** when offered, then
   return to Switchya and refresh the account. **Cancel sign-in** stops the
   pending login; it is not a logout action.
4. Choose **Use as default** for future work, or choose an account in the
   existing-agent run form. A running task keeps its original profile.

Managed profiles keep separate CLI homes and sign-ins. They do not copy or
replace the existing CLI's authentication. The existing Codex and Claude CLI
entries inherit their current environment and configuration; Switchya can
inspect and select them, but cannot replace their sign-ins through the account
controls. Conversations retain their original profile across host restarts. To
add another identity, create another managed profile. A managed profile can
be removed from **Accounts**: choose **Remove profile** and confirm to delete
the profile and the sign-in stored in its private CLI home. If it was the
default, the default returns to the Existing CLI entry, and conversations
that used it cannot continue. Removal is refused while that profile has work
in flight. Switchya still has no in-place credential replacement; remove and
recreate a profile instead.

Codex usage windows and reset times come from its documented account API when
available. Missing limits mean **unavailable**, never zero usage or unlimited
capacity. Claude's documented sign-in status does not provide subscription
quota, so Switchya leaves Claude usage unavailable. Checking account status
does not send a model generation request. A signed-in status or available
quota still does not prove a task will succeed.

For a signed-in managed Claude profile and an opened project, **Open Claude
Code** requests a separate native terminal in that project. Its work is outside
Switchya's recorded runs: review, approval and lifecycle belong to the CLI and
terminal. No Switchya usage status line is installed in that terminal. See
[Accounts](ACCOUNTS.md) for account storage, supported operations and platform
limitations.

Oh My Pi remains discovery-only. Opening a separate external terminal does
not add its conversation, approvals or results to Switchya's saved runs.
An uncertain run must be inspected before explicitly continuing; there is no
automatic retry or resumption.

## Current boundaries

Switchya's local host is available only on loopback. It has no published
remote-access service, cloud session sync, Switchya cloud account service or
automatic update service. The selected `switchya.com` domain does not itself provide
those services. Do not treat the product roadmap as a list of shipped features.

This guide describes current source behavior and the limited checks recorded
in [Published preview verification status](#published-preview-verification-status). It does not
establish live-model coding quality or that a detected CLI is authenticated.
See [APP-API.md](APP-API.md) for the
local host contract and [APP-ROADMAP.md](APP-ROADMAP.md) for proposed work and
verification gates.
