import { Archive, Mail, MailOpen, RotateCcw, ShieldAlert, Star, Tag, Trash, X } from "lucide-react";
import type { CSSProperties, RefObject } from "react";
import { ActionButton, HoverTooltip } from "./AppChrome";
import { ICON_SIZE } from "./iconSizes";
import type { MutationTemplate } from "./threadMutations";

/** Actions for the conversations checked in a thread mailbox's list. */
export function ConversationBatchToolbar({
  inTrash, selectedCount, allVisibleChecked, allStarred, selectAllRef,
  onSelectAll, onClearSelection, onRun, onLabels,
}: {
  inTrash: boolean;
  selectedCount: number;
  allVisibleChecked: boolean;
  /** Every checked conversation is starred, so the star action unstars. */
  allStarred: boolean;
  selectAllRef: RefObject<HTMLInputElement | null>;
  onSelectAll(): void;
  onClearSelection(): void;
  onRun(title: string, template: MutationTemplate): void;
  onLabels(): void;
}) {
  const starLabel = allStarred ? "Unstar" : "Star";
  return (
    <div className="batch-toolbar" role="toolbar" aria-label="Batch actions">
      <label className="select-all">
        <input
          ref={selectAllRef}
          type="checkbox"
          checked={allVisibleChecked}
          aria-label="Select All Conversations" title="Select all visible conversations (⌘/Ctrl+A)" aria-keyshortcuts="Meta+A Control+A"
          onChange={(event) => event.target.checked ? onSelectAll() : onClearSelection()}
        />
      </label>
      <span className="batch-count">{selectedCount} selected</span>
      {/* Trash swaps Archive/Trash/Spam for Restore: 6 actions -> 3 columns, otherwise 8 -> 4. */}
      <div className="batch-actions" style={{ "--batch-columns": inTrash ? 3 : 4 } as CSSProperties}>
        {inTrash ? (
          <HoverTooltip label="Restore" placement="bottom">
            <ActionButton label="Restore" onClick={() => onRun("Restore", { kind: "trash", value: false })}>
              <RotateCcw size={ICON_SIZE.md} />
            </ActionButton>
          </HoverTooltip>
        ) : (
          <>
            <HoverTooltip label="Archive" placement="bottom">
              <ActionButton label="Archive" onClick={() => onRun("Archive", { kind: "archive", value: true })}>
                <Archive size={ICON_SIZE.md} />
              </ActionButton>
            </HoverTooltip>
            <HoverTooltip label="Trash" placement="bottom">
              <ActionButton label="Trash" onClick={() => onRun("Trash", { kind: "trash", value: true })}>
                <Trash size={ICON_SIZE.md} />
              </ActionButton>
            </HoverTooltip>
            <HoverTooltip label="Mark spam" placement="bottom">
              <ActionButton label="Mark Spam" onClick={() => onRun("Mark Spam", { kind: "spam", value: true })}>
                <ShieldAlert size={ICON_SIZE.md} />
              </ActionButton>
            </HoverTooltip>
          </>
        )}
        <HoverTooltip label="Mark read" placement="bottom">
          <ActionButton label="Mark Read" onClick={() => onRun("Mark Read", { kind: "read", value: true })}>
            <MailOpen size={ICON_SIZE.md} />
          </ActionButton>
        </HoverTooltip>
        <HoverTooltip label="Mark unread" placement="bottom">
          <ActionButton label="Mark Unread" onClick={() => onRun("Mark Unread", { kind: "read", value: false })}>
            <Mail size={ICON_SIZE.md} />
          </ActionButton>
        </HoverTooltip>
        <HoverTooltip label={starLabel} placement="bottom">
          <ActionButton
            label={starLabel}
            onClick={() => onRun(starLabel, { kind: "star", value: !allStarred })}
          >
            <Star size={ICON_SIZE.md} fill={allStarred ? "currentColor" : "none"} />
          </ActionButton>
        </HoverTooltip>
        <HoverTooltip label="Labels" placement="bottom">
          <ActionButton label="Labels" onClick={onLabels}>
            <Tag size={ICON_SIZE.md} />
          </ActionButton>
        </HoverTooltip>
        <HoverTooltip label="Clear selection" placement="bottom">
          <button
            className="btn-icon"
            aria-label="Clear Selection" title="Clear selection (Esc)" aria-keyshortcuts="Escape"
            onClick={onClearSelection}
          >
            <X size={ICON_SIZE.lg} />
          </button>
        </HoverTooltip>
      </div>
    </div>
  );
}
