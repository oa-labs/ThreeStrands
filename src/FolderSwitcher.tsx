import { Check, ChevronDown } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { MailboxKind } from "./commands";
import { MAILBOX_TITLES } from "./mailboxTitles";
import { useEscapeDismiss } from "./useEscapeDismiss";
import { ICON_SIZE } from "./iconSizes";

const FOLDER_OPTIONS = [
  { id: "inbox", label: "Inbox", commandId: "mailbox.inbox", shortcut: "G I" },
  { id: "allMail", label: "All Mail", commandId: "mailbox.allMail", shortcut: "G A" },
  { id: "drafts", label: "Drafts", commandId: "drafts.open", shortcut: "G D" },
  { id: "outbox", label: "Outbox", commandId: "outbox.open", shortcut: "G O" },
  { id: "trash", label: "Trash", commandId: "mailbox.trash", shortcut: "G T" },
] as const;

export function FolderSwitcher({
  selected,
  inboxUnreadCount,
  draftCount,
  outboxCount,
  onSelect,
}: {
  selected: Exclude<MailboxKind, "split">;
  inboxUnreadCount: number;
  draftCount: number;
  outboxCount: number;
  onSelect(commandId: string): void;
}) {
  const [open, setOpen] = useState(false);
  const anchorRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const title = MAILBOX_TITLES[selected];
  useEscapeDismiss(() => {
    setOpen(false);
    triggerRef.current?.focus();
  }, open);

  useEffect(() => {
    if (!open) return;
    const closeOnOutsideClick = (event: MouseEvent) => {
      if (!anchorRef.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", closeOnOutsideClick);
    return () => document.removeEventListener("mousedown", closeOnOutsideClick);
  }, [open]);

  const countFor = (id: (typeof FOLDER_OPTIONS)[number]["id"]) => {
    if (id === "inbox") return inboxUnreadCount;
    if (id === "drafts") return draftCount;
    if (id === "outbox") return outboxCount;
    return 0;
  };

  return (
    <div className="folder-switcher" ref={anchorRef}>
      <button
        ref={triggerRef}
        type="button"
        className="folder-trigger eyebrow"
        aria-label={`Choose folder, current folder ${title}`}
        aria-expanded={open}
        aria-controls={open ? "folder-switcher-options" : undefined}
        onClick={() => setOpen((current) => !current)}
      >
        {title}<ChevronDown size={ICON_SIZE.sm} aria-hidden="true" />
      </button>
      {open ? (
        <div id="folder-switcher-options" className="folder-menu" role="group" aria-label="Folders">
          {FOLDER_OPTIONS.map((option) => {
            const count = countFor(option.id);
            return (
              <button
                key={option.id}
                type="button"
                className={`folder-menu-item ${selected === option.id ? "active" : ""}`}
                aria-current={selected === option.id ? "page" : undefined}
                onClick={() => {
                  setOpen(false);
                  onSelect(option.commandId);
                }}
              >
                <span className="folder-menu-label">
                  <span className="folder-menu-check" aria-hidden="true">{selected === option.id ? <Check size={ICON_SIZE.sm} /> : null}</span>
                  {option.label}
                </span>
                <span className="folder-menu-meta" aria-hidden="true">
                  {count > 0 ? <span>{count}</span> : null}
                  {option.shortcut ? <kbd>{option.shortcut}</kbd> : null}
                </span>
              </button>
            );
          })}
        </div>
      ) : null}
    </div>
  );
}
