import { nextUndoOrder } from "./undoOrder";
import { type SignatureState } from "./signatures";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode, type RefObject } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Square, SquareCheckBig, X } from "lucide-react";
import { Composer, type ComposerHandle } from "./Composer";
import type { DraftReviewAvailability } from "./DraftReviewSection";
import { mailClient } from "./data/client";
import type { ComposeMode, Draft, OutboxItem, ScheduledSendReport } from "./correspondence";
import type { Account, Snippet } from "./domain";
import { logBackgroundFailure } from "./errors";
import { draftWithSelectedQuote } from "./selectedMessageQuote";
import { sanitizeComposeHtml } from "./richText";
import { ICON_SIZE } from "./iconSizes";
import type { MessageAppearance } from "./SafeMessage";
import type { MailtoRequest } from "./mailtoLink";
import { ScheduleSendPicker, formatScheduledTime } from "./ScheduleSendPicker";
import { formatDisplayName, parseAddress, splitAddressList } from "./emailAddress";
import { formatMailTimestamp } from "./threadPresentation";
import { outboxBatchAbility, outboxEditAsNewSource, outboxEntryTime, selectedOutboxEntry, type OutboxEntry } from "./outboxView";
import { selectionGestureFor, type SelectionGesture } from "./threadSelection";

/**
 * Plain, honest label for an outbox item's raw state string. Only the states
 * whose raw word was itself shown to the user are remapped; every other state
 * keeps its existing rendering (handled by the caller), so existing copy is
 * unchanged. `unverifiable` keeps its explanatory `error` paragraph alongside
 * this short label.
 */
export function outboxStateLabel(state: OutboxItem["state"]): string {
  switch (state) {
    case "unverifiable": return "May have been sent";
    case "uncertain": return "Checking delivery";
    case "failed": return "Not sent";
    default: return state;
  }
}

type ComposeOptions = {
  availabilityText?: string;
  selectedQuote?: string;
  replyAssistInstruction?: string;
  followUpTaskId?: string;
  prefill?: MailtoRequest;
  /** Edit as New only: the account that sent the source, when no conversation is selected (the Outbox). */
  sourceAccount?: string;
};

/**
 * "Edit as New" copies a message the user sent. The composer only edits
 * sanitized HTML, so the copy saves exactly what it shows, and the inline
 * images its body displays are fetched before it opens.
 */
async function draftFromSent(messageId: string | undefined, account: string | undefined): Promise<Draft> {
  if (!messageId || !account) throw new Error("Select a message you sent");
  let draft = await mailClient.createDraftFromSent(messageId, account);
  if (draft.bodyHtml?.trim()) {
    draft = await mailClient.saveDraft({ ...draft, bodyHtml: sanitizeComposeHtml(draft.bodyHtml) });
  }
  for (const attachment of draft.attachments.filter((candidate) => candidate.inline && !candidate.ready)) {
    // The copy still opens offline; the composer retries the download.
    try { draft = await mailClient.fetchAttachment(draft.id, attachment.id); }
    catch (reason) { logBackgroundFailure("Edit as New image download")(reason); }
  }
  return draft;
}

function sameDraft(left: Draft, right: Draft): boolean {
  // Persisted revisions cover body, headers, attachments, and account changes.
  return left.id === right.id && left.revision === right.revision && left.updatedAt === right.updatedAt;
}

function sameOutbox(left: OutboxItem, right: OutboxItem): boolean {
  return left.id === right.id && sameDraft(left.draft, right.draft)
    && left.state === right.state && left.deadline === right.deadline
    && left.error === right.error && left.providerId === right.providerId
    && JSON.stringify(left.schedule) === JSON.stringify(right.schedule);
}

function retainUnchanged<T>(current: T[], next: T[], equal: (left: T, right: T) => boolean): T[] {
  return current.length === next.length && current.every((item, index) => equal(item, next[index])) ? current : next;
}

function sameSummary(left: ScheduledSendReport, right: ScheduledSendReport): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

function useCorrespondenceList<T>(equal: (left: T, right: T) => boolean) {
  const [items, setItems] = useState<T[]>([]);
  const latest = useRef(items);
  const publish = useCallback((update: T[] | ((current: T[]) => T[])) => {
    const next = typeof update === "function" ? update(latest.current) : update;
    const retained = retainUnchanged(latest.current, next, equal);
    if (retained === latest.current) return;
    latest.current = retained;
    // Compare before calling React: even an updater returning the same array
    // can cause an extra render after a previous state change.
    setItems(retained);
  }, [equal]);
  return [items, publish] as const;
}

