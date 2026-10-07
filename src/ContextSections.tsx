import { useEffect, useId, useMemo, useRef, useState, type ReactNode } from "react";
import { ChevronDown, ChevronRight, MessageSquareText } from "lucide-react";
import { mailClient } from "./data/client";
import type { Account, ContactFiles, ContactTimelineItem, DomainContext, Message, ThreadDetail } from "./domain";
import { parseAddress } from "./emailAddress";
import { errorMessage } from "./errors";
import { CONTACT_FILE_LIMIT, CONTEXT_SECTION_ROWS, dateTileParts, DOMAIN_CONTEXT_LIMIT, formatHistoryDate, organizationDomain, THREAD_OUTLINE_MIN_MESSAGES } from "./contactContext";
import { formatAttachmentSize, splitAttachmentName } from "./threadPresentation";
import { decodeHtmlEntities } from "./SafeMessage";
import { ICON_SIZE } from "./iconSizes";
import { AttachmentIcon } from "./AttachmentIcon";
import { messagePreview } from "./messagePreview";
import { threadTextIndex } from "./threadTextIndex";

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
  // The chevron (or an empty slot) sits in the rows' glyph column, so every
  // section label starts at the same edge as its rows' text.
  return (
    <header className="context-section-header">
      <h3>
        {toggle ? (
          <button type="button" className="context-section-toggle" aria-expanded={!toggle.collapsed} aria-controls={toggle.controls} onClick={toggle.onToggle}>
            <span className="context-row-glyph" aria-hidden="true">{toggle.collapsed ? <ChevronRight size={ICON_SIZE.xs} /> : <ChevronDown size={ICON_SIZE.xs} />}</span>
            <span id={titleId}>{title}</span>
          </button>
        ) : <><span className="context-row-glyph" aria-hidden="true" /><span id={titleId}>{title}</span></>}
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
 * Gives a line its full text as a tooltip only while the text is cut off, so
 * hovering never repeats what is already visible. Checked on each hover,
 * since the panel's width changes.
 */
function revealIfTruncated(line: HTMLElement | null) {
  if (!line) return;
  const cut = [line, ...line.querySelectorAll<HTMLElement>("*")].some((node) => node.scrollWidth > node.clientWidth);
  const text = line.textContent?.trim() ?? "";
  if (cut && text) line.title = text;
  else line.removeAttribute("title");
}

/**
 * The one row layout every context panel list shares. A glyph column lines up
 * with the section headers' chevrons; the text column holds the title with the
 * row's date at the end of that line, then an optional detail line; trailing
 * actions sit after the text. Rows without a glyph keep the column, so all
 * row text in the panel starts at one edge. A title or detail line that is cut
 * off shows its full text on hover. Email rows can show their date as a
 * month-and-day tile in the glyph column instead (`dateTile`).
 */
export function ContextRow({ as: Element = "div", icon, control, title, date, dateClassName, dateTile, detail, onActivate, trailing, wrapTitle, className }: {
  as?: "div" | "article";
  /** A decorative glyph, drawn inside the row's button. */
  icon?: ReactNode;
  /** A control of its own in the glyph column, such as a checkbox. */
  control?: ReactNode;
  title: ReactNode;
  /** The row's date, always at the end of the title line. */
  date?: ReactNode;
  /** A state on the date, such as overdue. */
  dateClassName?: string;
  /**
   * An ISO time shown as a month-and-day tile in the glyph column, in place of
   * `icon` and `date`; the title line keeps only a year outside the current one.
   */
  dateTile?: string;
  /** The second line: a snippet, size, address, or source. */
  detail?: ReactNode;
  /** Makes the glyph and text one button. */
  onActivate?(): void;
  trailing?: ReactNode;
  /** Lets a long title wrap, for titles the user wrote, such as tasks. */
  wrapTitle?: boolean;
  className?: string;
}) {
  const titleRef = useRef<HTMLElement>(null);
  const detailRef = useRef<HTMLElement>(null);
  const revealTruncated = () => { revealIfTruncated(titleRef.current); revealIfTruncated(detailRef.current); };
  const classes = ["context-row", onActivate ? "context-row-interactive" : "", control ? "context-row-has-control" : "", wrapTitle ? "context-row-wrap" : "", dateTile ? "context-row-dated" : "", className ?? ""].filter(Boolean).join(" ");
  const tile = dateTile ? dateTileParts(dateTile) : null;
  const glyph = tile
    ? <span className="context-calendar-date-tile" title={tile.full}><span className="context-calendar-date-tile-month">{tile.month}</span><span className="context-calendar-date-tile-day">{tile.day}</span></span>
    : icon;
  // The tile is decorative, so the title line still names the date for assistive technology.
  const shownDate = tile
    ? <>{tile.year ? <span aria-hidden="true">{tile.year}</span> : null}<span className="sr-only">{formatHistoryDate(dateTile!)}</span></>
    : date;
  const body = <>
    {control ? null : <span className="context-row-glyph context-row-icon" aria-hidden="true">{glyph}</span>}
    <span className="context-row-text">
      <span className="context-row-line">
        <strong className="context-row-title" ref={titleRef}>{title}</strong>
        {shownDate ? <small className={dateClassName ? `context-row-date ${dateClassName}` : "context-row-date"}>{shownDate}</small> : null}
      </span>
      {detail ? <small className="context-row-detail" ref={detailRef}>{detail}</small> : null}
    </span>
  </>;
  return (
    <Element className={classes} onMouseEnter={revealTruncated}>
      {control ? <span className="context-row-glyph context-row-control">{control}</span> : null}
      {onActivate
        ? <button type="button" className="context-row-main" onClick={onActivate}>{body}</button>
        : <div className="context-row-main">{body}</div>}
      {trailing ? <span className="context-row-trailing">{trailing}</span> : null}
    </Element>
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
      <ContextRow
        key={`${file.messageId}:${file.attachment.id}`}
        className="context-file"
        icon={<AttachmentIcon filename={file.attachment.filename} mimeType={file.attachment.mimeType} size="sm" />}
        title={<><span className="context-file-base">{base}</span>{extension}</>}
        date={formatHistoryDate(file.sentAt)}
        detail={formatAttachmentSize(file.attachment.size)}
        onActivate={() => open(file.messageId, file.attachment.id)}
        trailing={<button type="button" className="btn-icon btn-icon-sm" aria-label={`Show the email with ${file.attachment.filename}`} title="Show email" onClick={() => onShowMessage(file.threadId, file.messageId)}>
          <MessageSquareText size={ICON_SIZE.sm} />
        </button>}
      />
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
  // Each preview drops text quoted from the messages before it.
  const previews = useMemo(() => {
    if (messages.length < THREAD_OUTLINE_MIN_MESSAGES) return new Map<string, string>();
    const index = threadTextIndex(messages);
    return new Map(messages.map((message, position) => [message.id, messagePreview(message, index.before(position))]));
  }, [messages]);
  if (messages.length < THREAD_OUTLINE_MIN_MESSAGES) return null;
  const mine = messages.filter(isMine);
  const shown = (onlyMine ? mine : messages).slice().reverse();
  const first = messages[0].sentAt;
  const last = messages[messages.length - 1].sentAt;
  const range = formatHistoryDate(first) === formatHistoryDate(last) ? formatHistoryDate(last) : `${formatHistoryDate(first)} – ${formatHistoryDate(last)}`;
  const rows = shown.map((message) => {
    const sender = parseAddress(message.sender);
    const preview = previews.get(message.id);
    return (
      <ContextRow
        key={message.id}
        title={isMine(message) ? "You" : sender.name || sender.email}
        dateTile={message.sentAt}
        detail={preview || undefined}
        onActivate={() => onShowMessage(detail.thread.id, message.id)}
      />
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
          <ContextRow
            key={item.threadId}
            title={item.subject || "(no subject)"}
            dateTile={item.sentAt}
            detail={snippet || undefined}
            onActivate={() => onOpenThread(item.threadId)}
          />
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
      // The heading counts the conversations it lists, like every other section; the names line covers the people.
      count={threads.length}
      note={<span title={context.people.map((person) => person.email).join(", ")}>{names.join(", ")}</span>}
      rows={threads.map((item) => (
        <ContextRow
          key={item.threadId}
          title={item.subject || "(no subject)"}
          dateTile={item.sentAt}
          detail={item.contactEmail}
          onActivate={() => onOpenThread(item.threadId)}
        />
      ))}
    />
  );
}
