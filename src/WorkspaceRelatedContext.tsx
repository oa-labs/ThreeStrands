import { useEffect, useState } from "react";
import { MessageSquareText } from "lucide-react";
import { mailClient } from "./data/client";
import type { WorkspaceContextTarget, WorkspaceConversation, WorkspaceFile } from "./domain";
import { ContextRow, ContextSection } from "./ContextSections";
import { AttachmentIcon } from "./AttachmentIcon";
import { FileStarButton, FILES_CHANGED } from "./FileStarButton";
import { CONTEXT_POLICY, contextQuery, rankContext } from "./workspaceContextPolicy";
import { errorMessage } from "./errors";
import { formatHistoryDate } from "./contactContext";
import { formatAttachmentSize } from "./threadPresentation";
import { ICON_SIZE } from "./iconSizes";
import { attachmentMeetingTitle } from "./invitationTitle";

export function WorkspaceFileRow({ file, onOpenThread }: { file: WorkspaceFile; onOpenThread(id: string): void }) {
  const [error, setError] = useState<string | null>(null);
  return <>
    <ContextRow icon={<AttachmentIcon filename={file.attachment.filename} mimeType={file.attachment.mimeType} size="sm" />}
      title={attachmentMeetingTitle(file) ?? file.attachment.filename} date={formatHistoryDate(file.sentAt)} detail={`${file.reason ? `${file.reason} · ` : ""}${formatAttachmentSize(file.attachment.size)} · ${file.accountId}`}
      onActivate={() => { void mailClient.openAttachment(file.messageId, file.attachment.id).catch((reason) => setError(errorMessage(reason))); }}
      actions={[
        <button key="show" type="button" className="btn-icon btn-icon-sm" aria-label={`Show the email with ${file.attachment.filename}`} onClick={() => onOpenThread(file.threadId)}><MessageSquareText size={ICON_SIZE.sm} /></button>,
        <FileStarButton key="star" accountId={file.accountId} messageId={file.messageId} attachmentId={file.attachment.id} filename={file.attachment.filename} starred={file.starred} />,
      ]} />
    {error ? <p className="form-error" role="alert">{error}</p> : null}
  </>;
}

export function WorkspaceRelatedContext({ target, accountIds, onOpenThread }: { target: WorkspaceContextTarget; accountIds: string[]; onOpenThread(id: string): void }) {
  const [conversations, setConversations] = useState<WorkspaceConversation[]>([]);
  const [files, setFiles] = useState<WorkspaceFile[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [fileError, setFileError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [revision, setRevision] = useState(0);
  const requestKey = JSON.stringify(contextQuery(target, accountIds));
  const sourceMessage = target.kind === "task" ? target.task.sourceMessageId : undefined;
  useEffect(() => {
    const changed = () => setRevision((value) => value + 1);
    window.addEventListener(FILES_CHANGED, changed);
    return () => window.removeEventListener(FILES_CHANGED, changed);
  }, []);
  useEffect(() => {
    let active = true;
    setConversations([]); setFiles([]); setError(null); setFileError(null); setLoading(true);
    const timer = window.setTimeout(() => {
      const query = JSON.parse(requestKey) as ReturnType<typeof contextQuery>;
      void mailClient.contextCandidates(query).then(async (candidates) => {
        const related = rankContext(candidates, query);
        if (!active) return;
        setConversations(related);
        try {
          const page = await mailClient.listFiles({ accountIds: query.accountIds, threadIds: related.map((item) => item.threadId), priorityMessageId: sourceMessage ?? undefined, prioritizeStarred: true, limit: CONTEXT_POLICY.files });
          if (active) setFiles(page.files.map((file) => ({ ...file, reason: related.find((item) => item.threadId===file.threadId)?.reason }))
            .sort((a,b) => Number(b.messageId===sourceMessage)-Number(a.messageId===sourceMessage) || Number(b.starred)-Number(a.starred) || b.sentAt.localeCompare(a.sentAt)));
        } catch (reason) { if (active) setFileError(errorMessage(reason)); }
      }).catch((reason) => { if (active) setError(errorMessage(reason)); }).finally(() => { if (active) setLoading(false); });
    }, CONTEXT_POLICY.debounceMs);
    return () => { active = false; window.clearTimeout(timer); };
  }, [requestKey, revision, sourceMessage]);
  return <>
    {loading ? <p className="context-status" role="status">Finding related information…</p> : null}
    {error ? <p className="form-error" role="alert">{error}<button type="button" className="btn-link" onClick={() => setRevision((value) => value + 1)}>Retry context</button></p> : null}
    {fileError ? <p className="form-error" role="alert">{fileError}<button type="button" className="btn-link" onClick={() => setRevision((value) => value + 1)}>Retry files</button></p> : null}
    {files.length ? <ContextSection id="workspace-files" title="Files" count={files.length} limit={CONTEXT_POLICY.rows} rows={files.map((file) => <WorkspaceFileRow key={`${file.messageId}:${file.attachment.id}`} file={file} onOpenThread={onOpenThread} />)} /> : null}
    {conversations.length ? <ContextSection id="workspace-conversations" title="Conversations" count={conversations.length} rows={conversations.map((item) => <ContextRow key={item.threadId} icon={<MessageSquareText size={ICON_SIZE.sm} />} title={item.subject} date={formatHistoryDate(item.sentAt)} detail={`${item.reason} · ${item.accountId}`} onActivate={() => onOpenThread(item.threadId)} />)} /> : null}
    {!loading && !error && !fileError && !conversations.length ? <p className="context-status">No related information found in synchronized mail.</p> : null}
  </>;
}