export function useCorrespondence(
  accounts: Account[],
  sourceId: string | undefined,
  sourceAccountId: string | undefined,
  snippets: Snippet[],
  onCreateSnippet: (name: string, body: string) => Promise<Snippet>,
  onUpdateSnippet: (id: string, name: string, body: string) => Promise<Snippet>,
  onDeleteSnippet: (id: string) => Promise<void>,
  selectedThreadId: string | null,
  messageAppearance?: MessageAppearance,
  signatures?: SignatureState,
) {
  const [active, setActive] = useState<Draft | null>(null);
  // Bumped to reopen the composer on a version another device saved.
  const [activeVersion, setActiveVersion] = useState(0);
  const [selectedQuoteDraftId, setSelectedQuoteDraftId] = useState<string | null>(null);
  const [activeThreadId, setActiveThreadId] = useState<string | null>(null);
  const [activeAvailabilityText, setActiveAvailabilityText] = useState<string | null>(null);
  const [activeReplyAssistInstruction, setActiveReplyAssistInstruction] = useState<string | null>(null);
  const [activeFollowUpTaskId, setActiveFollowUpTaskId] = useState<string | null>(null);
  // The open draft as the composer last reported it, for the context panel.
  const [liveDraft, setLiveDraft] = useState<Draft | null>(null);
  const [drafts, setDrafts] = useCorrespondenceList(sameDraft);
  const [outbox, setOutbox] = useCorrespondenceList(sameOutbox);
  const sendUndoOrders = useRef(new Map<string, number>());
  const [summaries, setSummaries] = useCorrespondenceList(sameSummary);
  const [error, setError] = useState("");
  const [closing, setClosing] = useState(false);
  const editor = useRef<ComposerHandle>(null);
  const opening = useRef(false);
  const quitting = useRef(false);
  const [clock, setClock] = useState(Date.now());
  const [pendingOutboxActions, setPendingOutboxActions] = useState<ReadonlySet<string>>(new Set());
  const pendingOutboxActionsRef = useRef<ReadonlySet<string>>(new Set());
  const refreshing = useRef<Promise<void> | null>(null);
  const refresh = useCallback(function refresh(includeDrafts = true): Promise<void> {
    // An explicit refresh after editing/sending must not be dropped behind a poll.
    if (refreshing.current) return includeDrafts
      ? refreshing.current.then(() => refresh(), () => refresh())
      : refreshing.current;
    const pending = Promise.all([
      includeDrafts ? mailClient.listDrafts() : Promise.resolve(null),
      mailClient.listOutbox(),
      mailClient.listScheduledSummaries?.() ?? Promise.resolve([]),
    ]).then(([d, o, reports]) => {
      if (d) setDrafts(d);
      setOutbox(o);
      setSummaries(reports);
    }).finally(() => { refreshing.current = null; });
    refreshing.current = pending;
    return pending;
  }, [setDrafts, setOutbox, setSummaries]);
  useEffect(() => { void refresh().catch((e) => setError(String(e))); }, [refresh]);
  // Drafts arrive from the account's server drafts; each pull that changed
  // them says so.
  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    let listening = true;
    let stop: (() => void) | undefined;
    void listen("drafts-changed", () => void refresh().catch(logBackgroundFailure("Draft sync refresh")))
      .then((unlisten) => { if (listening) stop = unlisten; else unlisten(); })
      .catch(logBackgroundFailure("Draft sync listener"));
    return () => { listening = false; stop?.(); };
  }, [refresh]);
  const activeId = active?.id;
  useEffect(() => {
    if (activeId) editor.current?.remoteChanged(drafts.find((draft) => draft.id === activeId) ?? null);
  }, [activeId, drafts]);
  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    const listener = listen("compose-before-exit", async () => {
      if (quitting.current) return;
      quitting.current = true; setClosing(true);
      try { await editor.current?.prepareExit(); await invoke("finish_exit"); }
      catch (e) { setError(`Could not close safely: ${String(e)}`); quitting.current = false; setClosing(false); }
    });
    return () => { void listener.then((unlisten) => unlisten()); };
  }, []);
  const start = useCallback(async (mode: ComposeMode | "editAsNew", messageId?: string, options?: ComposeOptions) => {
    if (opening.current) return;
    opening.current = true;
    try {
      await editor.current?.flush();
      const created = mode === "new"
        ? await mailClient.createDraft(mode)
        : mode === "editAsNew"
          ? await draftFromSent(messageId ?? sourceId, options?.sourceAccount ?? sourceAccountId)
          : await mailClient.createDraft(mode, messageId ?? sourceId, sourceAccountId);
      const quoted = options?.selectedQuote ? draftWithSelectedQuote(created, options.selectedQuote) : null;
      const filled = quoted ?? (options?.prefill ? { ...created, ...options.prefill } : null);
      const prepared = filled ? await mailClient.saveDraft(filled) : created;
      const d = options?.followUpTaskId
        ? await mailClient.saveDraft({ ...prepared, followUpTaskId: options.followUpTaskId })
        : prepared;
      setSelectedQuoteDraftId(quoted ? d.id : null);
      setActiveAvailabilityText(options?.availabilityText ?? null);
      setActiveReplyAssistInstruction(options?.replyAssistInstruction ?? null);
      setActiveFollowUpTaskId(options?.followUpTaskId ?? null);
      setActiveThreadId(mode === "new" || mode === "editAsNew" ? null : selectedThreadId);
      setActive(d); setError("");
    }
    catch (e) { setError(String(e)); }
    finally { opening.current = false; }
  }, [sourceId, sourceAccountId, selectedThreadId]);
  useEffect(() => {
    if (active && activeThreadId && selectedThreadId !== activeThreadId) editor.current?.close();
  }, [active, activeThreadId, selectedThreadId]);
  // Saves pending edits before the composer unmounts, so leaving a draft for
  // another folder never drops what was typed since the last autosave.
  const closeComposer = useCallback(async () => {
    await editor.current?.flush();
    setActive(null); setActiveThreadId(null); setActiveAvailabilityText(null); setActiveReplyAssistInstruction(null); setActiveFollowUpTaskId(null);
  }, []);
  // Called when navigating to the inline Drafts/Outbox view: makes sure
  // whatever was being edited is saved and the lists are current.
  const openList = useCallback(async () => {
    try { await closeComposer(); await refresh(); }
    catch (e) { setError(String(e)); }
  }, [closeComposer, refresh]);
  const withOutboxActionGuard = useCallback((id: string, action: () => Promise<void>) => {
    if (pendingOutboxActionsRef.current.has(id)) return;
    const pending = new Set(pendingOutboxActionsRef.current);
    pending.add(id);
    pendingOutboxActionsRef.current = pending;
    setPendingOutboxActions(pending);
    void action().finally(() => {
      setPendingOutboxActions((current) => {
        if (!current.has(id)) return current;
        const next = new Set(current);
        next.delete(id);
        pendingOutboxActionsRef.current = next;
        return next;
      });
    });
  }, []);
  const undo = useCallback((id?: string) => {
    const target = id ?? outbox.find((o) => ["undo_pending", "ready"].includes(o.state))?.id;
    if (!target) return;
    withOutboxActionGuard(target, async () => {
      try {
        await editor.current?.flush();
        const d = await mailClient.cancelSend(target);
        setActiveFollowUpTaskId(d.followUpTaskId ?? null);
        setActiveThreadId(null);
        setActive(d);
        await refresh();
      }
      catch (e) { setError(String(e)); }
    });
  }, [outbox, refresh, withOutboxActionGuard]);
  const restoreFailedSend = useCallback((id: string) => {
    if (pendingOutboxActionsRef.current.has(id)) return;
    withOutboxActionGuard(id, () =>
      mailClient.recoverSend(id).then((d) => { setActiveFollowUpTaskId(d.followUpTaskId ?? null); setActiveThreadId(null); setActive(d); return refresh(); }).catch((e: unknown) => setError(String(e))),
    );
  }, [refresh, withOutboxActionGuard]);
  /** Runs `action` on each send, then reports how many failed in one message. */
  const runOnSends = useCallback(async (ids: readonly string[], verb: string, action: (id: string) => Promise<unknown>) => {
    const results = await Promise.allSettled(ids.map(action));
    const failed = results.filter((result): result is PromiseRejectedResult => result.status === "rejected");
    if (failed.length) setError(`${failed.length} ${failed.length === 1 ? "message" : "messages"} couldn't be ${verb}: ${String(failed[0].reason)}`);
    await refresh().catch((e: unknown) => setError(String(e)));
  }, [refresh]);
  /** Returns sends that haven't gone out to Drafts, without opening each in the composer. */
  const returnSendsToDrafts = useCallback(
    (ids: readonly string[]) => runOnSends(ids, "returned to Drafts", (id) => mailClient.cancelSend(id)),
    [runOnSends],
  );
  /** Discards failed sends: each is restored to a draft, then discarded. */
  const discardFailedSends = useCallback(
    (ids: readonly string[]) => runOnSends(ids, "discarded", async (id) => mailClient.discardDraft((await mailClient.recoverSend(id)).id)),
    [runOnSends],
  );
  const reconcileSend = useCallback((id: string) => {
    if (pendingOutboxActionsRef.current.has(id)) return;
    withOutboxActionGuard(id, () => mailClient.reconcileSend(id).then(() => refresh()).catch((e: unknown) => setError(String(e))));
  }, [refresh, withOutboxActionGuard]);
  useEffect(() => { if (closing) document.querySelector<HTMLElement>(".exit-notice")?.focus(); }, [closing]);
  const pending = outbox.find((o) => ["undo_pending", "ready"].includes(o.state));
  const pendingId = pending?.id ?? null;
  const pendingUndoOrder = useMemo(() => {
    // The list is newest first. Remember every pending send so canceling the
    // newest one cannot make an older send appear newer than a task or mail action.
    for (const item of [...outbox].reverse()) {
      if (["undo_pending", "ready"].includes(item.state) && !sendUndoOrders.current.has(item.id)) sendUndoOrders.current.set(item.id, nextUndoOrder());
    }
    return pendingId ? sendUndoOrders.current.get(pendingId) ?? 0 : 0;
  }, [outbox, pendingId]);
  const hasActiveDelivery = outbox.some((o) => ["undo_pending", "ready", "uncertain", "sending"].includes(o.state));
  useEffect(() => {
    const timer = window.setInterval(() => {
      // Only an immediate send has a live countdown. Idle and future-scheduled
      // messages do not need a clock update that re-renders the entire app.
      if (pendingId) setClock(Date.now());
      void refresh(false).catch(logBackgroundFailure("Outbox refresh"));
    }, hasActiveDelivery ? 1000 : 5000);
    return () => window.clearInterval(timer);
  }, [hasActiveDelivery, pendingId, refresh]);

  const compose = useCallback(() => { void start("new"); }, [start]);
  // Links can arrive together (several opened while the app launched), so
  // each waits for the previous draft to open instead of being dropped by
  // the one-at-a-time guard in start.
  const mailtoQueue = useRef(Promise.resolve());
  const composeMailto = useCallback((request: MailtoRequest) => {
    mailtoQueue.current = mailtoQueue.current.then(() => start("new", undefined, { prefill: request }));
  }, [start]);
  const reply = useCallback((messageId?: string, selectedQuote?: string) => { void start("reply", messageId, { selectedQuote }); }, [start]);
  const replyAll = useCallback((messageId?: string, selectedQuote?: string) => { void start("replyAll", messageId, { selectedQuote }); }, [start]);
  const replyWithAvailability = useCallback((text: string, messageId?: string) => {
    void start("reply", messageId, { availabilityText: text });
  }, [start]);
  /** Opens a reply with `text` as its starting body, for the user to review. */
  const replyWithText = useCallback((text: string, messageId?: string) => {
    void start("reply", messageId, { availabilityText: text });
  }, [start]);
  const replyWithFollowUp = useCallback((messageId: string, instruction: string, taskId?: string) => {
    void start("reply", messageId, {
      replyAssistInstruction: instruction,
      followUpTaskId: taskId,
    });
  }, [start]);
  const forward = useCallback((messageId?: string, selectedQuote?: string) => { void start("forward", messageId, { selectedQuote }); }, [start]);
  const editAsNew = useCallback((messageId: string, sourceAccount?: string) => { void start("editAsNew", messageId, { sourceAccount }); }, [start]);
  const openInbox = useCallback(() => {
    void closeComposer().catch((e: unknown) => setError(String(e)));
  }, [closeComposer]);
  const openDrafts = useCallback(() => { void openList(); }, [openList]);
  const openOutbox = useCallback(() => { void openList(); }, [openList]);
  const scheduleDraft = useCallback(() => editor.current?.schedule(), []);
  const sendDraft = useCallback(() => editor.current?.send(), []);
  const sendDraftAndThen = useCallback((action: () => void, archiveOnSend?: boolean) => editor.current?.send(action, archiveOnSend), []);
  const attachFiles = useCallback(() => editor.current?.attach(), []);
  const draftReplyWithAI = useCallback(() => editor.current?.draftReplyWithAI(), []);
  const discardDraft = useCallback(() => editor.current?.discard(), []);
  const undoSend = useCallback(() => { void undo(); }, [undo]);
  const composerActive = Boolean(active);
  const flushDraft = useCallback(async () => { await editor.current?.flush(); }, []);
  const insertIntoDraft = useCallback((text: string) => editor.current?.insertText(text), []);
  const replaceDraftRecipient = useCallback((from: string, to: string) => editor.current?.replaceRecipient(from, to), []);
  const switchDraftAccount = useCallback((email: string) => editor.current?.switchAccount(email), []);
  const moveDraftRecipientsToBcc = useCallback((emails: string[]) => editor.current?.moveRecipientsToBcc(emails), []);
  const focusDraftBody = useCallback(() => editor.current?.focusBody(), []);

  const context = useMemo(() => ({
    closing,
    composerActive,
    compose, reply, replyAll, forward, editAsNew, openInbox, openDrafts, openOutbox,
    scheduleDraft, sendDraft, sendDraftAndThen, attachFiles, discardDraft, draftReplyWithAI, undoSend, canUndoSend: pendingId !== null,
  }), [scheduleDraft, attachFiles, closing, compose, composerActive, discardDraft, draftReplyWithAI, editAsNew, forward, openDrafts, openInbox, openOutbox, reply, replyAll, sendDraft, sendDraftAndThen, undoSend, pendingId]);
  const openDraft = useCallback((draft: Draft) => {
    setSelectedQuoteDraftId(null);
    setActiveFollowUpTaskId(draft.followUpTaskId ?? null);
    setActiveThreadId(null);
    setActive(draft);
  }, []);
  const undoSendItem = useCallback((id: string) => { void undo(id); }, [undo]);
  // Discards a draft from the Drafts list. The open draft goes through the
  // composer so pending edits are flushed before it is removed.
  const discardListedDraft = useCallback((draft: Draft) => {
    if (active?.id === draft.id) { editor.current?.discard(); return; }
    void mailClient.discardDraft(draft.id).then(() => refresh()).catch((e: unknown) => setError(String(e)));
  }, [active?.id, refresh]);
  /** Discards every listed draft in `ids`; an open one closes its composer. */
  const discardListedDrafts = useCallback(async (ids: readonly string[]) => {
    if (active && ids.includes(active.id)) editor.current?.discard();
    const results = await Promise.allSettled(ids.filter((id) => id !== active?.id).map((id) => mailClient.discardDraft(id)));
    const failed = results.filter((result) => result.status === "rejected");
    if (failed.length) setError(`${failed.length} ${failed.length === 1 ? "draft" : "drafts"} couldn't be discarded: ${String((failed[0] as PromiseRejectedResult).reason)}`);
    await refresh().catch((e: unknown) => setError(String(e)));
  }, [active, refresh]);
  const renderComposer = (onSendAndArchive?: () => void, review?: DraftReviewAvailability) => active ? (
    <Composer
      key={`${active.id}:${activeVersion}`}
      ref={editor}
      draft={active}
      expandQuotedText={active.id === selectedQuoteDraftId}
      accounts={accounts}
      messageAppearance={messageAppearance}
      signatures={signatures}
      snippets={snippets}
      onCreateSnippet={onCreateSnippet}
      onUpdateSnippet={onUpdateSnippet}
      onDeleteSnippet={onDeleteSnippet}
      availabilityText={activeAvailabilityText}
      replyAssistInstruction={activeReplyAssistInstruction}
      onDraftChange={setLiveDraft}
      onRemoteVersion={(next) => { setActive(next); setActiveVersion((version) => version + 1); }}
      onSendAndArchive={onSendAndArchive}
      review={review}
      onClose={() => { setActive(null); setActiveThreadId(null); setActiveAvailabilityText(null); setActiveReplyAssistInstruction(null); setActiveFollowUpTaskId(null); void refresh(); }}
      onQueued={(item) => {
        const followUpTaskId = activeFollowUpTaskId ?? active.followUpTaskId ?? null;
        setActive(null);
        setActiveThreadId(null);
        setActiveAvailabilityText(null);
        setActiveReplyAssistInstruction(null);
        setActiveFollowUpTaskId(null);
        // queueDraft already returned the authoritative queued item. Publish
        // it immediately instead of waiting for a second listOutbox roundtrip
        // so an open conversation can render the reply optimistically.
        setOutbox((current) => [item, ...current.filter((entry) => entry.id !== item.id)]);
        void refresh();
        if (followUpTaskId && !item.schedule) {
          void mailClient.recordFollowUp(followUpTaskId).catch((reason) => setError(String(reason)));
        }
      }}
    />
  ) : null;
  // Peer schedules this computer also owns are shown once, as the local send.
  const peerSummaries = useMemo(
    () => summaries.filter((s) => !outbox.some((o) => o.schedule?.report.ownerInstallationId === s.ownerInstallationId && o.id === s.operationId)),
    [outbox, summaries],
  );
  return {
    context,
    pendingUndoOrder,
    composeMailto,
    replyWithAvailability,
    replyWithText,
    replyWithFollowUp,
    drafts,
    outbox,
    summaries: peerSummaries,
    refresh,
    clock,
    sentCount: outbox.filter((o) => o.state === "sent").length,
    draftCount: drafts.length,
    outboxCount: outbox.filter((o) => !["sent", "canceled"].includes(o.state)).length + summaries.filter((s) => !["sent", "canceled"].includes(s.state) && !outbox.some((o) => o.id === s.operationId && o.schedule?.report.ownerInstallationId === s.ownerInstallationId)).length,
    openDraft,
    discardListedDraft,
    discardListedDrafts,
    undoSendItem,
    restoreFailedSend,
    returnSendsToDrafts,
    discardFailedSends,
    reconcileSend,
    pendingOutboxActions,
    activeDraft: active,
    liveDraft: active ? (liveDraft?.id === active.id ? liveDraft : active) : null,
    flushDraft,
    insertIntoDraft,
    replaceDraftRecipient,
    switchDraftAccount,
    moveDraftRecipientsToBcc,
    focusDraftBody,
    renderComposer,
    overlay: <>
      {pending && !active && <div className="send-notice" role="status">{pending.deadline > clock ? `Sending in ${Math.ceil((pending.deadline - clock) / 1000)}s` : "Queued for delivery"}<button className="btn-link" title="Undo Send (z or ⌘/Ctrl+Z outside editing fields)" onClick={() => void undo(pending.id)}>Undo Send</button></div>}
      {closing && <div className="exit-backdrop"><div className="exit-notice" tabIndex={-1} role="dialog" aria-modal="true" aria-label="Closing ThreeStrands" onKeyDown={(e) => { e.stopPropagation(); if (e.key === "Tab") { e.preventDefault(); e.currentTarget.querySelector("button")?.focus(); } }}><span>Saving drafts and finishing pending delivery before closing… Queued mail remains saved for the next launch.</span>{pending && <button className="btn" onClick={() => void undo(pending.id)}>Undo Queued Send</button>}</div></div>}
      {error && <div className="compose-notice" role="alert">{error}<button className="btn-icon btn-icon-sm" aria-label="Dismiss Compose Error" onClick={() => setError("")}><X size={ICON_SIZE.sm} /></button></div>}
    </>,
  };
}

