import { ChevronDown, ChevronRight } from "lucide-react";
import { useId } from "react";
import type { TodayMail } from "./domain";
import { ICON_SIZE } from "./iconSizes";
import { groupTodayMail } from "./todayMail";
import { readLocalStorage, useLocalStorageState } from "./useLocalStorageState";

export const MAIL_SIGNAL_COLLAPSED_KEY = "threestrands.today.mailSignalCollapsed";

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/**
 * Today's one line about mail: invitations by name (they are calendar
 * decisions), counts of what needs a reply and of other unread mail, and
 * Open Inbox for the rest. Collapsing it is remembered on this device.
 */
export function TodayMailSignal({ mail, error, onOpenThread, onOpenInbox }: {
  mail: { threads: TodayMail[]; total: number } | null;
  error: string | null;
  onOpenThread(threadId: string): void;
  onOpenInbox(): void;
}) {
  const [collapsed, setCollapsed] = useLocalStorageState(MAIL_SIGNAL_COLLAPSED_KEY, () => readLocalStorage(MAIL_SIGNAL_COLLAPSED_KEY, (raw) => raw === "true", false));
  const bodyId = useId();
  const groups = groupTodayMail(mail?.threads ?? []);
  const invitations = groups.invitations;
  const reply = groups.reply.length;
  // The newest conversations are grouped; the rest of the total counts as other unread.
  const other = mail ? Math.max(0, mail.total - invitations.length - reply) : 0;
  const summary = error ? "Mail couldn't be read"
    : !mail ? "Reading mail…"
      : mail.total === 0 ? "Nothing unread"
        : [invitations.length ? plural(invitations.length, "invitation") : null, reply ? `${reply} needs a reply` : null, other ? `${other} other unread` : null].filter(Boolean).join(" · ");
  return <div className="today-mail-signal" role="group" aria-label="Mail">
    <button type="button" className="btn-link today-mail-signal-toggle" aria-expanded={!collapsed} aria-controls={collapsed ? undefined : bodyId} onClick={() => setCollapsed((value) => !value)}>
      {collapsed ? <ChevronRight size={ICON_SIZE.sm} /> : <ChevronDown size={ICON_SIZE.sm} />}Mail
    </button>
    {collapsed ? <span className="today-mail-signal-summary">{summary}</span> : <span id={bodyId} className="today-mail-signal-body">
      {error ? <span role="alert" className="form-error">{error}</span> : null}
      {invitations.length ? <span className="today-mail-signal-invitations">Invitations:{" "}
        {invitations.map((item, index) => <span key={item.thread.id}>{index ? ", " : null}<button type="button" className="btn-link" onClick={() => onOpenThread(item.thread.id)}>{item.thread.subject || "(no subject)"}</button></span>)}
      </span> : null}
      {mail && !error ? <span>{mail.total === 0 ? "Nothing unread" : [reply ? `${reply} needs a reply` : null, other ? `${other} other unread` : null].filter(Boolean).join(" · ")}</span> : null}
    </span>}
    <button type="button" className="btn-link" onClick={onOpenInbox}>Open Inbox</button>
  </div>;
}
