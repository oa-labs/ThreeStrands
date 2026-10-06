import { useEffect, useId, useMemo, useState, type ReactNode } from "react";
import { ChevronDown, ChevronRight, FileText, MessageSquareText } from "lucide-react";
import { mailClient } from "./data/client";
import type { Account, ContactFiles, ContactTimelineItem, DomainContext, Message, ThreadDetail } from "./domain";
import { parseAddress } from "./emailAddress";
import { errorMessage } from "./errors";
import { CONTACT_FILE_LIMIT, CONTEXT_SECTION_ROWS, DOMAIN_CONTEXT_LIMIT, formatHistoryDate, organizationDomain, THREAD_OUTLINE_MIN_MESSAGES } from "./contactContext";
import { formatAttachmentSize, splitAttachmentName } from "./threadPresentation";
import { decodeHtmlEntities } from "./SafeMessage";

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
 * The one heading layout every context panel section shares: an uppercase
 * label (optionally a collapse toggle) on the left, then the item count and
 * any section actions on the right. Labels carry no icons.
 */
export function ContextSectionHeader({ title, titleId, count, toggle, actions }: {
  title: ReactNode;
  /** Id on the label, for a section labelled by its heading. */
  titleId?: string;
  count?: number;
  /** Makes the label a collapse toggle for the element with id `controls`. */
  toggle?: { collapsed: boolean; controls: string; onToggle(): void };
  actions?: ReactNode;
}) {
  return (
    <header className="context-section-header">
      <h3>
        {toggle ? (
          <button type="button" className="context-section-toggle" aria-expanded={!toggle.collapsed} aria-controls={toggle.controls} onClick={toggle.onToggle}>
            {toggle.collapsed ? <ChevronRight size={13} aria-hidden="true" /> : <ChevronDown size={13} aria-hidden="true" />}
            <span id={titleId}>{title}</span>
          </button>
        ) : <span id={titleId}>{title}</span>}
      </h3>
      {(count !== undefined && count > 0) || actions ? (
        <div className="context-section-header-actions">
          {count !== undefined && count > 0 ? <span className="context-count">{count}</span> : null}
          {actions}
        </div>
      ) : null}
    </header>
  );
}

/**
 * A context panel section whose heading collapses it (remembered per device)
 * and whose rows stop at a few with "Show more", so stacked sections stay
 * scannable instead of pushing everything else down.
 */
export function ContextSection({ id, title, label, count, actions, note, rows, limit = CONTEXT_SECTION_ROWS, footer, className }: {
  /** Stable key for the remembered collapse state. */
  id: string;
  title: ReactNode;
  /** An accessible region name, when it should differ from the visible title. */
  label?: string;
  count?: number;
  /** Buttons at the end of the heading, such as Add. */
  actions?: ReactNode;
  /** A line under the heading, such as a date range. */
  note?: ReactNode;
  rows: ReactNode[];
  /** How many rows show before "Show more". */
  limit?: number;
  /** Shown under the rows once they are all visible, such as "Load older". */
  footer?: ReactNode;
  className?: string;
}) {
  const [collapsed, setCollapsed] = useState(() => readCollapsed().has(id));
  const [expanded, setExpanded] = useState(false);
  const bodyId = useId();
  const headingId = useId();
  const hidden = rows.length - limit;
  const toggle = () => {
    setCollapsed(!collapsed);
    saveCollapsed(id, !collapsed);
  };
  return (
    <section className={`context-section context-collapsible${className ? ` ${className}` : ""}`} aria-label={label} aria-labelledby={label ? undefined : headingId}>
      <ContextSectionHeader title={title} titleId={headingId} count={count} actions={actions} toggle={{ collapsed, controls: bodyId, onToggle: toggle }} />
      <div id={bodyId} hidden={collapsed}>
        {note ? <p className="context-section-note">{note}</p> : null}
        {expanded || hidden <= 0 ? rows : rows.slice(0, limit)}
        {hidden > 0 ? (
          <button type="button" className="btn-link context-link-button" onClick={() => setExpanded(!expanded)}>
            {expanded ? "Show fewer" : `Show ${hidden} more`}
          </button>
        ) : null}
        {footer && (expanded || hidden <= 0) ? footer : null}
      </div>
    </section>
  );
}

/** Attachments the selected person sent, newest first. */
export function ContactFilesSection({ contactId, onShowMessage }: {
  contactId: string;
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
        <button type="button" className="btn-icon btn-icon-sm" aria-label={`Show the email with ${file.attachment.filename}`} title="Show email" onClick={() => onShowMessage(file.threadId, file.messageId)}>
          <MessageSquareText size={14} />
        </button>
      </div>
    );
  });
  return (
    <ContextSection
      id="files"
      className="context-files"
      title="Files"
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
          <span className="segmented context-outline-filter" role="group" aria-label="Thread outline filter">
            <button type="button" className="segment" aria-pressed={!onlyMine} onClick={() => setOnlyMine(false)}>All</button>
            <button type="button" className="segment" aria-pressed={onlyMine} onClick={() => setOnlyMine(true)}>Your replies · {mine.length}</button>
          </span>
        ) : null}
      </>}
      rows={rows}
    />
  );
}

/** Other conversations with the selected person, newest first. */
export function RecentEmailsSection({ items, onOpenThread, limit, onLoadOlder }: {
  items: ContactTimelineItem[];
  onOpenThread(id: string): void;
  limit?: number;
  /** Fetches the next page; offered once every loaded row is showing. */
  onLoadOlder?(): void;
}) {
  return (
    <ContextSection
      id="recent"
      className="context-history"
      title="Recent emails"
      count={items.length}
      limit={limit}
      footer={onLoadOlder ? <button type="button" className="btn-link context-link-button" onClick={onLoadOlder}>Load older emails</button> : undefined}
      rows={items.map((item) => {
        // The section is about one person, so the row shows what was said rather than their address.
        const snippet = decodeHtmlEntities(item.snippet).replace(/\s+/g, " ").trim();
        return (
          <button type="button" className="context-history-row" key={item.threadId} onClick={() => onOpenThread(item.threadId)}>
            <strong>{item.subject || "(no subject)"}</strong>
            <small>{formatHistoryDate(item.sentAt)}{snippet ? ` · ${snippet}` : ""}</small>
          </button>
        );
      })}
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
      className="context-history context-organization"
      title={<>Others at <span className="context-domain">{domain}</span></>}
      count={context.people.length}
      note={<span title={context.people.map((person) => person.email).join(", ")}>{names.join(", ")}</span>}
      rows={threads.map((item) => (
        <button type="button" className="context-history-row" key={item.threadId} onClick={() => onOpenThread(item.threadId)}>
          <strong>{item.subject || "(no subject)"}</strong>
          <small>{formatHistoryDate(item.sentAt)} · {item.contactEmail}</small>
        </button>
      ))}
    />
  );
}