function outboxRecipients(draft: Draft): string {
  const recipients = splitAddressList([draft.to, draft.cc].filter(Boolean).join(", "));
  if (recipients.length === 0) return "Bcc recipients";
  return recipients.map((recipient) => formatDisplayName(parseAddress(recipient).name)).join(", ");
}

function ownerName(report: ScheduledSendReport): string {
  return report.ownerName ?? report.ownerNameAtCreation;
}

/** The short delivery status a row shows, and whether it needs the user's attention. */
function outboxRowStatus(entry: OutboxEntry, clock: number): { text: string; problem: boolean } {
  if (entry.kind === "peer") {
    const { report } = entry;
    return report.state === "scheduled" && report.scheduledAt <= clock
      ? { text: `Awaiting an update from ${ownerName(report)}`, problem: true }
      : { text: `Sends from ${ownerName(report)}`, problem: false };
  }
  const { item } = entry;
  if (item.schedule?.report.blockedReason) return { text: "Paused", problem: true };
  if (item.state === "overdue") return { text: "Scheduled time missed", problem: true };
  return {
    text: outboxUndoText(item, clock) ?? outboxStateLabel(item.state),
    problem: ["failed", "uncertain", "unverifiable"].includes(item.state) || item.sentCopy?.state === "abandoned",
  };
}

