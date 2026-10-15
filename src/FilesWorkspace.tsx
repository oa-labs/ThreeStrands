import { useCallback, useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
import { Download, ExternalLink, Info, MessageSquareText, ShieldAlert, SlidersHorizontal, TriangleAlert, X } from "lucide-react";
import { mailClient } from "./data/client";
import type { WorkspaceFile } from "./domain";
import { WorkspaceContextRail } from "./WorkspaceContextRail";
import { ContextRow, ContextSection, ContextSectionHeader } from "./ContextSections";
import { FileStarButton, FILES_CHANGED } from "./FileStarButton";
import { AttachmentIcon } from "./AttachmentIcon";
import { formatAttachmentSize } from "./threadPresentation";
import { formatHistoryDate } from "./contactContext";
import { isEditableTarget } from "./commands";
import { errorMessage } from "./errors";
import { ICON_SIZE } from "./iconSizes";
import { fileTypeLabel } from "./attachmentKind";
import { formatDisplayName, parseAddress } from "./emailAddress";
import { SEARCH_INPUT_ATTRIBUTES } from "./searchInputAttributes";
import { isCalendarAttachment } from "./CalendarAttachment";
import { invitationTitle } from "./invitationTitle";
import { AddressWithCard } from "./AddressWithCard";
import { attachmentRisk } from "./attachmentRisk";
import { FilePreview } from "./FilePreview";

/**
 * What Files remembers while another view is open: filters, selection,
 * scroll, and how many files were loaded. Not the files themselves, which
 * may have been marked spam, trashed, or unstarred meanwhile; the same number
 * are read again before the list shows.
 */
export type FilesWorkspaceSnapshot = { loaded: number; selected: string | null; search: string; sender: string; type: string; direction: "" | "received" | "sent"; scope: string; after: string; before: string; starred: boolean; scroll: number };
const PAGE_SIZE = 50;
const keyFor = (file: WorkspaceFile) => JSON.stringify([file.accountId, file.messageId, file.attachment.id]);
const senderName = (file: WorkspaceFile) => formatDisplayName(parseAddress(file.sender).name);
/** Invitations are named by their meeting, since the filename (often "invite.ics") says nothing. */
const meetingTitle = (file: WorkspaceFile) => isCalendarAttachment(file.attachment) ? invitationTitle(file.subject) : null;

const SHOW_INVITATIONS_KEY = "threestrands.files.showInvitations";
function readShowInvitations() {
  try { return localStorage.getItem(SHOW_INVITATIONS_KEY) === "true"; } catch { return false; }
}
function writeShowInvitations(show: boolean) {
  try { localStorage.setItem(SHOW_INVITATIONS_KEY, String(show)); } catch { /* The choice still applies for this session. */ }
}

/** Empty native date controls can display today's date on macOS. Label the
 * unrestricted state explicitly while retaining the native picker and local-day semantics. */
function FileDateFilter({ label, value, onChange }: { label: string; value: string; onChange(value: string): void }) {
  return <label>{label}<span className="file-date-control">
    <input type="date" aria-label={label} aria-description={!value ? "Any date" : undefined} value={value} data-empty={!value} onChange={(event) => onChange(event.target.value)} />
    {!value ? <span className="file-date-placeholder" aria-hidden="true">Any date</span> : null}
  </span></label>;
}
/** The instant a local calendar day (yyyy-mm-dd) starts, `days` days later. */
const localDayStart = (day: string, days = 0) => { const [year, month, date] = day.split("-").map(Number); return new Date(year, month-1, date+days).toISOString(); };
/** Rows to fetch: a refresh re-reads every loaded row, "Load more" appends one page. */
type FileLoad = { offset: number; count: number; attempt: number };

/**
 * The selected file's sender. Other people get the reader's contact hover
 * card, and clicking their name opens them in Contacts; the user's own
 * addresses stay plain text.
 */
function FileSender({ file, own, onOpenContact }: { file: WorkspaceFile; own: string[]; onOpenContact?(id: string): void }) {
  const email = parseAddress(file.sender).email.toLocaleLowerCase();
  if (!onOpenContact || !email.includes("@") || own.some((address) => address.toLocaleLowerCase() === email)) return <>{senderName(file)}</>;
  const open = () => void mailClient.resolveContactIds([email])
    .then((owners) => owners[email], () => undefined)
    .then((id) => onOpenContact(id ?? `derived:${email}`));
  return <AddressWithCard email={email} displayName={senderName(file)} hint="Opens in Contacts" onActivate={open} onOpenContact={onOpenContact} />;
}

export function FilesWorkspace({ accountId, accountIds, onOpenThread, onOpenContact, onReportSpam, sessions, refreshKey = "" }: { refreshKey?: string; sessions?: RefObject<Map<string, FilesWorkspaceSnapshot>>; accountId: string | null; accountIds: string[]; onOpenThread(id: string): void; onOpenContact?(id: string): void; /** Marks the file's source conversation as spam, which removes its files from Files. */ onReportSpam?(threadId: string): void }) {
  const sessionKey=accountId ?? "all";
  const restored=useRef(sessions?.current.get(sessionKey));
  const main=useRef<HTMLDivElement>(null);
  const [files, setFiles] = useState<WorkspaceFile[]>([]);
  const [total, setTotal] = useState(0);
  const [hiddenCalendar, setHiddenCalendar] = useState(0);
  const [showInvitations, setShowInvitationsState] = useState(readShowInvitations);
  const setShowInvitations = (show: boolean) => { setShowInvitationsState(show); writeShowInvitations(show); };
  const [selected, setSelected] = useState<string | null>(restored.current?.selected ?? null);
  const [search, setSearch] = useState(restored.current?.search ?? "");
  const [sender, setSender] = useState(restored.current?.sender ?? "");
  const [type, setType] = useState(restored.current?.type ?? "");
  const [direction, setDirection] = useState<"" | "received" | "sent">(restored.current?.direction ?? "");
  const [scope, setScope] = useState(restored.current?.scope ?? "");
  const [after, setAfter] = useState(restored.current?.after ?? "");
  const [before, setBefore] = useState(restored.current?.before ?? "");
  const [starred, setStarred] = useState(restored.current?.starred ?? false);
  const [filtersOpen, setFiltersOpen] = useState(false);
  const filtersButton = useRef<HTMLButtonElement>(null);
  const [online, setOnline] = useState(() => navigator.onLine);
  useEffect(() => {
    const changed = () => setOnline(navigator.onLine);
    window.addEventListener("online", changed);
    window.addEventListener("offline", changed);
    return () => { window.removeEventListener("online", changed); window.removeEventListener("offline", changed); };
  }, []);
  const loaded = useRef(restored.current?.loaded ?? 0);
  if (files.length) loaded.current = files.length;
  const [load, setLoad] = useState<FileLoad>(() => ({ offset: 0, count: Math.max(PAGE_SIZE, loaded.current), attempt: 0 }));
  const reload = useCallback(() => setLoad((current) => ({ offset: 0, count: Math.max(PAGE_SIZE, loaded.current), attempt: current.attempt+1 })), []);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const rows = useRef(new Map<string, HTMLButtonElement>());
  const scoped = accountId ? [accountId] : scope && accountIds.includes(scope) ? [scope] : accountIds;
  // Date inputs name days in the user's time zone.
  // Invitations stay hidden unless asked for: by the toggle, the Calendar type, or a star.
  const excludeCalendar = !showInvitations && type !== "calendar" && !starred;
  const queryKey = JSON.stringify({ accountIds: scoped, search, sender, type, direction: direction || undefined, starred, excludeCalendar: excludeCalendar || undefined, after: after ? localDayStart(after) : undefined, before: before ? localDayStart(before, 1) : undefined });
  const lastQuery = useRef(queryKey);
  if (lastQuery.current!==queryKey) { lastQuery.current=queryKey; setLoad((current) => ({ offset: 0, count: PAGE_SIZE, attempt: current.attempt+1 })); setFiles([]); setSelected(null); }
  const lastRefresh = useRef(refreshKey);
  if (lastRefresh.current!==refreshKey) { lastRefresh.current=refreshKey; reload(); }
  useEffect(() => {
    window.addEventListener(FILES_CHANGED, reload);
    return () => window.removeEventListener(FILES_CHANGED, reload);
  }, [reload]);
  useEffect(() => {
    let active = true;
    setLoading(true); setError(null);
    const timer = window.setTimeout(() => {
      const query = JSON.parse(queryKey);
      const pages = Array.from({ length: Math.ceil(load.count/PAGE_SIZE) }, (_, index) => mailClient.listFiles({ ...query, offset: load.offset+index*PAGE_SIZE, limit: PAGE_SIZE }));
      void Promise.all(pages).then((results) => {
        if (!active) return;
        const page = results.flatMap((result) => result.files);
        setFiles((previous) => load.offset ? [...previous,...page.filter((file) => !previous.some((existing) => keyFor(existing)===keyFor(file)))] : page);
        setTotal(results[results.length-1].total);
        setHiddenCalendar(results[results.length-1].hiddenCalendar ?? 0);
        if (!load.offset) setSelected((current) => page.some((file) => keyFor(file)===current) ? current : page[0] ? keyFor(page[0]) : null);
      }).catch((reason) => { if (active) setError(errorMessage(reason)); }).finally(() => { if (active) setLoading(false); });
    }, 150);
    return () => { active=false; window.clearTimeout(timer); };
  }, [queryKey, load]);
  const snapshot=useRef<FilesWorkspaceSnapshot | null>(null);
  snapshot.current={loaded:files.length || loaded.current,selected,search,sender,type,direction,scope,after,before,starred,scroll:0};
  useLayoutEffect(() => () => { if (sessions && snapshot.current) sessions.current.set(sessionKey,{...snapshot.current,scroll:main.current?.scrollTop ?? 0}); }, [sessions,sessionKey]);
  const scrollRestored=useRef(false);
  useLayoutEffect(() => { if (!loading && !scrollRestored.current && main.current) { main.current.scrollTop=restored.current?.scroll ?? 0; scrollRestored.current=true; } }, [loading]);
  const filterChips = [
    ...(sender ? [{ id: "sender", label: `Sender: ${sender}`, clear: () => setSender("") }] : []),
    ...(type ? [{ id: "type", label: `Type: ${type[0].toUpperCase()}${type.slice(1)}`, clear: () => setType("") }] : []),
    ...(direction ? [{ id: "direction", label: direction === "sent" ? "Sent" : "Received", clear: () => setDirection("") }] : []),
    ...(!accountId && scope && accountIds.includes(scope) ? [{ id: "account", label: `Account: ${scope}`, clear: () => setScope("") }] : []),
    ...(after ? [{ id: "after", label: `From: ${new Date(localDayStart(after)).toLocaleDateString()}`, clear: () => setAfter("") }] : []),
    ...(before ? [{ id: "before", label: `Through: ${new Date(localDayStart(before)).toLocaleDateString()}`, clear: () => setBefore("") }] : []),
  ];
  const showAccount = scoped.length > 1;
  const file = files.find((item) => keyFor(item)===selected);
  const risk = file ? attachmentRisk(file.attachment.filename, file.attachment.mimeType) : null;
  const action = (operation: Promise<void>) => { void operation.catch((reason) => setError(errorMessage(reason))); };
  return <section className="files-workspace" aria-label="Files" onKeyDown={(event) => {
    if (event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey || isEditableTarget(event.target)) return;
    if (["j","k","ArrowDown","ArrowUp"].includes(event.key)) {
      event.preventDefault(); event.stopPropagation();
      const index=files.findIndex((item) => keyFor(item)===selected);
      const next=files[Math.max(0,Math.min(files.length-1,index+(["j","ArrowDown"].includes(event.key) ? 1 : -1)))];
      if (next) { setSelected(keyFor(next)); rows.current.get(keyFor(next))?.focus(); }
    }
  }}>
    <header className="files-header"><div><span className="eyebrow">Files · {accountId ?? "All accounts"}</span><h1>{total} {total===1 ? "file" : "files"}</h1></div>
      <div className="segmented" role="group" aria-label="File views"><button type="button" className="segment" aria-pressed={!starred} onClick={() => setStarred(false)}>All</button><button type="button" className="segment" aria-pressed={starred} onClick={() => setStarred(true)}>Starred</button></div>
    </header>
    <div className="files-workspace-body">
      <div ref={main} className="files-main">
        <div className="files-toolbar">
          <label className="files-search"><span className="sr-only">Search Files</span><input type="search" {...SEARCH_INPUT_ATTRIBUTES} value={search} onChange={(event) => setSearch(event.target.value)} placeholder="Search filenames" /></label>
          <button ref={filtersButton} type="button" className="btn" aria-expanded={filtersOpen} aria-controls="file-filter-controls" onClick={() => setFiltersOpen((value) => !value)}><SlidersHorizontal size={ICON_SIZE.sm} />Filters{filterChips.length ? <span className="context-count">{filterChips.length}</span> : null}</button>
          <details className="files-help" onKeyDown={(event) => {
            if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); event.currentTarget.open = false; event.currentTarget.querySelector("summary")?.focus(); }
          }}>
            <summary className="btn-icon" aria-label="About Files" title="About Files"><Info size={ICON_SIZE.sm} /></summary>
            <div className="files-help-panel" role="note" aria-label="About Files"><p>Includes attachments from mail on this device, including archived conversations. Stars stay on this device. Starring a file does not keep an offline copy.</p></div>
          </details>
        </div>
        <div id="file-filter-controls" className="files-filters" role="group" aria-label="File filters" hidden={!filtersOpen} onKeyDown={(event) => {
          if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); setFiltersOpen(false); filtersButton.current?.focus(); }
        }}>
          <label>Sender<input {...SEARCH_INPUT_ATTRIBUTES} value={sender} onChange={(event) => setSender(event.target.value)} /></label>
          <label>File Type<select value={type} onChange={(event) => setType(event.target.value)}><option value="">All Types</option>{["document","spreadsheet","presentation","image","audio","video","archive","calendar"].map((kind) => <option key={kind} value={kind}>{kind[0].toUpperCase()+kind.slice(1)}</option>)}</select></label>
          <label>Direction<select value={direction} onChange={(event) => setDirection(event.target.value as typeof direction)}><option value="">Received and Sent</option><option value="received">Received</option><option value="sent">Sent</option></select></label>
          {!accountId ? <label>Account<select value={scope} onChange={(event) => setScope(event.target.value)}><option value="">All Accounts</option>{accountIds.map((email) => <option key={email}>{email}</option>)}</select></label> : null}
          <FileDateFilter label="From Date" value={after} onChange={setAfter} />
          <FileDateFilter label="Through Date" value={before} onChange={setBefore} />
          <label className="files-filter-toggle"><input type="checkbox" checked={showInvitations} onChange={(event) => setShowInvitations(event.target.checked)} />Include Invitations</label>
        </div>
        {filterChips.length ? <div className="files-active-filters" role="group" aria-label="Active file filters">
          {filterChips.map((filter) => <button type="button" key={filter.id} className="file-filter-chip" aria-label={`Remove ${filter.label} filter`} onClick={filter.clear}>{filter.label}<X size={ICON_SIZE.xs} /></button>)}
          <button type="button" className="btn-link" onClick={() => { setSender(""); setType(""); setDirection(""); setScope(""); setAfter(""); setBefore(""); }}>Clear filters</button>
        </div> : null}
        {error ? <p className="form-error" role="alert">{error}<button type="button" className="btn" onClick={() => setLoad((current) => ({ ...current, attempt: current.attempt+1 }))}>Retry</button></p> : null}
        <div className="file-list-heading" aria-hidden="true"><span className="file-list-heading-main"><span>Filename</span><span>Sender</span><span>Date</span><span>Size</span></span><span className="file-list-star-heading" /></div>
        <div className="files-list" role="list" aria-label="Mail attachments">
          {files.map((item) => <div key={keyFor(item)} role="listitem" className={`file-list-item${selected===keyFor(item) ? " selected" : ""}`}>
            <button type="button" className="file-list-select" aria-current={selected===keyFor(item) ? "true" : undefined}
              ref={(node) => { if (node) rows.current.set(keyFor(item),node); else rows.current.delete(keyFor(item)); }}
              onClick={() => setSelected(keyFor(item))}
              onKeyDown={(event) => {
                // Enter on a file takes it to the context rail; other controls keep native Enter.
                if (event.key!=="Enter" || event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) return;
                event.preventDefault(); setSelected(keyFor(item)); window.dispatchEvent(new CustomEvent("threestrands:focus-context", { detail: "File context" }));
              }} onDoubleClick={() => action(mailClient.openAttachment(item.messageId,item.attachment.id))}>
              <span className="file-list-name"><AttachmentIcon filename={item.attachment.filename} mimeType={item.attachment.mimeType} size="sm" />{meetingTitle(item) ? <><strong title={meetingTitle(item)!}>{meetingTitle(item)}</strong><span className="file-list-filename" title={item.attachment.filename}>({item.attachment.filename})</span></> : <strong title={item.attachment.filename}>{item.attachment.filename}</strong>}</span>
              <span className="file-list-sender" title={item.sender}>{senderName(item)}{showAccount ? <small className="file-list-account">{item.accountId}</small> : null}</span>
              <time className="file-list-date" dateTime={item.sentAt}>{formatHistoryDate(item.sentAt)}</time>
              <span className="file-list-size">{formatAttachmentSize(item.attachment.size)}</span>
            </button>
            <FileStarButton accountId={item.accountId} messageId={item.messageId} attachmentId={item.attachment.id} filename={item.attachment.filename} starred={item.starred} />
          </div>)}
        </div>
        {loading ? <p className="context-status" role="status">Loading files…</p> : !files.length ? <p className="context-status">{starred ? "No starred files yet." : "No files match these filters."}</p> : null}
        {!loading && excludeCalendar && hiddenCalendar ? <p className="context-status">{hiddenCalendar} calendar {hiddenCalendar===1 ? "invitation" : "invitations"} hidden. <button type="button" className="btn-link" onClick={() => setShowInvitations(true)}>Show invitations</button></p> : null}
        {files.length<total ? <button type="button" className="btn" disabled={loading} onClick={() => setLoad((current) => ({ offset: files.length, count: PAGE_SIZE, attempt: current.attempt+1 }))}>Load more files</button> : null}
      </div>
      <WorkspaceContextRail label="File context" onReturn={() => rows.current.get(selected ?? "")?.focus()}>
        <ContextSectionHeader title="File details" icon={<AttachmentIcon filename={file?.attachment.filename ?? ""} mimeType={file?.attachment.mimeType} size="xs" />} />
        {file ? <>
          <div className="file-detail">
            <FilePreview file={file} online={online} onOpen={() => action(mailClient.openAttachment(file.messageId,file.attachment.id))} />
            <h2 className="file-detail-name">{meetingTitle(file) ?? file.attachment.filename}</h2>
            <p className="file-detail-meta">{fileTypeLabel(file.attachment.filename, file.attachment.mimeType)}{meetingTitle(file) ? ` · ${file.attachment.filename}` : ""} · {formatAttachmentSize(file.attachment.size)}</p>
            <dl className="file-detail-metadata">
              <div><dt>From</dt><dd><FileSender file={file} own={accountIds} onOpenContact={onOpenContact} />{parseAddress(file.sender).email !== senderName(file) ? <span className="file-detail-sender-email">{parseAddress(file.sender).email}</span> : null}</dd></div>
              <div><dt>{file.direction === "sent" ? "Sent" : "Received"}</dt><dd>{new Date(file.sentAt).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" })}</dd></div>
              {showAccount ? <div><dt>Account</dt><dd>{file.accountId}</dd></div> : null}
            </dl>
            {risk ? <p className="file-detail-risk" role="note"><TriangleAlert size={ICON_SIZE.sm} aria-hidden="true" />{risk.reason}</p> : null}
            {file.availability === "offline" ? <p className="file-detail-meta">Available offline</p> : !online ? <p className="file-detail-meta" role="status">{file.availability === "download" ? "Connect to the internet to open this file." : "This file may need an internet connection."}</p> : null}
            <div className="file-detail-actions"><button type="button" className="btn btn-primary" onClick={() => action(mailClient.openAttachment(file.messageId,file.attachment.id))}><ExternalLink size={ICON_SIZE.sm} />Open</button><button type="button" className="btn" onClick={() => action(mailClient.saveAttachment(file.messageId,file.attachment.id))}><Download size={ICON_SIZE.sm} />Save as…</button><FileStarButton accountId={file.accountId} messageId={file.messageId} attachmentId={file.attachment.id} filename={file.attachment.filename} starred={file.starred} labeled /></div>
          </div>
          <ContextSection id="file-source" title="Source conversation" rows={[<ContextRow key="source" icon={<MessageSquareText size={ICON_SIZE.sm} />} title={file.subject} wrapTitle onActivate={() => onOpenThread(file.threadId)} />]}
            footer={onReportSpam && file.direction === "received" ? <div className="file-source-actions"><button type="button" className="btn btn-sm btn-danger" onClick={() => onReportSpam(file.threadId)}><ShieldAlert size={ICON_SIZE.sm} />Mark Spam</button></div> : null} />
        </> : <p className="context-status">Select a file to see its details and source email.</p>}
      </WorkspaceContextRail>
    </div>
  </section>;
}
