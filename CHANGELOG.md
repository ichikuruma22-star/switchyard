# Changelog

## Switchya 0.1.0-preview.5 (2026-10-30)

Adds opt-in automatic approval for projects you trust, and refines the
workspace.

### Workspace

- A failed Codex turn says why, for example that the configured model is not
  available for the account or that a usage limit was reached, instead of
  stopping with only "Failed". Long messages, and any that look like they
  carry a credential, keep the general advice to check the CLI.
- Opt-in automatic approval. **Approval settings** (select the approval note
  beside the model picker) lets a trusted project apply file edits and/or run
  shell commands without a prompt; a session can override its project. It is
  off by default, and while it is on the composer note turns amber or red and
  the header reads "Auto-approve on". Every operation is still recorded and
  marked Auto-approved; protected paths stay blocked, and edits outside the
  project, to Switchya's own state, gateway or agent account settings, to
  files other programs load by themselves (shell profiles, Git, editor and
  agent settings), or anywhere in a project that contains your home folder
  still ask. The same project setting covers complete Codex and Claude Code
  edit and command requests in Existing agents, read when each request
  arrives, so turning it off reaches runs already in progress; deny-only
  requests always wait for you. From the terminal, use
  `switchya trust --edits on --commands on` or `--auto-approve` on
  `chat` and `run`.
- Approval settings opened from Existing agents shows only the project
  setting, which those runs follow. It no longer offers the override of a
  built-in session that is selected but out of view. The approval note now
  reads "Edits and commands need approval", the same words as its other
  states and the dialog; screen readers hear each checkbox by its title with
  the explanation as its description, and the project and session sets are
  named groups.
- Existing agents, the first-run view, says "No agent CLI found" when no
  Codex, Claude Code or Antigravity CLI is installed, and opens the list of
  what it checked, instead of showing only a disabled agent picker.
- Keyboard focus no longer drops to the page after Allow once, Deny or Close
  details: it returns to the review heading. On wide screens, opening a
  transcript row moves focus to its details, and Review agent request
  focuses the review pane on screen instead of opening the narrow-window
  drawer over it.
- The provider form says why a configuration name is rejected (lowercase
  letters, digits, hyphens or underscores, starting with a letter) instead
  of only disabling Save. Cancelled and timed-out commands read "Cancelled"
  and "Timed out" in transcript rows, as in their results, and a tab opened
  without its launch token says where the launch link comes from.
- Transcript rows, notices and the composer share one reading column.
  Existing-agent conversations show a repeated agent notice once, with one
  thin rule per finished turn, and tool rows use readable names such as
  "Read file" while Details keeps the exact tool name.
- Pending approvals stand out: hold amber appears only on the transcript row
  and approval card that need a decision, and the review header says
  "Approval needed" or "Nothing to approve". The first-run screen offers
  Connect a provider, Use an existing agent and Refresh models.
- Phone, keyboard and contrast fixes: setup buttons no longer break words,
  touch targets are 44px, the Built-in agent / Existing agents switch
  announces its state, small text is at least 12px, dark-theme panes are
  easier to tell apart, and copied file previews no longer contain hidden
  zero-width characters.
- On macOS, a command that finished normally is no longer reported as failed
  with "Operation not permitted (os error 1)" when its process group had
  already exited by the time Switchya cleaned it up.

### Website

- Sharing switchya.com shows a large preview card instead of a bare text
  link, and the site has an Apple touch icon.
- The download section lists the Windows requirements, the ZIP size and its
  SHA-256 hash next to the button, followed by the known limits.
- The product illustration uses the app's real wording and keeps the review
  pane at every width; tabs show a clear selected state, and small text,
  focus rings and the 404 page are fixed.
- A "From download to first task" section walks through the four steps:
  extract the ZIP and its runtimes, open a project, connect a model, then
  send a task and review it.
- A Questions section answers what Switchya costs, what data leaves your
  computer, which models work, whether files change without asking, where
  data is stored, why Windows warns about the unsigned ZIP (with a
  `Get-FileHash` check), Mac and Linux availability, and how the app relates
  to Switchyard Gateway.
