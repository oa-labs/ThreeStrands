import { CircleAlert } from "lucide-react";
import { useRef, useState } from "react";
import type { Account, UnreadCounts } from "./domain";
import { HoverTooltip } from "./AppChrome";
import { ICON_SIZE } from "./iconSizes";

export function AccountSwitcher({
  accounts,
  unreadCounts,
  activeAccountId,
  onSwitch,
  onShowAll,
  onReorder,
}: {
  accounts: Account[];
  unreadCounts: UnreadCounts;
  activeAccountId: string | null;
  onSwitch(email: string): void;
  onShowAll(): void;
  onReorder(emails: string[]): void;
}) {
  const [draggedEmail, setDraggedEmail] = useState<string | null>(null);
  const [dragOverEmail, setDragOverEmail] = useState<string | null>(null);
  const draggedEmailRef = useRef<string | null>(null);

  if (accounts.length === 0) return null;

  const totalUnread = accounts.reduce((total, account) => total + (unreadCounts[account.email] ?? 0), 0);

  const clearDragState = () => {
    draggedEmailRef.current = null;
    setDraggedEmail(null);
    setDragOverEmail(null);
  };

  const handleDrop = (targetEmail: string, transferredEmail: string) => {
    const sourceEmail = draggedEmailRef.current ?? transferredEmail;
    if (sourceEmail && sourceEmail !== targetEmail) {
      const from = accounts.findIndex((account) => account.email === sourceEmail);
      const to = accounts.findIndex((account) => account.email === targetEmail);
      if (from !== -1 && to !== -1) {
        const next = [...accounts];
        const [moved] = next.splice(from, 1);
        next.splice(to, 0, moved!);
        onReorder(next.map((account) => account.email));
      }
    }
    clearDragState();
  };

  return (
    <div className="account-rail" role="radiogroup" aria-label="Filter by account">
      {accounts.length > 1 ? <HoverTooltip label="All accounts">
        <button
          type="button"
          role="radio"
          aria-checked={activeAccountId === null}
          aria-label={totalUnread > 0 ? `All accounts, ${totalUnread} unread` : "All Accounts"}
          className={`account-icon all-accounts ${activeAccountId === null ? "active" : ""}`}
          onClick={onShowAll}
        >
          {totalUnread > 0 ? <UnreadBadge count={totalUnread} /> : null}
        </button>
      </HoverTooltip> : null}
      {accounts.map((account) => {
        const name = account.displayName ?? account.email;
        const unreadCount = unreadCounts[account.email] ?? 0;
        const needsReconnect = account.status === "needs_reauth";
        const selected = activeAccountId === account.email || (accounts.length === 1 && activeAccountId === null);
        return (
          <HoverTooltip key={account.email} label={needsReconnect ? `${account.email} · Needs reconnect in Mail Accounts` : account.email}>
            <button
              type="button"
              role="radio"
              aria-checked={selected}
              aria-label={[name, unreadCount > 0 ? `${unreadCount} unread` : null, needsReconnect ? "Needs reconnect" : null].filter(Boolean).join(", ")}
              draggable
              className={`account-icon ${selected ? "active" : ""} ${draggedEmail === account.email ? "dragging" : ""} ${dragOverEmail === account.email && draggedEmail !== account.email ? "drag-over" : ""}`}
              style={{ background: account.color }}
              onClick={() => onSwitch(account.email)}
              onDragStart={(event) => {
                draggedEmailRef.current = account.email;
                setDraggedEmail(account.email);
                event.dataTransfer.effectAllowed = "move";
                event.dataTransfer.setData("text/plain", account.email);
              }}
              onDragEnter={(event) => {
                event.preventDefault();
                if (draggedEmail && draggedEmail !== account.email) setDragOverEmail(account.email);
              }}
              onDragOver={(event) => {
                event.preventDefault();
                event.dataTransfer.dropEffect = "move";
              }}
              onDragLeave={() => setDragOverEmail((current) => (current === account.email ? null : current))}
              onDrop={(event) => {
                event.preventDefault();
                handleDrop(account.email, event.dataTransfer.getData("text/plain"));
              }}
              onDragEnd={clearDragState}
            >
              {name.charAt(0).toUpperCase()}
              {unreadCount > 0 ? <UnreadBadge count={unreadCount} /> : null}
              {needsReconnect ? <span className="account-reconnect-badge" aria-hidden="true"><CircleAlert size={ICON_SIZE.sm} strokeWidth={2.5} /></span> : null}
            </button>
          </HoverTooltip>
        );
      })}
    </div>
  );
}

function UnreadBadge({ count }: { count: number }) {
  return <span className="account-unread-badge" aria-hidden="true">{count > 99 ? "99+" : count}</span>;
}
