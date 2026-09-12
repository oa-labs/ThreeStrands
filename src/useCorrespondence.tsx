import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { X } from "lucide-react";
import { Composer, type ComposerHandle } from "./Composer";
import { mailClient } from "./data/client";
import type { ComposeMode, Draft, OutboxItem } from "./correspondence";
import type { Account } from "./domain";
import { useEscapeDismiss } from "./useEscapeDismiss";

export function useCorrespondence(accounts: Account[], sourceId?: string, sourceAccountId?: string) {
  const [active, setActive] = useState<Draft | null>(null);
  const [view, setView] = useState<"drafts" | "outbox" | null>(null);
  const [drafts, setDrafts] = useState<Draft[]>([]);
  const [outbox, setOutbox] = useState<OutboxItem[]>([]);
  const [error, setError] = useState("");
  const [closing, setClosing] = useState(false);
  const editor = useRef<ComposerHandle>(null);
  const opening = useRef(false);
  const quitting = useRef(false);
  const [clock, setClock] = useState(Date.now());
  const refreshing = useRef(false);
  const refresh = useCallback(async () => {
    if (refreshing.current) return;
    refreshing.current = true;
    try { const [d, o] = await Promise.all([mailClient.listDrafts(), mailClient.listOutbox()]); setDrafts(d); setOutbox(o); }
    finally { refreshing.current = false; }
  }, []);
  useEffect(() => { void refresh().catch((e) => setError(String(e))); const timer = setInterval(() => { setClock(Date.now()); void refresh().catch(() => {}); }, 1000); return () => clearInterval(timer); }, [refresh]);
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
  const start = useCallback(async (mode: ComposeMode) => {
    if (opening.current) return;
    opening.current = true;
    try {
      await editor.current?.flush();
      const d = mode === "new"
        ? await mailClient.createDraft(mode)
        : await mailClient.createDraft(mode, sourceId, sourceAccountId);
      setActive(d); setView(null); setError("");
    }
    catch (e) { setError(String(e)); }
    finally { opening.current = false; }
  }, [sourceId, sourceAccountId]);
  const show = useCallback(async (next: "drafts" | "outbox") => { try { await editor.current?.flush(); setActive(null); await refresh(); setView(next); } catch (e) { setError(String(e)); } }, [refresh]);
  const undo = useCallback(async (id?: string) => {
    const target = id ?? outbox.find((o) => ["undo_pending", "ready"].includes(o.state))?.id;
    if (!target) return;
    try { await editor.current?.flush(); const d = await mailClient.cancelSend(target); setActive(d); setView(null); await refresh(); }
    catch (e) { setError(String(e)); }
  }, [outbox, refresh]);
  useEffect(() => { if (closing) document.querySelector<HTMLElement>(".exit-notice")?.focus(); }, [closing]);
  const pending = outbox.find((o) => ["undo_pending", "ready"].includes(o.state));
  const context = {
    closing,
    composerActive: Boolean(active),
    compose: () => { void start("new"); }, reply: () => { void start("reply"); }, replyAll: () => { void start("replyAll"); }, forward: () => { void start("forward"); },
    openInbox: () => { setActive(null); setView(null); },
    openDrafts: () => { void show("drafts"); }, openOutbox: () => { void show("outbox"); },
    sendDraft: () => editor.current?.send(), attachFiles: () => editor.current?.attach(),
    undoSend: () => { void undo(); }, canUndoSend: Boolean(pending),
  };
  return { context, sentCount: outbox.filter((o) => o.state === "sent").length, draftCount: drafts.length, outboxCount: outbox.filter((o) => !["sent", "canceled"].includes(o.state)).length, overlay: <>
    {active && <Composer key={active.id} ref={editor} draft={active} accounts={accounts} onClose={() => { setActive(null); void refresh(); }} onQueued={() => { setActive(null); void refresh(); }} />}
    {view && <CorrespondenceList title={view === "drafts" ? "Drafts" : "Outbox"} onClose={() => setView(null)}>
      {view === "drafts" ? <>{drafts.length === 0 && <p className="empty">No saved drafts.</p>}{drafts.map((d) => <button className="draft-row" key={d.id} onClick={() => { setActive(d); setView(null); }}><strong>{d.subject || "(no subject)"}</strong><span>{d.to || "No recipients"}</span><small>{new Date(d.updatedAt).toLocaleString()} · Saved on this device</small></button>)}</> : <>{outbox.filter((o) => o.state !== "canceled").length === 0 && <p className="empty">No outgoing messages.</p>}{outbox.filter((o) => o.state !== "canceled").map((o) => <article className="outbox-row" key={o.id}><strong>{o.draft.subject || "(no subject)"}</strong><span>From {o.draft.account} · To {o.draft.to || o.draft.cc || "Bcc recipients"}</span><small>{o.state === "undo_pending" ? (o.deadline > clock ? `Undo available · ${Math.ceil((o.deadline - clock) / 1000)}s` : "Waiting for connection") : o.state}</small>{o.error && <p>{o.error}</p>}{["undo_pending", "ready"].includes(o.state) && <button onClick={() => void undo(o.id)}>Undo send</button>}{o.state === "failed" && <button onClick={() => { void mailClient.recoverSend(o.id).then((d) => { setActive(d); setView(null); void refresh(); }).catch((e) => setError(String(e))); }}>Restore draft</button>}{o.state === "uncertain" && <button onClick={() => { void mailClient.reconcileSend(o.id).then(refresh).catch((e) => setError(String(e))); }}>Check sent mail</button>}</article>)}</>}
    </CorrespondenceList>}
    {pending && !active && !view && <div className="send-notice" role="status">{pending.deadline > clock ? `Sending in ${Math.ceil((pending.deadline - clock) / 1000)}s` : "Queued for delivery"}<button onClick={() => void undo(pending.id)}>Undo send</button><button onClick={() => void show("outbox")}>Outbox</button></div>}
    {closing && <div className="exit-backdrop"><div className="exit-notice" tabIndex={-1} role="dialog" aria-modal="true" aria-label="Closing Dispatch" onKeyDown={(e) => { e.stopPropagation(); if (e.key === "Tab") { e.preventDefault(); e.currentTarget.querySelector("button")?.focus(); } }}><span>Saving drafts and finishing pending delivery before closing… Queued mail remains saved for the next launch.</span>{pending && <button onClick={() => void undo(pending.id)}>Undo queued send</button>}</div></div>}
    {error && <div className="compose-notice" role="alert">{error}<button aria-label="Dismiss compose error" onClick={() => setError("")}><X size={16} /></button></div>}
  </> };
}

function CorrespondenceList({ title, onClose, children }: { title: string; onClose(): void; children: React.ReactNode }) {
  const panel = useRef<HTMLDivElement>(null);
  useEffect(() => { const previous = document.activeElement as HTMLElement; panel.current?.querySelector("button")?.focus(); return () => previous?.focus(); }, []);
  useEscapeDismiss(onClose);
  return <div className="compose-backdrop"><div className="correspondence-list" role="dialog" aria-modal="true" aria-label={title} ref={panel} onKeyDown={(e) => {
    if (e.key === "Tab") { const buttons = Array.from(panel.current?.querySelectorAll("button") ?? []); if (e.shiftKey && document.activeElement === buttons[0]) { e.preventDefault(); buttons.at(-1)?.focus(); } else if (!e.shiftKey && document.activeElement === buttons.at(-1)) { e.preventDefault(); buttons[0]?.focus(); } }
  }}><header><h2>{title}</h2><button className="icon-button" aria-label={`Close ${title}`} onClick={onClose}><X size={19} /></button></header><div>{children}</div></div></div>;
}
