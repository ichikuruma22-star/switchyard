# Workspace UI verification

Checked on 2026-10-03 against the local Switchya host. The browser checks used the actual app and a controlled local provider fixture. This records UI verification; it does not claim a production release or successful authentication with a live provider account.

## Browser readback

- Opened a real local project and submitted a task through the built-in agent. The recorded sequence read a file, proposed changing its value from 41 to 42, requested approval for that exact edit, and then requested approval for a PowerShell assertion. The command recorded exit code 0 and `fixture check passed`; the resulting file was independently read back as 42.
- Verified the readable conversation, proposed diff, approval controls, recorded command output and exit status. Empty tool-only assistant responses are omitted. Raw request identities, arguments and results remain available in Details.
- Reload retained the saved native session. A follow-up submitted through the CLI appeared in the same browser session.
- Checked 390px mobile and 768px tablet layouts without document overflow. Navigation and review dialogs opened and closed, restored focus, and remained usable. Dark appearance was checked and returned to System.
- The refined desktop screenshot at 1280×720 showed a composer approximately 126px high, two-line session titles, and readable command output.
- In Accounts, created two empty Codex profiles and two empty Claude profiles. Selected a default for each agent, reloaded, and verified all four profiles and both defaults persisted. This flow did not invoke sign-in or CLI account-status probes.
- Immediately after reload, the new-session model picker was temporarily disabled while the catalog loaded. A fresh browser readback then showed the fixture model selected and enabled. No picker defect was reproduced.

## Source checks

The frontend has 56 validated focused checks covering authenticated transport, uncertain request handling, exact retry identities, account-profile binding, event isolation, escaped diffs, transcript construction, truthful usage values, managed-account readiness, review results, and rendering with the shipped Preact/HTM runtime. The renderer regression also removes the static loading fallback and renders real account-card components. Module parsing and whitespace checks passed.

The latest complete suite ran 55 checks before the final account-readiness test was added. The subsequent affected accounts, adapters and rendering suite passed all 15 checks; unchanged checks retain their earlier valid results. Copy-only changes after that passed the rendering regression and parsing checks.

## Workspace polish readback (2026-10-15)

Checked in Chromium against the local debug host serving `app-ui` from disk, with one project, a saved two-turn Antigravity conversation and only a mock model. No agent run was started, nothing was signed in or out, and no profile was changed. The pending-approval states were checked with recorded fixtures in the test suite, not with a live provider.

- 1440×900, light and dark: in the open Antigravity conversation every message, notice, tool row, turn rule and the composer start at x = 349px. The composer area is 147px high and the conversation 596px (66% of the viewport). The transcript shows two user and two Antigravity messages, one plan-mode notice, one "Continued" divider and two "Antigravity reported the turn completed" rules, with no Starting, Working or Completed rows; the tool row reads "Read file" with `view_file` in its Details. Text contrast sweeps found no element below 4.5:1 in either theme. In dark, the composer border measures 3.07:1 and the rail selects 3.54:1 against Graphite.
- 1440×900, built-in agent with only a mock model: the empty state reads "Connect a model to start." with Connect a provider, Use an existing agent and Refresh models; the model picker reads "No model connected" and Send uses the disabled style. Focus rings on both mode-switch segments are drawn inside the control and are fully visible.
- 1024×768 first run: the headline and all three setup actions are visible, the conversation scroller has no overflow (438px content in a 438px view), and the empty state and composer share x = 301px.
- 375×812: setup actions render one per row at 343×44px with no broken words; view tabs, the Terminal toggle (44×44, icon only) and the header Review button do not overlap; no horizontal page scroll. In the open conversation the title wraps to two lines with an ellipsis, the account stays in the header and the composer area is 179px.
- 320×640: no horizontal page scroll; the view tabs scroll inside their own 216px strip and do not reach the Terminal toggle.
- Files: the NOTES.md preview contains no U+200B; its two blank lines keep the 22px line height. One omitted entry reads "1 generated, protected or unavailable entry omitted." The path field shows the Ctrl / ⌘ P shortcut.
- Accounts: each card shows its refresh hint once and its default badge as "Default for Codex" or "Default for Claude Code". Escape closed the dialog and returned focus to Accounts.
- The console reported no errors.

The source suite (`node --test app-ui/tests/*.test.mjs`) passes 90 checks. New checks cover progress-state filtering and end-state deduplication, once-per-conversation notices (errors and recovery reviews always kept), readable tool labels with exact-name fallback, `displayPath`, a file preview without hidden characters, a rendered two-turn Antigravity conversation, and the hold rule for both the built-in agent and an existing agent appearing only while the backend reports the request pending.

## Remaining boundaries

- Live provider sign-in, live CLI usage-limit/status reads, and launching an authenticated external Claude terminal remain unverified by this UI browser pass.
- Profile creation and list reads are metadata/cache operations. Only the explicit **Refresh account** action requests a CLI status probe. An unknown managed profile must be refreshed before sign-in or a run becomes available.
- Codex runs display the account returned for that run. Changing the default affects future work; uncertain-start retries retain their original profile and request ID.
- Existing-agent runs and external Claude terminals have different persistence and permission boundaries from built-in saved sessions. Those boundaries are stated in the interface.