- The page names local Ollama models next to OpenAI, Anthropic and Gemini,
  follows the visitor's light or dark system theme, and publishes
  SoftwareApplication and FAQ structured data for search engines. The footer
  links the user guide, changelog, security model and MIT License, and
  responses add a `Cross-Origin-Opener-Policy` header.

## 0.1.2 (2026-10-30)

A gateway release with a clearer dashboard. Configuration files from 0.1.1
load unchanged.

### Dashboard

- The Overview headline says "Degraded" when a model is cooling down on
  every credential or 5% or more of the last minute's requests failed, and
  names the model and when it comes back.
- A Routes panel on the Overview shows where each model's requests go now;
  rails turn amber while a route cools down.
- Request lists on Requests and the Overview hold still while the pointer or
  keyboard focus is on them; new requests wait behind "N new requests".

### Dependencies

- `rustls` is updated to 0.23.46.

## Switchya 0.1.0-preview.4 (2026-10-15)

A bug-fix preview from a full audit of the app, CLI, agent tools and
adapters. It includes the gateway fixes listed under 0.1.1.

- Security: on Windows, the agent's file tools refuse 8.3 short names such as
  `GIT~1` or `ENV~1`. These aliases could read, search or patch repository
  metadata and credential files that the tools block by name.
- Antigravity conversations can be continued. A follow-up message resumes the
  saved `agy` conversation; if `agy` reports a different conversation, the run
  stops for review instead of silently starting a new thread.
- Claude Code replies that follow a thinking block no longer appear twice in
  the workspace or in `switchya agent` output, and their streamed fragments
  are replaced by the completed reply in saved history.
- On macOS and Linux, **Open Antigravity** launches the CLI for managed
  Antigravity profiles instead of failing with "Account profile does not
  match the selected agent."
- Agent discovery detects an installed Oh My Pi again (in `~/.bun/bin` or on
  `PATH`) instead of always reporting it as not installed.
- `switchya agent run` and `switchya agent continue` work with managed
  Antigravity profiles. Like the workspace, the CLI no longer refuses them
  for lacking a confirmed sign-in, which Antigravity cannot report; the
  Antigravity CLI reports sign-in failures itself.
- `switchya agent login` checks a profile whose status is not yet known before
  it starts a sign-in, so signing in from the terminal works without the app
  open, and it says clearly when a profile is already signed in. It exits with
  an error when the sign-in fails or is cancelled. Without `--launch-info`,
  Ctrl-C says that the sign-in stops with the command.
- Removing a profile whose account metadata was changed outside the app no
  longer deletes its sign-in: the removal is refused and the profile keeps
  working. A removal that cannot be saved no longer stops the app from
  starting.
- `switchya mcp --project` resolves a relative path such as `.` against its
  own working directory, also when it joins the open app's host.
- Project search (the built-in agent's `search_files`, the local API's
  `GET /api/projects/:id/search` and MCP `switchya_search`) no longer fails
  when one subfolder is unreadable or has a reserved name such as `aux`: that
  entry is skipped and counted in a new `omitted` field. Search also finds
  matches in large UTF-8 files whose 64 KiB read limit falls inside a
  multi-byte character.
- On Linux and macOS, files created by the agent's `apply_patch` tool follow
  your umask (usually 0644) instead of being readable only by your user.
- Ctrl/Cmd+Enter in an existing-agent conversation that is still running,
  awaiting approval or recovery review, or unsaved no longer starts a
  separate run. The shortcut follows the same rules as the disabled Send
  button.
- Claude Code Edit approvals that replace every occurrence of the text
  (`replace_all`) say so above the before/after text.
- A task sent from **New session** no longer reappears in the composer the
  next time New session is opened. **Changes** shows the current diff or
  file preview once a later refresh succeeds, instead of keeping an earlier
  transient error such as "Project inspection is busy".
- `switchya-desktop.exe` shows invalid command-line options in an error
  dialog instead of exiting silently, and shows `--help` and `--version` text
  in a dialog when started without a console.
- The Windows portable preview README says Codex, Claude Code and Antigravity
  conversations run inside the workspace and can be shown, continued and
  recovered with `switchya agent`; opening Claude Code in its own terminal is
  optional. The archive includes the MCP guide that the user guide, local API
  guide and desktop README link to.
