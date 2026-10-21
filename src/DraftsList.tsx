import { Paperclip, Square, SquareCheckBig } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Draft } from "./correspondence";
import { formatDisplayName, parseAddress, splitAddressList } from "./emailAddress";
import { ICON_SIZE } from "./iconSizes";
import { matchesShortcut } from "./commands";
import { formatMailTimestamp } from "./threadPresentation";
import { applySelectionGesture, selectionGestureFor, type SelectionGesture } from "./threadSelection";
import { useEscapeDismiss } from "./useEscapeDismiss";

function draftRecipients(draft: Draft): string {
  const recipients = splitAddressList([draft.to, draft.cc, draft.bcc].filter(Boolean).join(", "));
  if (recipients.length === 0) return "No recipients";
  return `To ${recipients.map((recipient) => formatDisplayName(parseAddress(recipient).name)).join(", ")}`;
}

/** The first words of the body, as the inbox shows a conversation's snippet. */
function draftPreview(draft: Draft): string {
  return draft.body.replace(/\s+/g, " ").trim().slice(0, 200);
}

/** Checked drafts for batch actions, with the inbox's click, ⌘-click and Shift-click rules. */
export function useDraftSelection(drafts: Draft[], openId: string | null) {
  const [checkedIds, setCheckedIds] = useState<Set<string>>(() => new Set());
  const anchor = useRef<string | null>(null);
  const orderedIds = useMemo(() => drafts.map((draft) => draft.id), [drafts]);

  useEffect(() => {
    setCheckedIds((current) => {
      if (current.size === 0) return current;
      const next = new Set([...current].filter((id) => orderedIds.includes(id)));
      return next.size === current.size ? current : next;
    });
  }, [orderedIds]);
  useEffect(() => { if (openId) anchor.current = openId; }, [openId]);
  useEscapeDismiss(() => setCheckedIds((current) => (current.size > 0 ? new Set() : current)), checkedIds.size > 0);

  const toggle = useCallback((id: string) => {
    anchor.current = id;
    setCheckedIds((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);
  const applyGesture = useCallback((id: string, gesture: SelectionGesture) => {
    const anchorId = anchor.current;
    if (gesture === "toggle") anchor.current = id;
    setCheckedIds((current) => applySelectionGesture({ gesture, targetId: id, anchorId, openId, orderedIds, checked: current }));
  }, [openId, orderedIds]);
  const selectAll = useCallback(() => setCheckedIds(new Set(orderedIds)), [orderedIds]);
  const clear = useCallback(() => setCheckedIds((current) => (current.size > 0 ? new Set() : current)), []);
  const open = useCallback((id: string) => {
    anchor.current = id;
    clear();
  }, [clear]);

  return { checkedIds, toggle, applyGesture, selectAll, clear, open };
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
