import { useEffect, useId, useMemo, useState, type ReactNode } from "react";
import { ChevronDown, ChevronRight, FileText, MessageSquareText } from "lucide-react";
import { mailClient } from "./data/client";
import type { Account, ContactFiles, ContactTimelineItem, DomainContext, Message, ThreadDetail } from "./domain";
import { parseAddress } from "./emailAddress";
import { errorMessage } from "./errors";
import { CONTACT_FILE_LIMIT, CONTEXT_SECTION_ROWS, DOMAIN_CONTEXT_LIMIT, formatHistoryDate, organizationDomain, THREAD_OUTLINE_MIN_MESSAGES } from "./contactContext";
import { formatAttachmentSize, splitAttachmentName } from "./threadPresentation";

/** Per-device, per-section collapse choices. Transient layout, so not exported with settings. */
const COLLAPSED_KEY = "threestrands.contextPanel.collapsedSections";

function readCollapsed(): Set<string> {
  try {
    const saved = JSON.parse(localStorage.getItem(COLLAPSED_KEY) ?? "[]") as unknown;
    return new Set(Array.isArray(saved) ? saved.filter((item): item is string => typeof item === "string") : []);
  } catch {
    return new Set();
  }
}

function saveCollapsed(id: string, collapsed: boolean) {
  const next = readCollapsed();
  if (collapsed) next.add(id);
  else next.delete(id);
  try {
    localStorage.setItem(COLLAPSED_KEY, JSON.stringify([...next]));
  } catch {
    // Storage can be unavailable; the section still toggles for this session.
  }
}

/**
 * A context panel section whose heading collapses it (remembered per device)
 * and whose rows stop at a few with "Show more", so stacked sections stay
 * scannable instead of pushing everything else down.
 */
export function ContextSection({ id, title, count, note, rows, empty, className }: {
  /** Stable key for the remembered collapse state. */
  id: string;
  title: ReactNode;
  count?: number;
  /** A line under the heading, such as a date range. */
  note?: ReactNode;
  rows: ReactNode[];
  /** Shown instead of rows when there are none. */
  empty?: ReactNode;
  className?: string;
}) {
  const [collapsed, setCollapsed] = useState(() => readCollapsed().has(id));
  const [expanded, setExpanded] = useState(false);
  const bodyId = useId();
  const headingId = useId();
  const hidden = rows.length - CONTEXT_SECTION_ROWS;
  const toggle = () => {
    setCollapsed(!collapsed);
    saveCollapsed(id, !collapsed);
  };
  return (
    <section className={`context-section context-collapsible${className ? ` ${className}` : ""}`} aria-labelledby={headingId}>
      <header className="context-section-header">
        <h3>
          <button type="button" className="context-section-toggle" aria-expanded={!collapsed} aria-controls={bodyId} onClick={toggle}>
            {collapsed ? <ChevronRight size={13} aria-hidden="true" /> : <ChevronDown size={13} aria-hidden="true" />}
            <span id={headingId}>{title}</span>
            {count !== undefined && count > 0 ? <span className="context-count">{count}</span> : null}
          </button>
        </h3>
      </header>
      <div id={bodyId} hidden={collapsed}>
        {note ? <p className="context-section-note">{note}</p> : null}
        {rows.length === 0 ? empty : null}
        {expanded || hidden <= 0 ? rows : rows.slice(0, CONTEXT_SECTION_ROWS)}
        {hidden > 0 ? (
          <button type="button" className="context-link-button" onClick={() => setExpanded(!expanded)}>
            {expanded ? "Show fewer" : `Show ${hidden} more`}
          </button>
        ) : null}
      </div>
    </section>
  );
}

function firstName(name: string) {
  return name.split(/[\s,]+/)[0] || name;
}

/** Attachments the selected person sent, newest first. */
export function ContactFilesSection({ contactId, name, onShowMessage }: {
  contactId: string;
  name: string;
  onShowMessage(threadId: string, messageId: string): void;
}) {
  const [files, setFiles] = useState<ContactFiles | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    setFiles(null);
    mailClient.contactFiles(contactId, CONTACT_FILE_LIMIT)
      .then((loaded) => { if (active) { setFiles(loaded); setError(null); } })
      .catch((reason: unknown) => { if (active) setError(errorMessage(reason)); });
    return () => { active = false; };
  }, [contactId]);
  if (error) return <p className="contacts-error" role="alert">{error}</p>;
  if (!files || files.files.length === 0) return null;
  const open = (messageId: string, attachmentId: string) => {
    mailClient.openAttachment(messageId, attachmentId).catch((reason: unknown) => setError(errorMessage(reason)));
  };
  const rows = files.files.map((file) => {
    const { base, extension } = splitAttachmentName(file.attachment.filename);
    return (
      <div className="context-file" key={`${file.messageId}:${file.attachment.id}`}>
        <button type="button" className="context-file-main" title={`Open ${file.attachment.filename}`} onClick={() => open(file.messageId, file.attachment.id)}>
          <FileText size={14} aria-hidden="true" />
          <span className="context-file-text">
            <strong><span className="context-file-base">{base}</span>{extension}</strong>
            <small>{formatHistoryDate(file.sentAt)} · {formatAttachmentSize(file.attachment.size)}</small>
          </span>
        </button>
        <button type="button" className="context-icon-button" aria-label={`Show the email with ${file.attachment.filename}`} title="Show email" onClick={() => onShowMessage(file.threadId, file.messageId)}>
          <MessageSquareText size={14} />
        </button>
      </div>
    );
  });
  return (
    <ContextSection
      id="files"
      className="context-files"
      title={`Files from ${firstName(name)}`}
      count={files.total}
      note={files.total > files.files.length ? `Newest ${files.files.length} of ${files.total}` : undefined}
      rows={rows}
    />
  );
}

