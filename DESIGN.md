# ThreeStrands Design Principles

## Keyboard-first interaction

ThreeStrands is a keyboard-empowered mail client. Its normal mail, context panel,
Tasks, and Calendar surfaces are reading and navigation surfaces. They must not
silently become text-entry modes or consume printable keys that are application
shortcuts.

The application has three interaction categories:

1. **Read mode** — mailbox, reader, context panel, Tasks, and Calendar. Single-letter
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
regions directly in the mailbox, reader, context panel, Tasks, or Calendar workspace.
Open a modal form when a user needs to create or edit structured data.

The deliberate exceptions are:

- message composition, which is an explicit writing mode;
- mail search, entered explicitly with `/` and exited with Escape;
- the command palette, entered explicitly with its shortcut; and
- thread chat in the context panel, entered explicitly with `q` or `Mod+J`
  or by clicking its prompt, and exited with Escape.

Thread chat follows the search pattern rather than living as an always-present
field. In read mode it renders as a prompt button, so every single-key command
stays available. Activating it turns the prompt into a text box inside the
`modal` shortcut scope and moves focus there; Enter asks, Shift+Enter adds a
line, and Escape returns focus to where it was before, keeps unsent text, and
restores read mode. Leaving the box empty by clicking elsewhere also restores
the prompt. Nothing in the panel may take focus on its own.

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

When a field holds multiple discrete values in one text box, show committed
values as removable badges instead of a delimiter-filled text area. Keep an
input for adding another value, accept pasted lists and keyboard separators
appropriate to the value type, and commit an unfinished value on blur. Each
badge should have an accessible copy button that appears on hover or keyboard
focus and stays available on touch devices. Preserve punctuation that belongs
to a value, such as commas and semicolons in URLs.

`Cmd/Ctrl+Enter` is the standard optional shortcut for submitting a structured
form. Textareas retain ordinary Enter for new lines.

Reversible one-step actions such as starring, completing a task, or selecting a
candidate time may remain buttons on read-only surfaces. Data entry, ambiguous
AI output, destructive actions, and external writes require a review or form
boundary appropriate to their risk.

## Workspaces

Mail has one right-side context panel for the open conversation. It stacks a
compact card for the selected participant (the name opens the contact, a heart
toggles favorite, and a line of facts from local history covers how much mail,
since when, any regular cadence, and when the user last wrote), the AI brief
and suggestions, open tasks from this conversation and the person's other
conversations, upcoming meetings that include the person, files the person
sent, an outline of conversations with six or more messages, recent emails
with them, and other people at their organization's domain (never for
personal mail providers or the user's own domains). Do not add a second
always-on panel beside it; new conversation context belongs in this panel as
a section.

Keep the panel short by construction rather than by available height: a
section without content is left out, lists show three rows before "Show
more", and a section heading collapses it, remembered per device. Do not
show or hide sections based on measured space; that makes content appear and
disappear as the window resizes. An empty state is stated only when local
history confirms it, such as "Every email with Daniel is in this
conversation".
The Calendar sidebar may open beside it so meeting suggestions and candidate
times stay visible together.

Tasks, Week, and Contacts are primary navigation workspaces: they replace the
mailbox, reader, and context panel while leaving the application sidebar
available. Represent the active workspace with one state value (`null`,
`calendar`, `contacts`, `tasks`, or `week`), not multiple booleans that must be
synchronized manually.

Opening a workspace must not steal focus or open a form. Workspace cards and
candidate choices should use buttons and semantic disclosure elements. A plus,
Edit, or Review action opens a modal and leaves the underlying workspace
read-only.

Current primary commands are:

- `1`: open the Inbox/mail view;
- `3`: open the Tasks view;
- `0`: cycle between the Inbox/mail and Tasks views;
- `d`: add a task linked to the selected conversation in Mail, or a blank,
  standalone task in the Tasks view;
- `t`: toggle Calendar;
- `Shift+A`: show the conversation's suggestions in the context panel,
  fetching them if missing (`a` remains Reply All);