- Corrected the documentation for preview.3. The adapter contract describes
  the Codex device-code and Claude Console/SSO sign-in methods, the built-in
  `system-antigravity` profile and managed profile removal, and the local API
  and account guides accept `antigravity` as an agent and terminal target. The
  guide's verification-status links work again, and the workbench, MCP and
  Claude workspace notes no longer tell preview.3 users to build from source.

## 0.1.1 (2026-10-15)

A gateway bug-fix release from the same audit. Configuration files from
0.1.0 load unchanged.

### Fixes

- Security: the admin API refuses (403) a direct loopback request (one not
  relayed by a reverse proxy on this machine) whose `Host`, HTTP/2 authority
  or `Origin` names a host other than `localhost`, a loopback address,
  `server.host` or the listener's own address. A web page using DNS
  rebinding can no longer make the operator's browser send wrong secrets and
  lock the operator out of the dashboard. If you reach a loopback gateway
  through a hosts-file alias, set `server.host` to that name.
- Security: a client that sends the first bytes of the HTTP/2 preface and then
  stops is disconnected after the two-minute opening timeout. Before, such a
  connection stayed open indefinitely.
- Structured output requests from OpenAI, Responses and Gemini clients served
  by Claude no longer return an empty `{}` for map fields (for example
  pydantic `Dict[str, int]` or `patternProperties`) or free-form objects.
  These schemas are described to the model in a system instruction instead
  of being forced into a schema that only allows an empty object.
- `/v1/completions` returns log probabilities in the legacy completions shape
  (`tokens`, `token_logprobs`, `top_logprobs`, `text_offset`) instead of the
  Chat shape, in full and streamed answers. Offsets count from the start of
  the generated text and continue across stream chunks.
- `/v1/completions` refuses token-id prompts, and any other prompt that is
  not text, with a 400 `invalid_request_error` on `prompt`. Before, the
  prompt was silently replaced by a placeholder and answered.
- Gemini requests that list very many `allowedFunctionNames` or declare very
  many functions are no longer slow to process, so one large request cannot
  stall the gateway.
- Tool parameters whose union choices are objects (discriminated unions such
  as pydantic `Union` models) keep every choice when sent to a Gemini
  upstream. Before, only the first choice survived.
- `videoMetadata` (clip offsets, frame rate) and `mediaResolution` on a Gemini
  client's media part are kept when the request is re-encoded for a Gemini
  upstream.
- A `POST /v1/responses` request with `previous_response_id` or
  `conversation` for a model served by a Chat Completions, Anthropic or
  Gemini provider is refused with a 400 (`previous_response_not_found` for
  `previous_response_id`); send the full conversation in `input` instead.
  Before, it was answered without the earlier conversation while the reply
  claimed to continue it.
- Responses WebSocket follow-up turns that drop a tool call the client never
  answered also drop the reasoning in front of that call, which OpenAI
  otherwise rejected with a 400.
- Requests translated for an OpenAI Responses upstream (for example from Chat
  Completions clients) no longer fail with a duplicate reasoning item when the
  client splits one response's parallel tool calls over several assistant
  turns: the shared reasoning is restored once per request.
