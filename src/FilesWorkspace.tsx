import { useCallback, useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
import { Download, ExternalLink, MessageSquareText } from "lucide-react";
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

export type FilesWorkspaceSnapshot = { files: WorkspaceFile[]; total: number; selected: string | null; search: string; sender: string; type: string; direction: "" | "received" | "sent"; scope: string; after: string; before: string; starred: boolean; scroll: number };
const PAGE_SIZE = 50;
const keyFor = (file: WorkspaceFile) => JSON.stringify([file.accountId, file.messageId, file.attachment.id]);
const availability = (file: WorkspaceFile) => file.availability==="offline" ? "Available offline" : file.availability==="download" ? "Download required" : "Availability not yet known";
/** The instant a local calendar day (yyyy-mm-dd) starts, `days` days later. */
const localDayStart = (day: string, days = 0) => { const [year, month, date] = day.split("-").map(Number); return new Date(year, month-1, date+days).toISOString(); };
/** Rows to fetch: a refresh re-reads every loaded row, "Load more" appends one page. */
type FileLoad = { offset: number; count: number; attempt: number };

export function FilesWorkspace({ accountId, accountIds, onOpenThread, sessions, refreshKey = "" }: { refreshKey?: string; sessions?: RefObject<Map<string, FilesWorkspaceSnapshot>>; accountId: string | null; accountIds: string[]; onOpenThread(id: string): void }) {
  const sessionKey=accountId ?? "all";
  const restored=useRef(sessions?.current.get(sessionKey));
  const main=useRef<HTMLDivElement>(null);
  const [files, setFiles] = useState<WorkspaceFile[]>(restored.current?.files ?? []);
  const [total, setTotal] = useState(restored.current?.total ?? 0);
  const [selected, setSelected] = useState<string | null>(restored.current?.selected ?? null);
  const [search, setSearch] = useState(restored.current?.search ?? "");
  const [sender, setSender] = useState(restored.current?.sender ?? "");
  const [type, setType] = useState(restored.current?.type ?? "");
  const [direction, setDirection] = useState<"" | "received" | "sent">(restored.current?.direction ?? "");
  const [scope, setScope] = useState(restored.current?.scope ?? "");
  const [after, setAfter] = useState(restored.current?.after ?? "");
  const [before, setBefore] = useState(restored.current?.before ?? "");
  const [starred, setStarred] = useState(restored.current?.starred ?? false);
  const loaded = useRef(files.length);
  loaded.current = files.length;
  const [load, setLoad] = useState<FileLoad>(() => ({ offset: 0, count: Math.max(PAGE_SIZE, loaded.current), attempt: 0 }));
  const reload = useCallback(() => setLoad((current) => ({ offset: 0, count: Math.max(PAGE_SIZE, loaded.current), attempt: current.attempt+1 })), []);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const rows = useRef(new Map<string, HTMLButtonElement>());
  const scoped = accountId ? [accountId] : scope && accountIds.includes(scope) ? [scope] : accountIds;
  // Date inputs name days in the user's time zone.
  const queryKey = JSON.stringify({ accountIds: scoped, search, sender, type, direction: direction || undefined, starred, after: after ? localDayStart(after) : undefined, before: before ? localDayStart(before, 1) : undefined });
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
        if (!load.offset) setSelected((current) => page.some((file) => keyFor(file)===current) ? current : page[0] ? keyFor(page[0]) : null);
      }).catch((reason) => { if (active) setError(errorMessage(reason)); }).finally(() => { if (active) setLoading(false); });
    }, 150);
    return () => { active=false; window.clearTimeout(timer); };
  }, [queryKey, load]);
  const snapshot=useRef<FilesWorkspaceSnapshot | null>(null);
  snapshot.current={files,total,selected,search,sender,type,direction,scope,after,before,starred,scroll:0};
  useLayoutEffect(() => () => { if (sessions && snapshot.current) sessions.current.set(sessionKey,{...snapshot.current,scroll:main.current?.scrollTop ?? 0}); }, [sessions,sessionKey]);
  const scrollRestored=useRef(false);
  useLayoutEffect(() => { if (!loading && !scrollRestored.current && main.current) { main.current.scrollTop=restored.current?.scroll ?? 0; scrollRestored.current=true; } }, [loading]);
  const file = files.find((item) => keyFor(item)===selected);
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
        <div className="files-filters">
          <label>Search files<input type="search" value={search} onChange={(event) => setSearch(event.target.value)} placeholder="Filename" /></label>
          <label>Sender<input value={sender} onChange={(event) => setSender(event.target.value)} /></label>
          <label>File type<select value={type} onChange={(event) => setType(event.target.value)}><option value="">All types</option>{["document","spreadsheet","presentation","image","audio","video","archive","calendar"].map((kind) => <option key={kind} value={kind}>{kind[0].toUpperCase()+kind.slice(1)}</option>)}</select></label>
          <label>Direction<select value={direction} onChange={(event) => setDirection(event.target.value as typeof direction)}><option value="">Received and sent</option><option value="received">Received</option><option value="sent">Sent</option></select></label>
          {!accountId ? <label>Account<select value={scope} onChange={(event) => setScope(event.target.value)}><option value="">All accounts</option>{accountIds.map((email) => <option key={email}>{email}</option>)}</select></label> : null}
          <label>From date<input type="date" value={after} onChange={(event) => setAfter(event.target.value)} /></label>
          <label>Through date<input type="date" value={before} onChange={(event) => setBefore(event.target.value)} /></label>
        </div>
        <p className="files-coverage">Attachments from mail synchronized on this device, including archived conversations. Stars are stored on this device.</p>
        {error ? <p className="form-error" role="alert">{error}<button type="button" className="btn" onClick={() => setLoad((current) => ({ ...current, attempt: current.attempt+1 }))}>Retry</button></p> : null}
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
              <AttachmentIcon filename={item.attachment.filename} mimeType={item.attachment.mimeType} size="sm" /><span><strong>{item.attachment.filename}</strong><small>{item.sender} · {formatHistoryDate(item.sentAt)} · {formatAttachmentSize(item.attachment.size)} · {item.accountId}</small><small>{availability(item)}</small></span>
            </button>
            <FileStarButton accountId={item.accountId} messageId={item.messageId} attachmentId={item.attachment.id} filename={item.attachment.filename} starred={item.starred} />
          </div>)}
        </div>
        {loading ? <p className="context-status" role="status">Loading files…</p> : !files.length ? <p className="context-status">{starred ? "No starred files yet." : "No files match these filters."}</p> : null}
        {files.length<total ? <button type="button" className="btn" disabled={loading} onClick={() => setLoad((current) => ({ offset: files.length, count: PAGE_SIZE, attempt: current.attempt+1 }))}>Load more files</button> : null}
      </div>
      <WorkspaceContextRail label="File context" onReturn={() => rows.current.get(selected ?? "")?.focus()}>
        <ContextSectionHeader title="File details" />
        {file ? <>
          <h2 className="file-detail-name">{file.attachment.filename}</h2>
          <p>{formatAttachmentSize(file.attachment.size)} · {file.attachment.mimeType}</p><p>{availability(file)}</p>
          <div className="file-detail-actions"><button type="button" className="btn" onClick={() => action(mailClient.openAttachment(file.messageId,file.attachment.id))}><ExternalLink size={ICON_SIZE.sm} />Open</button><button type="button" className="btn" onClick={() => action(mailClient.saveAttachment(file.messageId,file.attachment.id))}><Download size={ICON_SIZE.sm} />Download</button><FileStarButton accountId={file.accountId} messageId={file.messageId} attachmentId={file.attachment.id} filename={file.attachment.filename} starred={file.starred} /></div>
          <ContextSection id="file-source" title="Source conversation" rows={[<ContextRow key="source" icon={<MessageSquareText size={ICON_SIZE.sm} />} title={file.subject} detail={`${file.sender} · ${file.accountId}`} date={formatHistoryDate(file.sentAt)} onActivate={() => onOpenThread(file.threadId)} />]} />
        </> : <p className="context-status">Select a file to see its details and source email.</p>}
      </WorkspaceContextRail>
    </div>
  </section>;
}