function outboxUndoText(item: OutboxItem, clock: number): string | null {
  if (item.state !== "undo_pending") return null;
  return item.deadline > clock ? `Undo available · ${Math.ceil((item.deadline - clock) / 1000)}s` : "Waiting for connection";
}

/** The Outbox's message list. Selecting a row shows that send in the reader, like a conversation. */
export function OutboxList({
  entries,
  selectedKey,
  clock,
  onSelect,
  selectedRowRef,
  checkedKeys = new Set(),
  onToggleCheck,
  onSelectionGesture,
}: {
  entries: OutboxEntry[];
  selectedKey: string | null;
  clock: number;
  onSelect(key: string): void;
  selectedRowRef?: RefObject<HTMLButtonElement | null>;
  checkedKeys?: ReadonlySet<string>;
  onToggleCheck?(key: string): void;
  onSelectionGesture?(key: string, gesture: SelectionGesture): void;
}) {
  if (entries.length === 0) return <p className="empty">No outgoing messages.</p>;
  const selected = selectedOutboxEntry(entries, selectedKey)?.key;
  return <>{entries.map((entry) => {
    const status = outboxRowStatus(entry, clock);
    const isSelected = entry.key === selected;
    const ability = outboxBatchAbility(entry);
    const checkable = ability.returnable || ability.discardable;
    const checked = checkedKeys.has(entry.key);
    return (
      <button
        key={entry.key}
        ref={isSelected ? selectedRowRef : undefined}
        type="button"
        role="option"
        aria-selected={isSelected}
        className={`thread-row outbox-row${isSelected ? " selected" : ""}`}
        onMouseDown={(event) => {
          // Keep Shift-click from extending a text selection across rows.
          if (event.shiftKey && checkable) event.preventDefault();
        }}
        onClick={(event) => {
          const gesture = checkable && onSelectionGesture ? selectionGestureFor(event) : null;
          if (gesture) onSelectionGesture!(entry.key, gesture);
          else onSelect(entry.key);
        }}
      >
        {/* Every row keeps the checkbox column, so text lines up with other folders. */}
        <span className="row-leading">
          {checkable && onToggleCheck ? (
            <span
              className={`row-check ${checked ? "checked" : ""}`}
              aria-hidden="true"
              onClick={(event) => {
                event.stopPropagation();
                // Shift-clicking a checkbox adds every row since the last one checked.
                if (event.shiftKey && onSelectionGesture) onSelectionGesture(entry.key, "range-add");
                else onToggleCheck(entry.key);
              }}
            >
              {checked ? <SquareCheckBig size={ICON_SIZE.md} /> : <Square size={ICON_SIZE.md} />}
            </span>
          ) : null}
        </span>
        {checked ? <span className="sr-only">Selected for batch actions</span> : null}
        <span className="unread-dot" />
        <span className="thread-content">
          <span className="thread-meta">
            <span className="thread-sender">
              <strong>{entry.kind === "local" ? `To ${outboxRecipients(entry.item.draft)}` : `From ${entry.report.account}`}</strong>
            </span>
            <span className="thread-meta-trailing">
              <time>{formatMailTimestamp(new Date(outboxEntryTime(entry)).toISOString())}</time>
            </span>
          </span>
          <span className="thread-subject">{(entry.kind === "local" ? entry.item.draft.subject : entry.report.subject) || "(no subject)"}</span>
          <span className={`thread-snippet outbox-row-status${status.problem ? " problem" : ""}`}>{status.text}</span>
        </span>
      </button>
    );
  })}</>;
}