- When a vendor answers a pooled credential with a 400 because it disabled or
  restricted the organisation behind it (Anthropic's "This organization has
  been disabled.", Groq's `organization_restricted`), that credential rests
  and the request is retried on another one. Before, the client got the 400
  and the dead credential stayed in rotation.
- Rate limits that arrive while a model already rests on a credential after
  a 5xx, a transport error or a 404 no longer escalate its backoff, so a mixed
  burst of 5xx and 429 replies no longer rests that model on the credential
  for much longer than `rate_limit_base_secs`, doubled once per rate-limit
  window up to `rate_limit_max_secs`, would.
- An alias that is ignored because none of its targets is routable no longer
  hides the real model of the same name from other aliases that name it.
- Round-robin keeps separate cursors for models whose names differ only in
  case, and for requests only some providers can serve (raw side endpoints,
  the Realtime relay), so every credential gets its share.
- A request refused while a model is cooling down names the failure that
  started the rest on its record, not a later failure that did not lengthen
  it.
- Embeddings, image, speech and moderation requests whose JSON body is
  labelled as something else (for example curl's default form type) reach
  the upstream as `application/json`, with the upstream's own model id.
- A model listing that ends while a provider's endpoint is being changed no
  longer leaves the old endpoint's models routed to the new one.
- After a key's `rate_limit_rpm` is lowered, a refused request's `Retry-After`
  gives the real wait instead of about a second at a time.
- Realtime relay: with `auth.required = false`, a key offered in the
  `openai-insecure-api-key.<key>` WebSocket subprotocol authenticates even
  when the browser also sends an `Authorization` header that holds no key,
  such as a reverse proxy's Basic credentials. Before, such clients were
  treated as anonymous: refused with `origin_not_allowed` on a public host
  name, or recorded without their key.
- Provider `headers` values written as `env:NAME` or `${NAME}` are read from
  the environment, as API keys are. Before, they were sent upstream as the
  literal text. If the variable is unset, the provider's credentials are
  shown as unusable with the reason and no request is sent; a reference that
  names no variable is refused when the configuration is checked.
- Saving a provider through the admin API while moving one credential into
  `api_keys` no longer hands it the key of a labelled credential with the
  same mask. Before, that silently moved the labelled credential's disabled
  flag, weight, priority and proxy to the other key.
- `switchyard import-cliproxy` no longer turns CLIProxyAPI's `request-retry`
  into one attempt per retry round. `routing.max_attempts` now covers every
  imported credential (or `max-retry-credentials` of them) in each round, and
  never less than the default, so failover between keys keeps working after
  import, and the report explains the value.
- `switchyard import-cliproxy` imports a model pool of a prefixed
  OpenAI-compatible provider as `prefix/pool`, the name CLIProxyAPI served it
  under, plus the bare name unless `force-model-prefix` is on. A pool member
  whose name an earlier entry already gives to a different upstream model, or
  that another pool of the same provider is named after (earlier or later in
  the list), is left out and reported; before, the pool silently sent
  requests to that model or pool.
- `switchyard import-cliproxy` leaves out provider headers written as
  `env:NAME` and lists them in its report, since Switchyard now reads such
  values from its own environment.
- Unquoted passwords and other secrets that contain brackets (for example
  `DB_PASSWORD=x7{Kq9!mZ#2`) are redacted in logs and captured bodies instead
  of being mistaken for source code.
- Captured request and response bodies over the size cap no longer keep the
  start of a secret that was cut off at the cap, including when
  `logging.request_log_max_body_kb` is 8 MiB or more.
- A usage record written after a write that failed part-way (for example on
  a full disk) is no longer lost when usage history is reloaded.
- Dashboard: the Overview setup steps keep a reverse proxy's path prefix in
  the example curl request, so it reaches the gateway when the dashboard is
  opened at an address like `https://host/gw/admin/`.
- Dashboard: payload rules show a read error instead of loading a copy whose
  large integers and decimals such as `1.0` had been silently rewritten,
  which the next save would have written back to the configuration.
- Dashboard: on the Requests page, a status filter the gateway refuses (for
  example `6xx`) no longer lets live requests fill the list as if no filter
  were set.
- Dashboard: "Paths to remove" in a payload filter rule accepts paths that
  differ only in letter case, as the gateway treats them as different fields.
- Release tags with a prerelease suffix (for example `v0.2.0-rc.1`) are
  published as GitHub prereleases and no longer become the Latest release or
  move the `latest` container image tag.

### Documentation

- `switchyard.example.toml` shows every setting, including a client key's
  `enabled` flag, an alias's `hide_targets`, the Vertex `project` and
  `location` options and the thinking `dynamic_allowed` field.
- The design notes no longer claim the container image enables remote admin
  access; pass `-e SWITCHYARD_ADMIN_ALLOW_REMOTE=true` at run time.
- The README, design notes, security model, adapter contract, admin API
  reference, gateway API notes, agent-tools README, example configuration and
  the Switchya, account, MCP and local API guides describe the changes in
  this release and in Switchya 0.1.0-preview.4.

## Switchya 0.1.0-preview.3 (2026-10-15)

- Managed Codex and Claude Code profiles can be removed from **Accounts**.
  Removal deletes the profile's private CLI home (its stored sign-in), returns
  a removed default to the Existing CLI entry, and is refused while the
  profile has an active run, refresh, sign-in or terminal launch.
  Conversations that used a removed profile can no longer continue.
- The built-in agent's "Connect a coding model" notice also offers the
  existing Codex or Claude Code setup, opening Accounts in Existing agents
  mode.
- Sign-in methods: Codex profiles can sign in with a browser or a device
  code; Claude profiles can use a Claude subscription, Anthropic Console or
  SSO. The CLI gained `switchya agent login --profile ID [--method …]` and
  `switchya agent use --profile ID` for terminal-first account switching.
- Antigravity (`agy`) is a third existing agent: named profiles, terminal
  sign-in, and read-only plan-mode conversations in the workspace and CLI
  (`switchya agent run --agent antigravity`). Its CLI has no account-status
  command, so status stays unknown and runs surface the CLI's own sign-in
  failures. Gemini CLI is not supported: Google has ended Gemini Code Assist
  for individuals and directs users to Antigravity.
- Website: corrected the model-access section — Codex and Claude Code
  conversations run inside the workspace; opening Claude in its official
  terminal is optional for managed profiles.

## 0.1.0

First release.

### Gateway

- Client APIs: OpenAI Chat Completions, OpenAI Responses, Anthropic Messages
  and Gemini `generateContent`, streaming and non-streaming, with token
  counting and model listings in each vendor's shape. Legacy `/v1/completions`
  is served through chat.
- Any client protocol can be served by a provider that speaks another one.
  Requests in the provider's own protocol preserve provider fields, with JSON
  normalization, model routing and configured reasoning or payload changes.
- Providers: OpenAI, Anthropic, Gemini, Vertex AI (service account or API
  key), any OpenAI-compatible server, and a built-in mock provider for trying
  things out.
- Several credentials per provider with round-robin, fill-first, weighted or
  least-latency selection, priorities, session affinity, cooldowns with
  backoff and automatic failover. Streaming requests are retried on another
  credential until the first event has been sent.
- Model aliases with fallback chains, provider prefixes, exclusions, model
  discovery and a built-in catalog of model capabilities.
- Reasoning suffixes — `model(high)`, `model(8192)`, `model(none)`,
  `model(auto)` — converted to each provider's own settings and fitted to what
  the model accepts.
- Payload rules that set default values, override values or remove fields in
  upstream request bodies.
- Client API keys with model allow-lists and per-minute rate limits.
- Outbound proxies (HTTP, HTTPS, SOCKS5) globally, per provider or per
  credential. Optional TLS for the listener.

### WebSockets

- The OpenAI Responses API over WebSocket (`GET /v1/responses`) for any model,
  with per-connection conversation state so follow-up turns can send only
  their new input.
- A relay for the OpenAI Realtime API (`GET /v1/realtime`).
- A live event feed for the dashboard.
- Single-use browser connection tickets, with current key permissions checked
  during long-lived client sessions.

### Dashboard

- Embedded in the binary at `/admin/`: live overview, request log with attempt
  timelines and captured bodies, provider and credential management, model
  routes and aliases, client keys, usage and cost charts, a playground for all
  four protocols and the WebSocket endpoint, live logs, settings forms and a
  raw configuration editor. Light and dark themes; works on phones.

### Operations

- One TOML configuration file, reloaded when it changes. Edits made from the
  dashboard keep the file's comments and formatting.
- Usage history kept on disk, with estimated cost from a configurable price
  table. Optional capture of request and response bodies.
- `switchyard init`, `check` and `import-cliproxy` (converts the API-key
  sections of a CLIProxyAPI configuration).

### Release hardening

- Enforced current key policy and resolved-model permissions; anonymous browser
  requests check their Origin and Host independently of CORS configuration.
- Bounded schema conversion, tool bookkeeping, discovery responses and JSON
  display work; expanded secret redaction and private Unix file permissions.
- Completed dashboard forms, focus handling, exact usage/request drill-down,
  WebSocket tickets, configuration error mapping and credential preservation.
- Scoped release job permissions and resolved release tags to an immutable
  commit. Documented shared-account, trusted-browser and Windows ACL limits in
  [the security model](docs/SECURITY-MODEL.md).

### Artifact availability

- Windows x64 archive, built and verified locally. Linux/macOS archives and
  container publication remain pending while GitHub Actions is unavailable.

### Not included

- Signing in to consumer subscriptions or imitating vendor command-line
  clients. Providers are reached with API keys or service accounts.
