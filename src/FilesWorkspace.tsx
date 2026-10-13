import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
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

export type FilesWorkspaceSnapshot = { files: WorkspaceFile[]; total: number; selected: string | null; search: string; sender: string; type: string; direction: "" | "received" | "sent"; scope: string; after: string; before: string; starred: boolean; offset: number; scroll: number };
const PAGE_SIZE = 50;
const keyFor = (file: WorkspaceFile) => JSON.stringify([file.accountId, file.messageId, file.attachment.id]);
const availability = (file: WorkspaceFile) => file.availability==="offline" ? "Available offline" : file.availability==="download" ? "Download required" : "Availability not yet known";

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
  const [offset, setOffset] = useState(restored.current?.offset ?? 0);
  const [revision, setRevision] = useState(0);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const rows = useRef(new Map<string, HTMLButtonElement>());
  const scoped = accountId ? [accountId] : scope && accountIds.includes(scope) ? [scope] : accountIds;
  const queryKey = JSON.stringify({ accountIds: scoped, search, sender, type, direction: direction || undefined, starred, after: after ? `${after}T00:00:00Z` : undefined, before: before ? new Date(new Date(`${before}T00:00:00Z`).getTime()+86_400_000).toISOString() : undefined, limit: PAGE_SIZE });
  const lastQuery = useRef(queryKey);
  if (lastQuery.current!==queryKey) { lastQuery.current=queryKey; setOffset(0); setFiles([]); setSelected(null); }
  useEffect(() => {
    const changed=() => { setOffset(0); setRevision((value) => value+1); };
    window.addEventListener(FILES_CHANGED, changed);
    return () => window.removeEventListener(FILES_CHANGED, changed);
  }, []);
  useEffect(() => {
    let active = true;
    setLoading(true); setError(null);
    const timer = window.setTimeout(() => {
      void mailClient.listFiles({ ...JSON.parse(queryKey), offset }).then((page) => {
        if (!active) return;
        setFiles((previous) => offset ? [...previous,...page.files.filter((file) => !previous.some((existing) => keyFor(existing)===keyFor(file)))] : page.files);
        setTotal(page.total);
        if (!offset) setSelected((current) => page.files.some((file) => keyFor(file)===current) ? current : page.files[0] ? keyFor(page.files[0]) : null);
      }).catch((reason) => { if (active) setError(errorMessage(reason)); }).finally(() => { if (active) setLoading(false); });
    }, 150);
    return () => { active=false; window.clearTimeout(timer); };
  }, [queryKey, offset, revision, refreshKey]);
  const snapshot=useRef<FilesWorkspaceSnapshot | null>(null);
  snapshot.current={files,total,selected,search,sender,type,direction,scope,after,before,starred,offset,scroll:0};
  useLayoutEffect(() => () => { if (sessions && snapshot.current) sessions.current.set(sessionKey,{...snapshot.current,scroll:main.current?.scrollTop ?? 0}); }, [sessions,sessionKey]);
  const scrollRestored=useRef(false);
  useLayoutEffect(() => { if (!loading && !scrollRestored.current && main.current) { main.current.scrollTop=restored.current?.scroll ?? 0; scrollRestored.current=true; } }, [loading]);
  const file = files.find((item) => keyFor(item)===selected);
  const action = (operation: Promise<void>) => { void operation.catch((reason) => setError(errorMessage(reason))); };
  return <section className="files-workspace" aria-label="Files" onKeyDown={(event) => {
    if (event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey || isEditableTarget(event.target)) return;
    if (event.key==="Enter" && !(event.target instanceof HTMLElement && event.target.closest(".workspace-context-rail"))) { event.preventDefault(); window.dispatchEvent(new CustomEvent("threestrands:focus-context", { detail: "File context" })); return; }
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
        {error ? <p className="form-error" role="alert">{error}<button type="button" className="btn" onClick={() => setRevision((value) => value+1)}>Retry</button></p> : null}
        <div className="files-list" role="list" aria-label="Mail attachments">
          {files.map((item) => <div key={keyFor(item)} role="listitem" className={`file-list-item${selected===keyFor(item) ? " selected" : ""}`}>
            <button type="button" className="file-list-select" aria-current={selected===keyFor(item) ? "true" : undefined}
              ref={(node) => { if (node) rows.current.set(keyFor(item),node); else rows.current.delete(keyFor(item)); }}
              onClick={() => setSelected(keyFor(item))} onDoubleClick={() => action(mailClient.openAttachment(item.messageId,item.attachment.id))}>
              <AttachmentIcon filename={item.attachment.filename} mimeType={item.attachment.mimeType} size="sm" /><span><strong>{item.attachment.filename}</strong><small>{item.sender} · {formatHistoryDate(item.sentAt)} · {formatAttachmentSize(item.attachment.size)} · {item.accountId}</small><small>{availability(item)}</small></span>
            </button>
            <FileStarButton accountId={item.accountId} messageId={item.messageId} attachmentId={item.attachment.id} threadId={item.threadId} filename={item.attachment.filename} starred={item.starred} />
          </div>)}
        </div>
        {loading ? <p className="context-status" role="status">Loading files…</p> : !files.length ? <p className="context-status">{starred ? "No starred files yet." : "No files match these filters."}</p> : null}
        {files.length<total ? <button type="button" className="btn" disabled={loading} onClick={() => setOffset(files.length)}>Load more files</button> : null}
      </div>
      <WorkspaceContextRail label="File context" onReturn={() => rows.current.get(selected ?? "")?.focus()}>
        <ContextSectionHeader title="File details" />
        {file ? <>
          <h2 className="file-detail-name">{file.attachment.filename}</h2>
          <p>{formatAttachmentSize(file.attachment.size)} · {file.attachment.mimeType}</p><p>{availability(file)}</p>
          <div className="file-detail-actions"><button type="button" className="btn" onClick={() => action(mailClient.openAttachment(file.messageId,file.attachment.id))}><ExternalLink size={ICON_SIZE.sm} />Open</button><button type="button" className="btn" onClick={() => action(mailClient.saveAttachment(file.messageId,file.attachment.id))}><Download size={ICON_SIZE.sm} />Download</button><FileStarButton accountId={file.accountId} messageId={file.messageId} attachmentId={file.attachment.id} threadId={file.threadId} filename={file.attachment.filename} starred={file.starred} /></div>
          <ContextSection id="file-source" title="Source conversation" rows={[<ContextRow key="source" icon={<MessageSquareText size={ICON_SIZE.sm} />} title={file.subject} detail={`${file.sender} · ${file.accountId}`} date={formatHistoryDate(file.sentAt)} onActivate={() => onOpenThread(file.threadId)} />]} />
        </> : <p className="context-status">Select a file to see its details and source email.</p>}
      </WorkspaceContextRail>
    </div>
  </section>;
}