/**
 * An outline of a long conversation, newest first, so the reader can jump to
 * a message (or to their own replies) without scrolling the whole thread.
 */
export function ThreadOutlineSection({ detail, accounts, onShowMessage }: {
  detail: ThreadDetail;
  accounts: Account[];
  onShowMessage(threadId: string, messageId: string): void;
}) {
  const own = useMemo(() => new Set(accounts.map((account) => account.email.toLocaleLowerCase())), [accounts]);
  const [onlyMine, setOnlyMine] = useState(false);
  useEffect(() => { setOnlyMine(false); }, [detail.thread.id]);
  const isMine = (message: Message) => own.has(parseAddress(message.sender).email.toLocaleLowerCase());
  const messages = detail.messages;
  if (messages.length < THREAD_OUTLINE_MIN_MESSAGES) return null;
  const mine = messages.filter(isMine);
  const shown = (onlyMine ? mine : messages).slice().reverse();
  const first = messages[0].sentAt;
  const last = messages[messages.length - 1].sentAt;
  const range = formatHistoryDate(first) === formatHistoryDate(last) ? formatHistoryDate(last) : `${formatHistoryDate(first)} – ${formatHistoryDate(last)}`;
  const rows = shown.map((message) => {
    const sender = parseAddress(message.sender);
    const preview = message.bodyText.replace(/\s+/g, " ").trim();
    return (
      <button type="button" className="context-outline-row" key={message.id} onClick={() => onShowMessage(detail.thread.id, message.id)}>
        <span className="context-outline-meta">
          <strong>{isMine(message) ? "You" : sender.name || sender.email}</strong>
          <small>{formatHistoryDate(message.sentAt)}</small>
        </span>
        {preview ? <span className="context-outline-preview">{preview}</span> : null}
      </button>
    );
  });
  return (
    <ContextSection
      id="thread"
      className="context-outline"
      title="This thread"
      count={messages.length}
      note={<>
        <span>{range}</span>
        {mine.length > 0 ? (
          <span className="context-outline-filter" role="group" aria-label="Thread outline filter">
            <button type="button" aria-pressed={!onlyMine} onClick={() => setOnlyMine(false)}>All</button>
            <button type="button" aria-pressed={onlyMine} onClick={() => setOnlyMine(true)}>Your replies · {mine.length}</button>
          </span>
        ) : null}
      </>}
      rows={rows}
    />
  );
}

/**
 * Other conversations with the selected person. With none, it says that every
 * email with them is in the open conversation; callers show it empty only
 * when local history confirms that.
 */
export function RecentEmailsSection({ items, name, onOpenThread }: {
  items: ContactTimelineItem[];
  name: string;
  onOpenThread(id: string): void;
}) {
  return (
    <ContextSection
      id="recent"
      className="contact-sidebar-history"
      title="Recent emails"
      rows={items.map((item) => (
        <button type="button" key={item.threadId} onClick={() => onOpenThread(item.threadId)}>
          <strong>{item.subject || "(no subject)"}</strong>
          <small>{new Date(item.sentAt).toLocaleDateString()} · {item.contactEmail}</small>
        </button>
      ))}
      empty={<p className="context-status">Every email with {firstName(name)} is in this conversation.</p>}
    />
  );
}

/** Other people at the selected person's organization and their latest conversations. */
export function DomainSection({ email, addresses, accounts, hideThreadIds, onOpenThread }: {
  email: string;
  /** The selected person's addresses, left out of the results. */
  addresses: string[];
  accounts: Account[];
  /** Conversations already shown elsewhere in the panel. */
  hideThreadIds: string[];
  onOpenThread(id: string): void;
}) {
  const ownEmails = useMemo(() => accounts.map((account) => account.email), [accounts]);
  const domain = organizationDomain(email, ownEmails);
  const addressKey = addresses.join("\n");
  const [context, setContext] = useState<DomainContext | null>(null);
  useEffect(() => {
    setContext(null);
    if (!domain) return;
    let active = true;
    mailClient.domainContext(domain, addressKey.split("\n"), DOMAIN_CONTEXT_LIMIT)
      .then((loaded) => { if (active) setContext(loaded); })
      // The section is supplementary; a failed lookup leaves it out.
      .catch(() => { if (active) setContext(null); });
    return () => { active = false; };
  }, [domain, addressKey]);
  if (!domain || !context || context.people.length === 0) return null;
  const hidden = new Set(hideThreadIds);
  const threads = context.threads.filter((item) => !hidden.has(item.threadId));
  const names = context.people.map((person) => person.displayName || person.email);
  return (
    <ContextSection
      id="organization"
      className="contact-sidebar-history context-organization"
      title={<>Others at <span className="context-domain">{domain}</span></>}
      count={context.people.length}
      note={<span title={context.people.map((person) => person.email).join(", ")}>{names.join(", ")}</span>}
      rows={threads.map((item) => (
        <button type="button" key={item.threadId} onClick={() => onOpenThread(item.threadId)}>
          <strong>{item.subject || "(no subject)"}</strong>
          <small>{new Date(item.sentAt).toLocaleDateString()} · {item.contactEmail}</small>
        </button>
      ))}
    />
  );
}
