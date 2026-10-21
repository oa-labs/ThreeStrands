import { Paperclip, Square, SquareCheckBig } from "lucide-react";
import { useMemo } from "react";
import type { Draft } from "./correspondence";
import { formatDisplayName, parseAddress, splitAddressList } from "./emailAddress";
import { ICON_SIZE } from "./iconSizes";
import { matchesShortcut } from "./commands";
import { formatMailTimestamp } from "./threadPresentation";
import { selectionGestureFor, type SelectionGesture } from "./threadSelection";
import { useCheckedSelection } from "./useCheckedSelection";

function draftRecipients(draft: Draft): string {
  const recipients = splitAddressList([draft.to, draft.cc, draft.bcc].filter(Boolean).join(", "));
  if (recipients.length === 0) return "No recipients";
  return `To ${recipients.map((recipient) => formatDisplayName(parseAddress(recipient).name)).join(", ")}`;
}

/** The first words of the body, as the inbox shows a conversation's snippet. */
function draftPreview(draft: Draft): string {
  return draft.body.replace(/\s+/g, " ").trim().slice(0, 200);
}

/** Checked drafts for batch actions. */
export function useDraftSelection(drafts: Draft[], openId: string | null) {
  const orderedIds = useMemo(() => drafts.map((draft) => draft.id), [drafts]);
  return useCheckedSelection(orderedIds, openId);
}

export function DraftsList({
  drafts,
  openId,
  checkedIds,
  showAccount,
  accountColors,
  onOpen,
  onDiscard,
  onToggleCheck,
  onSelectionGesture,
}: {
  drafts: Draft[];
  openId: string | null;
  checkedIds: ReadonlySet<string>;
  showAccount: boolean;
  accountColors: ReadonlyMap<string, string>;
  onOpen(draft: Draft): void;
  onDiscard(draft: Draft): void;
  onToggleCheck(id: string): void;
  onSelectionGesture(id: string, gesture: SelectionGesture): void;
}) {
  if (drafts.length === 0) return <p className="empty">No saved drafts.</p>;
  return <>{drafts.map((draft) => {
    const checked = checkedIds.has(draft.id);
    const preview = draftPreview(draft);
    return (
      <button
        key={draft.id}
        type="button"
        role="option"
        aria-selected={draft.id === openId}
        aria-keyshortcuts="#"
        className={`thread-row draft-row${draft.id === openId ? " selected" : ""}`}
        onMouseDown={(event) => {
          // Keep Shift-click from extending a text selection across rows.
          if (event.shiftKey) event.preventDefault();
        }}
        onClick={(event) => {
          const gesture = selectionGestureFor(event);
          if (gesture) onSelectionGesture(draft.id, gesture);
          else onOpen(draft);
        }}
        onKeyDown={(event) => {
          if (!matchesShortcut(event.nativeEvent, "#")) return;
          event.preventDefault();
          onDiscard(draft);
        }}
      >
        <span className="row-leading">
          <span
            className={`row-check ${checked ? "checked" : ""}`}
            aria-hidden="true"
            onClick={(event) => {
              event.stopPropagation();
              onToggleCheck(draft.id);
            }}
          >
            {checked ? <SquareCheckBig size={ICON_SIZE.md} /> : <Square size={ICON_SIZE.md} />}
          </span>
          {draft.attachments.some((attachment) => !attachment.inline) ? <Paperclip className="thread-attachment" size={ICON_SIZE.xs} aria-label="Has attachments" /> : null}
        </span>
        {checked ? <span className="sr-only">Selected for batch actions</span> : null}
        <span className="unread-dot" />
        <span className="thread-content">
          <span className="thread-meta">
            <span className="thread-sender"><strong>{draftRecipients(draft)}</strong></span>
            <span className="thread-meta-trailing">
              {showAccount ? <span className="account-dot" aria-hidden="true" style={{ background: accountColors.get(draft.account) }} /> : null}
              <time>{formatMailTimestamp(new Date(draft.updatedAt).toISOString())}</time>
            </span>
          </span>
          <span className="thread-subject">{draft.subject || "(no subject)"}</span>
          <span className="thread-snippet">{preview || "No message text"}</span>
        </span>
      </button>
    );
  })}</>;
}