/**
 * The reader for one Outbox entry: its delivery status and the actions that
 * apply to it, above the message itself (`children`). A peer device's
 * schedule is read-only and has no content here.
 */
export function OutboxDetail({
  entry,
  clock,
  onUndo,
  onRestore,
  onReconcile,
  onEditAsNew,
  pendingActions,
  onChanged,
  children,
}: {
  entry: OutboxEntry;
  clock: number;
  onUndo(id: string): void;
  onRestore(id: string): void;
  onReconcile(id: string): void;
  onEditAsNew?(messageId: string, account: string): void;
  pendingActions?: ReadonlySet<string>;
  onChanged?(): Promise<void>;
  children?: ReactNode;
}) {
  const [rescheduling, setRescheduling] = useState<OutboxItem | null>(null);
  const [actionError, setActionError] = useState("");
  const actions = useRef(new Set<string>());
  const runScheduleAction = async (item: OutboxItem) => {
    if (actions.current.has(item.id)) return;
    actions.current.add(item.id); setActionError("");
    try { await mailClient.sendScheduledNow(item.id,item.schedule!.report.reportRevision); await onChanged?.(); }
    catch (e) { setActionError(String(e)); }
    finally { actions.current.delete(item.id); }
  };

  if (entry.kind === "peer") {
    const s = entry.report;
    return (
      <div className="outbox-detail" role="region" aria-label="Outgoing message">
        <header className="reader-header">
          <div>
            <span className="reader-account-scope">From {s.account}</span>
            <div className="subject-row"><h2>{s.subject || "(no subject)"}</h2></div>
            <div className="outbox-detail-status">
              <small>{formatScheduledTime(s.scheduledAt,s.timeZone)} · Sends from {ownerName(s)}</small>
              {s.timeZone !== Intl.DateTimeFormat().resolvedOptions().timeZone && <small>On this computer: {new Date(s.scheduledAt).toLocaleString()}</small>}
              <p className="outbox-detail-problem">{s.state === "scheduled" && s.scheduledAt <= clock ? `Scheduled time passed; awaiting an update from ${ownerName(s)}` : `Last reported status: ${s.state}`}</p>
              <small>Report changed {new Date(s.statusChangedAt).toLocaleString()}{s.lastContactAt ? ` · Last contact ${new Date(s.lastContactAt).toLocaleString()}` : ""}</small>
            </div>
          </div>
        </header>
        <div className="reader-empty">
          <p>The message stays on {ownerName(s)}.</p>
          <p>Manage on {ownerName(s)}</p>
        </div>
      </div>
    );
  }

  const o = entry.item;
  const busy = pendingActions?.has(o.id) ?? false;
  const canManage = !o.schedule || o.schedule.canManage;
  const editSource = outboxEditAsNewSource(entry);
  return (
    <div className="outbox-detail" role="region" aria-label="Outgoing message">
      {rescheduling?.schedule && <ScheduleSendPicker initialAt={rescheduling.schedule.report.scheduledAt} initialZone={rescheduling.schedule.report.timeZone} onClose={() => setRescheduling(null)} onSchedule={async (selection) => {
        await mailClient.rescheduleSend(rescheduling.id,rescheduling.schedule!.report.reportRevision,selection); setRescheduling(null); await onChanged?.();
      }} />}
      <header className="reader-header">
        <div>
          <span className="reader-account-scope">From {o.draft.from || o.draft.account}</span>
          <div className="subject-row"><h2>{o.draft.subject || "(no subject)"}</h2></div>
          <div className="outbox-detail-status">
            {o.schedule && <>
              <small>{formatScheduledTime(o.schedule.report.scheduledAt,o.schedule.report.timeZone)} · Sends from {o.schedule.report.ownerNameAtCreation}</small>
              {o.schedule.report.blockedReason && <p className="outbox-detail-problem">Paused: {o.schedule.report.blockedReason === "owner_unavailable" ? "the sending-device identity is unavailable on this computer" : "reconnect the sending account"}.</p>}
              <small>{o.schedule.visibility === "local" ? "Saved on this computer" : o.schedule.visibility === "published" ? "Published to sync storage" : o.schedule.visibility === "paused" ? "Visibility sync paused" : "Waiting to sync visibility"}</small>
              {o.state === "overdue" && <p className="outbox-detail-problem">Scheduled time missed. Confirm sending or reschedule on the sending computer.</p>}
            </>}
            <small>{outboxUndoText(o, clock) ?? outboxStateLabel(o.state)}</small>
            {o.error && <p className="outbox-detail-problem">{o.error}</p>}
            {o.sentCopy?.state === "abandoned" && <p className="outbox-detail-problem outbox-sent-copy-notice">{o.sentCopy.error ?? "Your message was sent, but a copy couldn't be saved to your Sent folder."}</p>}
            {actionError && <p className="outbox-detail-problem" role="alert">{actionError}</p>}
          </div>
          <div className="outbox-detail-actions">
            {o.schedule?.canManage && ["scheduled","overdue"].includes(o.state) && <>
              <button className="btn btn-sm" onClick={() => setRescheduling(o)} disabled={busy}>Reschedule</button>
              <button className="btn btn-sm" onClick={() => onUndo(o.id)} disabled={busy}>Return to draft</button>
              {o.state === "overdue" && <button className="btn btn-sm" onClick={() => void runScheduleAction(o)} disabled={busy}>Send now</button>}
            </>}
            {canManage && ["undo_pending", "ready"].includes(o.state) && <button className="btn btn-sm" onClick={() => onUndo(o.id)}>Undo Send</button>}
            {canManage && o.state === "failed" && <button className="btn btn-sm" disabled={busy} onClick={() => onRestore(o.id)}>Restore Draft</button>}
            {canManage && o.state === "uncertain" && <button className="btn btn-sm" disabled={busy} onClick={() => onReconcile(o.id)}>Check Sent Mail</button>}
            {editSource && onEditAsNew && <button className="btn btn-sm" onClick={() => onEditAsNew(editSource.messageId, editSource.account)}>Edit as New</button>}
          </div>
        </div>
      </header>
      {children}
    </div>
  );
}
