# Switchya workspace

A coding workspace organized like a signal box: projects and sessions stay on the left, the agent's current work is readable in the center, and decisions about that work stay on the right. The gateway remains a separate product surface.

## Tokens

- Graphite `#101820`: permanent project rail; quieter than a pure black editor.
- Sheet `#f6f8fb`: light workspace background.
- Paper `#ffffff`: conversation and composer surface.
- Steel `#dce3eb`: pane separators and inactive boundaries. Field and composer borders use `#7f8c9a` (3:1 or better on paper and sheet).
- Signal blue `#176fa8`: selection, focus and activity. Dark theme uses `#76c9f8`.
- Hold amber `#97600c`: approval requests and genuinely risky requests (network access, replace-all edits, machine permissions, unsaved history). Setup and status notices use info or neutral. Green and red are reserved for actual success and failure.
- Dark theme keeps the Graphite rail and lifts the surfaces so the three panes stay distinct: sheet `#151f2a`, paper `#1a2531`, lines `#34465a`. Selection, scrollbars and native controls on the rail (`color-scheme: dark`) take their colours from these tokens.

Archivo is the interface and conversation face. The type scale is 12/13/15/22px (28px for the empty-state headline): 13px controls, 15px conversation, 22px session heading. Nothing is set below 12px except 11px tracked uppercase labels (PROJECT, CONVERSATIONS, REVIEW) and mono identifiers inside collapsed Details. Project-name labels are 12px, untracked and in their own case. Times, counts and status labels use tabular numerals. JetBrains Mono is for paths, diffs, commands and tool output, never decorative labels. Headings use Archivo's slightly condensed width. Content is left-aligned.

The conversation has one reading edge. `--measure` (640px) is the shared column for messages, tool and state rows, notices, the empty state and the composer, which all start at the same x-coordinate. Prose inside a message keeps a 34em measure (about 75 characters); code blocks use the full column. The conversation and composer share their side padding and scrollbar gutter so the column lines up at every width.

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

The project rail is 232px; the review pane is 320px. At intermediate widths, review becomes an accessible drawer. On phones, the conversation owns the screen with project navigation and review available as modal sheets. Controls keep 44px touch targets on phones and coarse pointers, including view tabs, the Terminal toggle, terminal, file and handoff tools. Below 480px the Terminal toggle shows its icon and keeps its name for assistive technology; view tabs scroll inside their own strip rather than covering it. Dialogs trap focus and restore it to the opener; Escape closes them. Reduced motion suppresses transitions.

## Product principles

The distinctive element is the review rail. While a decision is pending, and only then, the transcript row that needs it and the approval card each carry the same 3px hold-amber rule, one on each side of the pane border; the row returns to neutral when the decision is recorded. The review header shows a labelled signal light: "Approval needed" in amber or "Nothing to approve" in muted text. Notices are tinted boxes with a 1px toned border; a thick vertical rule is reserved for decisions (approval, recorded results). Each finished turn ends with one thin rule carrying the junction icon, the outcome and its time ("Codex reported the turn completed · 14:05"); failed and interrupted turns use the stop and caution tones. Progress states (starting, working, waiting) are not transcript rows: the header status, one static live row at the end of an active turn and a polite status region report them. A notice the agent repeats on every turn is shown once per conversation; errors and recovery reviews always stay. Transcript messages remain plain, generous text. No decorative gradients, invented metrics, sample completed sessions, disabled placeholder navigation, or simulated progress. Empty state directs the user to choose a real folder and send a task; loading and connection failure explain their actual state. Diff and tool content is escaped text, never interpreted HTML.

The theme follows the OS until explicitly changed. Light mode retains the dark project rail; dark mode changes the sheet and review surfaces with the same information hierarchy. Model selection, interruption, resumption and approval controls reflect backend state and supported actions. Disabled buttons lose their fill (hover surface, muted label, no opacity), so a disabled Send never resembles an enabled control. Send is the one filled signal-blue button in the conversation pane; the rail's New action is a graphite secondary with a signal-blue icon.

The original Switchya mark is an angular S made from a switched route. Its white track, cyan branch and endpoint signals remain legible at favicon size. `assets/switchya-mark.svg` is the canonical source for app, website and native icons. The app uses the Switchya wordmark with “Powered by Switchyard Gateway” attribution; no unowned domain is used as a destination.

The built-in agent and the existing agents (Codex, Claude Code, Antigravity) keep durable conversations. The mode switch sits directly under the project picker as a labelled two-button group with pressed state. Each mode lists its conversations in the project rail and names its actual permission boundary. With no non-mock model, the built-in empty state is the setup step: Connect a provider, Use an existing agent, Refresh models. An open existing-agent conversation shows its agent and pinned account in the header, so the composer carries only the message and its actions. The selected project, conversation, mode, workspace view and appearance are saved in the local database and restored across host ports and app restarts. Drafts survive navigation within an open workspace. A completed model turn is a neutral state, never a claim that checks passed. Recovery requires a written acknowledgement and then an explicit new task.

Conversation, Files and Changes share a project workbench. Files are bounded, read-only previews with line numbers, wrapping and a path reference that can be added to a task. Changes show actual Git state, including edits from outside the conversation, with separate staged and unstaged views. Neither view executes repository-defined diff, text conversion, clean, process, hook or fsmonitor helpers. Existing filesystem path protections also apply to previews.

The terminal is a real PTY with a local xterm renderer. Opening its dock does not start a shell; Start terminal does. Hiding the dock preserves the session. Input is ordered and retains its sequence across uncertain retries; a partial write stops that shell instead of replaying input. Terminals belong to the local host and stop with it. They use the owner's local permissions, not the agent approval queue or an OS sandbox. Commands deliberately detached into another Unix session can outlive a terminal. Ctrl+backtick toggles the dock, and Ctrl/Cmd+P opens the file path input.

Accounts are separate CLI profiles. The Accounts dialog reports actual sign-in, plan, usage windows and reset times, retaining unavailable states when the CLI cannot provide them. A profile can become the default for future work; an existing run always displays its pinned account. Pending start requests retain the same profile and request ID across retries. Both agents offer a read-only Existing CLI account that uses the installed CLI's current sign-in and configuration. Managed profiles need confirmed sign-in. Claude Code runs in the workspace with the selected existing or managed profile; its optional external terminal action remains limited to managed profiles and is not tracked in the workspace.

Both transcripts omit empty tool-only model messages and use readable action names (`view_file` and `Read` read as "Read file", `Bash` and `PowerShell` as "Run command"; unknown names stay verbatim). Exact tool identifiers, arguments and result data remain available in Details. Approval titles always keep the exact tool name ("Allow Claude Code to use Edit?"). Paths shown for reading drop Windows verbatim prefixes (`\\?\`); approvals and exact details keep the recorded value. The review pane's tabs are Proposals and Results; Changes names only the Git view. Command output and exit status are readable directly; a denied action uses human decision copy. A pending edit counts as a proposal and is never presented as an applied change.

## Brief review

The pane split serves the requested coding workflow rather than a generic chat clone: project history is persistent, conversation is separate from executable actions, and proposed edits remain reviewable beside their approval. The railway colors carry operational meaning. Removed ornamental status counters and starter task cards because neither has real data or helps the user begin this workspace.
