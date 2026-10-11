# Switchya workspace

A coding workspace organized like a signal box: projects and sessions stay on the left, the agent's current work is readable in the center, and decisions about that work stay on the right. The gateway remains a separate product surface.

## Tokens

- Graphite `#101820`: permanent project rail; quieter than a pure black editor.
- Sheet `#f6f8fb`: light workspace background.
- Paper `#ffffff`: conversation and composer surface.
- Steel `#dce3eb`: pane separators and inactive boundaries.
- Signal blue `#176fa8`: selection, focus and activity. Dark theme uses `#76c9f8`.
- Hold amber `#97600c`: approval requests. Green and red are reserved for actual success and failure.

Archivo is the interface and conversation face: 14px controls, 15px conversation, 22px session heading. JetBrains Mono is for paths, diffs, commands and tool output, never decorative labels. Headings use Archivo's slightly condensed width. Content is left-aligned; prose stays below 76 characters per line.

## Layout

Desktop has three continuous panes with shared alignment, not a dashboard of cards.

```text
┌─ Projects / sessions ─┬─ Project, session, connection ──────────┬─ Review ─────────┐
│ Switchya             │                                       │ Pending approval │
│ Project switcher     │ Task and assistant conversation       │ Proposed changes │
│ New session          │ Tool activity alongside its turn      │ Checks / results │
│ Session history      │                                       │                  │
│                      ├─ Model / task composer / controls ─────┤                  │
│ Theme / connection   │ Persistent, reachable input           │                  │
└──────────────────────┴───────────────────────────────────────┴──────────────────┘
```

The project rail is 248px; the review pane is 340px. At intermediate widths, review becomes an accessible drawer. On phones, the conversation owns the screen with project navigation and review available as modal sheets. Controls keep 44px touch targets. Dialogs trap focus and restore it to the opener; Escape closes them. Reduced motion suppresses transitions.

## Product principles

The distinctive element is the review rail: a signal-colored vertical rule connects the decision to the agent action that needs it. Transcript messages remain plain, generous text. No decorative gradients, invented metrics, sample completed sessions, disabled placeholder navigation, or simulated progress. Empty state directs the user to choose a real folder and send a task; loading and connection failure explain their actual state. Diff and tool content is escaped text, never interpreted HTML.

The theme follows the OS until explicitly changed. Light mode retains the dark project rail; dark mode changes the sheet and review surfaces with the same information hierarchy. Model selection, interruption, resumption and approval controls reflect backend state and supported actions.

The original Switchya mark is an angular S made from a switched route. Its white track, cyan branch and endpoint signals remain legible at favicon size. `assets/switchya-mark.svg` is the canonical source for app, website and native icons. The app uses the Switchya wordmark with “Powered by Switchyard Gateway” attribution; no unowned domain is used as a destination.

The built-in agent and Codex adapter keep durable conversations. Each mode lists its conversations in the project rail and names its actual permission boundary. The selected project, conversation, mode, workspace view and appearance are saved in the local database and restored across host ports and app restarts. Drafts survive navigation within an open workspace. A completed model turn is a neutral state, never a claim that checks passed. Recovery requires a written acknowledgement and then an explicit new task.

Conversation, Files and Changes share a project workbench. Files are bounded, read-only previews with line numbers, wrapping and a path reference that can be added to a task. Changes show actual Git state, including edits from outside the conversation, with separate staged and unstaged views. Neither view executes repository-defined diff, text conversion, clean, process, hook or fsmonitor helpers. Existing filesystem path protections also apply to previews.

The terminal is a real PTY with a local xterm renderer. Opening its dock does not start a shell; Start terminal does. Hiding the dock preserves the session. Input is ordered and retains its sequence across uncertain retries; a partial write stops that shell instead of replaying input. Terminals belong to the local host and stop with it. They use the owner's local permissions, not the agent approval queue or an OS sandbox. Commands deliberately detached into another Unix session can outlive a terminal. Ctrl+backtick toggles the dock, and Ctrl/Cmd+P opens the file path input.

Accounts are separate CLI profiles. The Accounts dialog reports actual sign-in, plan, usage windows and reset times, retaining unavailable states when the CLI cannot provide them. A profile can become the default for future work; an existing run always displays its pinned account. Pending start requests retain the same profile and request ID across retries. Managed Codex profiles need confirmed sign-in, and Claude Code opens in an explicitly separate terminal whose session is not tracked in the workspace.

The native transcript omits empty tool-only model messages and uses readable action names. Exact tool identifiers, arguments and result data remain available in Details. Command output and exit status are readable directly; a denied action uses human decision copy. A pending edit counts as a proposal and is never presented as an applied change.

## Brief review

The pane split serves the requested coding workflow rather than a generic chat clone: project history is persistent, conversation is separate from executable actions, and proposed edits remain reviewable beside their approval. The railway colors carry operational meaning. Removed ornamental status counters and starter task cards because neither has real data or helps the user begin this workspace.
