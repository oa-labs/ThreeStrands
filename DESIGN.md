# ThreeStrands Design Principles

## Keyboard-first interaction

ThreeStrands is a keyboard-empowered mail client. Its normal mail, Actions,
Tasks, and Calendar surfaces are reading and navigation surfaces. They must not
silently become text-entry modes or consume printable keys that are application
shortcuts.

The application has three interaction categories:

1. **Read mode** — mailbox, reader, Actions, Tasks, and Calendar. Single-letter
   commands remain active, including when focus is on read-only content.
2. **Entry mode** — compose, search, command palette, and modal forms. Printable
   keys belong to the active editor and ordinary application shortcuts are
   suspended.
3. **Transient navigation** — menus, tooltips, and read-only popovers. These may
   receive focus without disabling unrelated application commands.

Use an explicit `data-shortcut-scope` or application interaction scope for a
new entry surface. Do not infer modality from a generic ARIA role. Keep the
editable-target check as a safety net.

Native keyboard behavior wins when an interactive control is focused. In
particular, Space and Enter must activate buttons and links instead of being
captured by global shortcuts. Escape dismisses only the topmost transient
surface.

## Forms and mutations

Do not place text inputs, textareas, selects, date controls, or content-editable
regions directly in the mailbox, reader, Actions, Tasks, or Calendar workspace.
Open a modal form when a user needs to create or edit structured data.

The deliberate exceptions are:

- message composition, which is an explicit writing mode;
- mail search, entered explicitly with `/` and exited with Escape; and
- the command palette, entered explicitly with its shortcut.

Modal forms must:

- identify themselves as the `modal` shortcut scope;
- place initial focus intentionally;
- trap Tab and Shift+Tab;
- make the background inert;
- close with Escape and restore prior focus;
- preserve entered values when validation or a provider call fails; and
- provide a visible Cancel action and a clear primary action.

Single-line controls in the same form must have the same explicit vertical
height. This includes text, date, time, and number inputs as well as selects.
Use shared `box-sizing` and height rules instead of relying on native padding or
line-height, because browsers render dropdowns differently from text inputs.
Textareas and multi-line editors are exempt from the fixed-height rule.

`Cmd/Ctrl+Enter` is the standard optional shortcut for submitting a structured
form. Textareas retain ordinary Enter for new lines.

Reversible one-step actions such as starring, completing a task, or selecting a
candidate time may remain buttons on read-only surfaces. Data entry, ambiguous
AI output, destructive actions, and external writes require a review or form
boundary appropriate to their risk.

## Workspaces

Actions and Calendar share a mutually exclusive right-side workspace. Tasks is
a primary navigation workspace: it replaces the mailbox and reader while
leaving the application sidebar available. Represent the active workspace with
one state value (`null`, `actions`, `calendar`, or `tasks`), not multiple
booleans that must be synchronized manually.

Opening a workspace must not steal focus or open a form. Workspace cards and
candidate choices should use buttons and semantic disclosure elements. A plus,
Edit, or Review action opens a modal and leaves the underlying workspace
read-only.

Current primary commands are:

- `d`: toggle Tasks;
- `t`: toggle Calendar;
- `Shift+A`: toggle Actions (`a` remains Reply All);
- `Cmd/Ctrl+D`: add a task for the selected conversation; and
- `Cmd/Ctrl+K`: open the command palette.

The focused-pane model routes `j`/`k` and Up/Down to conversations or tasks.
In Tasks, Enter opens the selected task's conversation and `x` completes or
reopens it. Keep these commands in the central command registry; do not scatter
competing window-level key listeners across components.

## AI actions

AI output is a proposal, never a mutation. Proposal cards remain read-only.
Editing a proposal opens a review dialog. Accepting an AI task opens a task
review dialog and creates the task only after the user submits it. Meeting and
future calendar writes follow the same review boundary.

Evidence remains visible during review. Failure must preserve the proposal and
the user's edits. AI analysis itself must not focus an editor or weaken global
keyboard navigation.

## Accessibility and testing

ARIA semantics do not define shortcut behavior by themselves. A read-only
popover may use dialog-like semantics without becoming an application modal;
shortcut scope must still be explicit.

Every new form or workspace interaction needs automated coverage for:

- initial focus and focus restoration;
- Tab containment and Escape ordering;
- background inertness;
- preservation of native button activation;
- suspension of letter shortcuts only during entry mode;
- absence of editable controls in read-only workspaces; and
- no persistent mutation before explicit submission.

When changing shortcut behavior, retain the existing command-registry collision
tests and add integration coverage through the same path a user invokes.
