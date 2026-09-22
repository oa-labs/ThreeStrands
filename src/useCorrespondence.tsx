import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { X } from "lucide-react";
import { Composer, type ComposerHandle } from "./Composer";
import { mailClient } from "./data/client";
import type { ComposeMode, Draft, OutboxItem } from "./correspondence";
import type { Account, Snippet } from "./domain";

type ComposeOptions = {
  availabilityText?: string;
  replyAssistInstruction?: string;
  followUpTaskId?: string;
};

export function useCorrespondence(
  accounts: Account[],
  sourceId: string | undefined,
  sourceAccountId: string | undefined,
  snippets: Snippet[],
  onCreateSnippet: (name: string, body: string) => Promise<Snippet>,
  onUpdateSnippet: (id: string, name: string, body: string) => Promise<Snippet>,
  onDeleteSnippet: (id: string) => Promise<void>,
) {
  const [active, setActive] = useState<Draft | null>(null);
  const [activeAvailabilityText, setActiveAvailabilityText] = useState<string | null>(null);
  const [activeReplyAssistInstruction, setActiveReplyAssistInstruction] = useState<string | null>(null);
  const [activeFollowUpTaskId, setActiveFollowUpTaskId] = useState<string | null>(null);
  const [drafts, setDrafts] = useState<Draft[]>([]);
  const [outbox, setOutbox] = useState<OutboxItem[]>([]);
  const [error, setError] = useState("");
  const [closing, setClosing] = useState(false);
  const editor = useRef<ComposerHandle>(null);
  const opening = useRef(false);
  const quitting = useRef(false);
  const [clock, setClock] = useState(Date.now());
  const [pendingOutboxActions, setPendingOutboxActions] = useState<ReadonlySet<string>>(new Set());
  const pendingOutboxActionsRef = useRef<ReadonlySet<string>>(new Set());
  const refreshing = useRef(false);
  const refresh = useCallback(async () => {
    if (refreshing.current) return;
    refreshing.current = true;
    try { const [d, o] = await Promise.all([mailClient.listDrafts(), mailClient.listOutbox()]); setDrafts(d); setOutbox(o); }
    finally { refreshing.current = false; }
  }, []);
  useEffect(() => { void refresh().catch((e) => setError(String(e))); }, [refresh]);
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
  const start = useCallback(async (mode: ComposeMode, messageId?: string, options?: ComposeOptions) => {
    if (opening.current) return;
    opening.current = true;
    try {
      await editor.current?.flush();
      const created = mode === "new"
        ? await mailClient.createDraft(mode)
        : await mailClient.createDraft(mode, messageId ?? sourceId, sourceAccountId);
      const d = options?.followUpTaskId
        ? await mailClient.saveDraft({ ...created, followUpTaskId: options.followUpTaskId })
        : created;
      setActiveAvailabilityText(options?.availabilityText ?? null);
      setActiveReplyAssistInstruction(options?.replyAssistInstruction ?? null);
      setActiveFollowUpTaskId(options?.followUpTaskId ?? null);
      setActive(d); setError("");
    }
    catch (e) { setError(String(e)); }
    finally { opening.current = false; }
  }, [sourceId, sourceAccountId]);
  // Called when navigating to the inline Drafts/Outbox view: makes sure
  // whatever was being edited is saved and the lists are current.
  const openList = useCallback(async () => {
    try { await editor.current?.flush(); setActive(null); setActiveAvailabilityText(null); setActiveReplyAssistInstruction(null); setActiveFollowUpTaskId(null); await refresh(); }
    catch (e) { setError(String(e)); }
  }, [refresh]);
  const undo = useCallback(async (id?: string) => {
    const target = id ?? outbox.find((o) => ["undo_pending", "ready"].includes(o.state))?.id;
    if (!target) return;
    try {
      await editor.current?.flush();
      const d = await mailClient.cancelSend(target);
      setActiveFollowUpTaskId(d.followUpTaskId ?? null);
      setActive(d);
      await refresh();
    }
    catch (e) { setError(String(e)); }
  }, [outbox, refresh]);
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
  const restoreFailedSend = useCallback((id: string) => {
    if (pendingOutboxActionsRef.current.has(id)) return;
    withOutboxActionGuard(id, () =>
      mailClient.recoverSend(id).then((d) => { setActiveFollowUpTaskId(d.followUpTaskId ?? null); setActive(d); return refresh(); }).catch((e: unknown) => setError(String(e))),
    );
  }, [refresh, withOutboxActionGuard]);
  const reconcileSend = useCallback((id: string) => {
    if (pendingOutboxActionsRef.current.has(id)) return;
    withOutboxActionGuard(id, () => mailClient.reconcileSend(id).then(refresh).catch((e: unknown) => setError(String(e))));
  }, [refresh, withOutboxActionGuard]);
  useEffect(() => { if (closing) document.querySelector<HTMLElement>(".exit-notice")?.focus(); }, [closing]);
  const pending = outbox.find((o) => ["undo_pending", "ready"].includes(o.state));
  const pendingId = pending?.id ?? null;
  const hasActiveDelivery = outbox.some((o) => ["undo_pending", "ready", "sending"].includes(o.state));
  useEffect(() => {
    if (!hasActiveDelivery) return;
    const timer = window.setInterval(() => {
      setClock(Date.now());
      void refresh().catch(() => {});
    }, 1000);
    return () => window.clearInterval(timer);
  }, [hasActiveDelivery, refresh]);

  const compose = useCallback(() => { void start("new"); }, [start]);
  const reply = useCallback((messageId?: string) => { void start("reply", messageId); }, [start]);
  const replyAll = useCallback((messageId?: string) => { void start("replyAll", messageId); }, [start]);
  const replyWithAvailability = useCallback((text: string, messageId?: string) => {
    void start("reply", messageId, { availabilityText: text });
  }, [start]);
  const replyWithFollowUp = useCallback((messageId: string, instruction: string, taskId?: string) => {
    void start("reply", messageId, {
      replyAssistInstruction: instruction,
      followUpTaskId: taskId,
    });
  }, [start]);
  const forward = useCallback((messageId?: string) => { void start("forward", messageId); }, [start]);
  const openInbox = useCallback(() => {
    setActive(null);
    setActiveAvailabilityText(null);
    setActiveReplyAssistInstruction(null);
    setActiveFollowUpTaskId(null);
  }, []);
  const openDrafts = useCallback(() => { void openList(); }, [openList]);
  const openOutbox = useCallback(() => { void openList(); }, [openList]);
  const sendDraft = useCallback(() => editor.current?.send(), []);
  const sendDraftAndThen = useCallback((action: () => void, archiveOnSend?: boolean) => editor.current?.send(action, archiveOnSend), []);
  const attachFiles = useCallback(() => editor.current?.attach(), []);
  const draftReplyWithAI = useCallback(() => editor.current?.draftReplyWithAI(), []);
  const undoSend = useCallback(() => { void undo(); }, [undo]);
  const composerActive = Boolean(active);

  const context = useMemo(() => ({
    closing,
    composerActive,
    compose, reply, replyAll, forward, openInbox, openDrafts, openOutbox,
    sendDraft, sendDraftAndThen, attachFiles, draftReplyWithAI, undoSend, canUndoSend: pendingId !== null,
  }), [attachFiles, closing, compose, composerActive, draftReplyWithAI, forward, openDrafts, openInbox, openOutbox, reply, replyAll, sendDraft, sendDraftAndThen, undoSend, pendingId]);
  const openDraft = useCallback((draft: Draft) => {
    setActiveFollowUpTaskId(draft.followUpTaskId ?? null);
    setActive(draft);
  }, []);
  const undoSendItem = useCallback((id: string) => { void undo(id); }, [undo]);
  const composer = active ? (
    <Composer
      key={active.id}
      ref={editor}
      draft={active}
      accounts={accounts}
      snippets={snippets}
      onCreateSnippet={onCreateSnippet}
      onUpdateSnippet={onUpdateSnippet}
      onDeleteSnippet={onDeleteSnippet}
      availabilityText={activeAvailabilityText}
      replyAssistInstruction={activeReplyAssistInstruction}
      onClose={() => { setActive(null); setActiveAvailabilityText(null); setActiveReplyAssistInstruction(null); setActiveFollowUpTaskId(null); void refresh(); }}
      onQueued={(item) => {
        const followUpTaskId = activeFollowUpTaskId ?? active.followUpTaskId ?? null;
        setActive(null);
        setActiveAvailabilityText(null);
        setActiveReplyAssistInstruction(null);
        setActiveFollowUpTaskId(null);
        // queueDraft already returned the authoritative queued item. Publish
        // it immediately instead of waiting for a second listOutbox roundtrip
        // so an open conversation can render the reply optimistically.
        setOutbox((current) => [item, ...current.filter((entry) => entry.id !== item.id)]);
        void refresh();
        if (followUpTaskId) {
          void mailClient.recordFollowUp(followUpTaskId).catch((reason) => setError(String(reason)));
        }
      }}
    />
  ) : null;
  return {
    context,
    replyWithAvailability,
    replyWithFollowUp,
    drafts,
    outbox,
    clock,
    sentCount: outbox.filter((o) => o.state === "sent").length,
    draftCount: drafts.length,
    outboxCount: outbox.filter((o) => !["sent", "canceled"].includes(o.state)).length,
    openDraft,
    undoSendItem,
    restoreFailedSend,
    reconcileSend,
    pendingOutboxActions,
    activeDraft: active,
    composer,
    overlay: <>
      {pending && !active && <div className="send-notice" role="status">{pending.deadline > clock ? `Sending in ${Math.ceil((pending.deadline - clock) / 1000)}s` : "Queued for delivery"}<button onClick={() => void undo(pending.id)}>Undo Send</button></div>}
      {closing && <div className="exit-backdrop"><div className="exit-notice" tabIndex={-1} role="dialog" aria-modal="true" aria-label="Closing ThreeStrands" onKeyDown={(e) => { e.stopPropagation(); if (e.key === "Tab") { e.preventDefault(); e.currentTarget.querySelector("button")?.focus(); } }}><span>Saving drafts and finishing pending delivery before closing… Queued mail remains saved for the next launch.</span>{pending && <button onClick={() => void undo(pending.id)}>Undo Queued Send</button>}</div></div>}
      {error && <div className="compose-notice" role="alert">{error}<button aria-label="Dismiss Compose Error" onClick={() => setError("")}><X size={16} /></button></div>}
    </>,
  };
}