- `i`: summarize the conversation into the context panel's brief;
- `q` or `Mod+J`: ask about the conversation in the context panel's thread
  chat (`Mod+J` drafts with AI while composing);
- `Cmd/Ctrl+K`: open the command palette.

The focused-pane model routes `j`/`k` and Up/Down to conversations or tasks.
Tasks use the same list-and-reader structure as mail: selection shows read-only
details, Enter opens the edit modal, `e` completes, `Shift+e` reopens, and `o`
opens the linked conversation when one exists. Keep these commands in the central command
registry; do not scatter competing window-level key listeners across components.

## AI actions

The context panel's brief combines the thread summary and suggestions. When
both features are enabled and both are missing, fetch them in one provider
request; otherwise request only the missing part. A saved summary that is
current for the newest message is never regenerated without an explicit
refresh. Show confidence as a "Check details" flag, not a percentage, and
count suggestions withheld by validation instead of reporting that nothing
was found.

Proactive suggestions are opt-in. They run the same brief request after the
reader stays on a conversation for the mark-read delay (never less than the
minimum dwell in `src/proactiveBrief.ts`), skip mailing lists and
conversations with no one else in them, can be limited to people the user
has emailed, and try each conversation revision once per session. Verified
suggestions are saved per revision, so a restart does not pay for them again.
Proactive work only prepares proposals; it never creates, sends, or books
anything.

Every successful provider response records its token usage, and any cost the
provider reports, in the local `ai_usage` table. AI settings show today's and
the last week's totals; other providers' cost is estimated from per-model
prices the user enters, which stay on the device.

Thread chat answers from the open conversation, its open tasks, and the
selected person's open tasks. Other mail is shared only for a question where
the reader turns on Search all mail; that choice resets after each question,
and the answer lists every other conversation that was shared. Answers are
plain text. A chat answer's tasks and meetings join Suggested and follow the
same review boundary, and a drafted reply opens in the composer for review
rather than being sent.

Meeting suggestions and chat answers about availability are scheduled inside
the card, from the user's calendar and without AI. An exact time is checked
for conflicts as soon as the card appears, naming the events it overlaps; a
range, or a meeting with no usable time, offers the first open working-hour
slot on each of the next few days. A missing meeting timezone defaults to the
user's and is labelled. Candidate times are toggle buttons reached with Tab and
chosen with Space or Enter, never with letter or number keys, which stay
application shortcuts. Every outcome crosses a review boundary: Add to Calendar
opens the event dialog prefilled, replies open in the composer, and More Times
opens the Calendar sidebar on the meeting's day at its duration. The chat model
never states availability; it can only ask the app to show open times for a
range.

AI output is a proposal, never a mutation. Proposal cards remain read-only.
Editing a proposal opens a review dialog. Accepting an AI task opens a task
review dialog and creates the task only after the user submits it. Meeting and
future calendar writes follow the same review boundary.

Evidence remains visible during review. Failure must preserve the proposal and
the user's edits. AI analysis itself must not focus an editor or weaken global
keyboard navigation.

## Text capitalization

Follow macOS convention: title-style for anything a user picks as a discrete
control, sentence-style for anything read as a phrase.

- **Title-style** (capitalize each major word) for button text, menu items,
  dialog and modal titles, section headings, and form field labels —
  including the `aria-label` that stands in for a field or icon-only
  button's visible label. Do not capitalize articles, conjunctions, or short
  prepositions (a, an, and, as, at, but, by, for, in, of, on, or, the, to)
  unless one is the first or last word of the label. Hyphenated compounds
  capitalize both halves ("Signed-In Devices").
- **Sentence-style** (capitalize only the first word and proper nouns) for
  the native `title` attribute (tooltip), inline help text, placeholders,
  and status or error messages.

A single control's tooltip and its label may differ in case even though
they share nearby text, because they serve different roles: the label names
the control, the tooltip describes what it does. Dynamic content (email
subjects, contact names, user-entered text) is exempt — this convention
governs application chrome, not sender or user data.

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