export function DraftsList({ drafts, onOpen }: { drafts: Draft[]; onOpen(draft: Draft): void }) {
  if (drafts.length === 0) return <p className="empty">No saved drafts.</p>;
  return <>{drafts.map((d) => (
    <button className="draft-row" key={d.id} onClick={() => onOpen(d)}>
      <strong>{d.subject || "(no subject)"}</strong>
      <span>{d.to || "No recipients"}</span>
      <small>{new Date(d.updatedAt).toLocaleString()} · Saved on this device</small>
    </button>
  ))}</>;
}

export function OutboxList({
  outbox,
  clock,
  onUndo,
  onRestore,
  onReconcile,
  pendingActions,
}: {
  outbox: OutboxItem[];
  clock: number;
  onUndo(id: string): void;
  onRestore(id: string): void;
  onReconcile(id: string): void;
  pendingActions?: ReadonlySet<string>;
}) {
  const visible = outbox.filter((o) => o.state !== "canceled");
  if (visible.length === 0) return <p className="empty">No outgoing messages.</p>;
  return <>{visible.map((o) => {
    const busy = pendingActions?.has(o.id) ?? false;
    return (
    <article className="outbox-row" key={o.id}>
      <strong>{o.draft.subject || "(no subject)"}</strong>
      <span>From {o.draft.account} · To {o.draft.to || o.draft.cc || "Bcc recipients"}</span>
      <small>{o.state === "undo_pending" ? (o.deadline > clock ? `Undo available · ${Math.ceil((o.deadline - clock) / 1000)}s` : "Waiting for connection") : o.state}</small>
      {o.error && <p>{o.error}</p>}
      {["undo_pending", "ready"].includes(o.state) && <button onClick={() => onUndo(o.id)}>Undo Send</button>}
      {o.state === "failed" && <button disabled={busy} onClick={() => onRestore(o.id)}>Restore Draft</button>}
      {o.state === "uncertain" && <button disabled={busy} onClick={() => onReconcile(o.id)}>Check Sent Mail</button>}
    </article>
    );
  })}</>;
}
