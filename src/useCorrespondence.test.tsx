import { act, renderHook, waitFor, render, screen, fireEvent, cleanup } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Draft, OutboxItem, ScheduledSendReport } from "./correspondence";
import { mailClient } from "./data/client";
import type { Snippet } from "./domain";
import { OutboxDetail, OutboxList, outboxStateLabel, useCorrespondence } from "./useCorrespondence";
import { outboxEntries, type OutboxEntry } from "./outboxView";

const listenMock = vi.hoisted(() => vi.fn(async (_event: string, _handler: () => void) => () => {}));
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));

const draft: Draft = {
  id: "draft-1",
  revision: 2,
  account: "me@example.com",
  mode: "new",
  sourceId: null,
  threadId: null,
  replyId: null,
  references: [],
  to: "",
  cc: "",
  bcc: "",
  subject: "",
  body: "",
  attachments: [],
  updatedAt: 0,
};

const outbox: OutboxItem = {
  id: "outbox-1",
  draft,
  state: "undo_pending",
  deadline: Date.now() + 10_000,
  error: null,
};

const noSnippets: Snippet[] = [];
const noopCreateSnippet = vi.fn();
const noopUpdateSnippet = vi.fn();
const noopDeleteSnippet = vi.fn();
const snippetArgs = [noSnippets, noopCreateSnippet, noopUpdateSnippet, noopDeleteSnippet] as const;

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("useCorrespondence", () => {
  it.each(["idle", "scheduled"])("polls %s mail without reloading drafts or re-rendering unchanged state", async (state) => {
    vi.useFakeTimers();
    const listDrafts = vi.spyOn(mailClient, "listDrafts").mockResolvedValue([draft]);
    const items: OutboxItem[] = state === "idle" ? [] : [{ ...outbox, state: "scheduled", deadline: Date.now() + 60_000 }];
    const listOutbox = vi.spyOn(mailClient, "listOutbox").mockImplementation(async () => structuredClone(items));
    vi.spyOn(mailClient, "listScheduledSummaries").mockResolvedValue([]);
    let renders = 0;
    const { result } = renderHook(() => {
      renders++;
      return useCorrespondence([], undefined, undefined, ...snippetArgs, null);
    });
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    const stableOutbox = result.current.outbox;
    const clock = result.current.clock;
    const initialRenders = renders;
    await act(async () => { await vi.advanceTimersByTimeAsync(15_000); });
    expect(listDrafts).toHaveBeenCalledTimes(1);
    expect(listOutbox).toHaveBeenCalledTimes(4);
    expect(result.current.outbox).toBe(stableOutbox);
    expect(result.current.clock).toBe(clock);
    expect(renders).toBe(initialRenders);
  });

  it("keeps older pending sends in their original undo order when a newer send leaves the queue", async () => {
    vi.useFakeTimers();
    let items: OutboxItem[] = [{ ...outbox, id: "new-send" }, { ...outbox, id: "old-send" }];
    vi.spyOn(mailClient, "listDrafts").mockResolvedValue([]);
    vi.spyOn(mailClient, "listOutbox").mockImplementation(async () => items);
    vi.spyOn(mailClient, "listScheduledSummaries").mockResolvedValue([]);
    const { result } = renderHook(() => useCorrespondence([], undefined, undefined, ...snippetArgs, null));
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    const newerOrder = result.current.pendingUndoOrder;
    expect(newerOrder).toBeGreaterThan(0);
    items = [{ ...outbox, id: "old-send" }];
    await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
    expect(result.current.pendingUndoOrder).toBeGreaterThan(0);
    expect(result.current.pendingUndoOrder).toBeLessThan(newerOrder);
  });

  it("coalesces repeated Undo Send requests until cancellation finishes", async () => {
    vi.spyOn(mailClient, "listDrafts").mockResolvedValue([]);
    const listOutbox = vi.spyOn(mailClient, "listOutbox").mockResolvedValue([outbox]);
    vi.spyOn(mailClient, "listScheduledSummaries").mockResolvedValue([]);
    let finish!: (draft: Draft) => void;
    const cancel = vi.spyOn(mailClient, "cancelSend").mockImplementation(() => new Promise<Draft>((resolve) => { finish = resolve; }));
    const { result } = renderHook(() => useCorrespondence([], undefined, undefined, ...snippetArgs, null));
    await waitFor(() => expect(result.current.context.canUndoSend).toBe(true));
    act(() => { result.current.context.undoSend(); result.current.context.undoSend(); });
    await waitFor(() => expect(cancel).toHaveBeenCalledExactlyOnceWith(outbox.id));
    listOutbox.mockResolvedValue([]);
    await act(async () => { finish(draft); });
    expect(result.current.activeDraft?.id).toBe(draft.id);
    expect(result.current.context.canUndoSend).toBe(false);
  });

  it("keeps countdowns live and publishes delivery changes without polling draft bodies", async () => {
    vi.useFakeTimers();
    const listDrafts = vi.spyOn(mailClient, "listDrafts").mockResolvedValue([]);
    let current: OutboxItem = { ...outbox, deadline: Date.now() + 10_000 };
    vi.spyOn(mailClient, "listOutbox").mockImplementation(async () => [structuredClone(current)]);
    vi.spyOn(mailClient, "listScheduledSummaries").mockResolvedValue([]);
    const { result } = renderHook(() => useCorrespondence([], undefined, undefined, ...snippetArgs, null));
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    const clock = result.current.clock;
    await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
    expect(result.current.clock).toBe(clock + 1000);
    current = { ...current, state: "sent", providerId: "delivered" };
    await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
    expect(result.current.sentCount).toBe(1);
    expect(result.current.context.canUndoSend).toBe(false);
    expect(listDrafts).toHaveBeenCalledTimes(1);
  });

  it("publishes ownership and peer status changes even when message revisions are unchanged", async () => {
    vi.useFakeTimers();
    const report: ScheduledSendReport = {
      operationId: outbox.id, ownerInstallationId: "self", ownerSyncDeviceId: null,
      ownerNameAtCreation: "Desktop", account: draft.account, subject: draft.subject,
      scheduledAt: Date.now() + 60_000, timeZone: "UTC", state: "scheduled",
      blockedReason: null, reportRevision: 1, statusChangedAt: Date.now(),
    };
    let item: OutboxItem = { ...outbox, state: "scheduled", schedule: { report, canManage: true, visibility: "local" } };
    let peer: ScheduledSendReport = { ...report, operationId: "remote", ownerInstallationId: "peer", ownerName: "Laptop", lastContactAt: 1 };
    vi.spyOn(mailClient, "listDrafts").mockResolvedValue([]);
    vi.spyOn(mailClient, "listOutbox").mockImplementation(async () => [structuredClone(item)]);
    vi.spyOn(mailClient, "listScheduledSummaries").mockImplementation(async () => [structuredClone(peer)]);
    const { result } = renderHook(() => useCorrespondence([], undefined, undefined, ...snippetArgs, null));
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    item = { ...item, schedule: { report: { ...report, blockedReason: "owner_unavailable" }, canManage: false, visibility: "pending" } };
    peer = { ...peer, ownerName: "Renamed laptop", lastContactAt: 2 };
    await act(async () => { await vi.advanceTimersByTimeAsync(5000); });
    expect(result.current.outbox[0].schedule).toMatchObject({ canManage: false, visibility: "pending", report: { blockedReason: "owner_unavailable" } });
    expect(result.current.summaries[0]).toMatchObject({ ownerName: "Renamed laptop", lastContactAt: 2 });
  });

  it.each([false, true])("refreshes edited drafts after an outstanding poll (poll fails: %s)", async (fails) => {
    vi.useFakeTimers();
    const listDrafts = vi.spyOn(mailClient, "listDrafts").mockResolvedValue([]);
    const listOutbox = vi.spyOn(mailClient, "listOutbox").mockResolvedValue([]);
    vi.spyOn(mailClient, "listScheduledSummaries").mockResolvedValue([]);
    const { result } = renderHook(() => useCorrespondence([], undefined, undefined, ...snippetArgs, null));
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    let release!: (items: OutboxItem[]) => void;
    let reject!: (reason: Error) => void;
    listOutbox.mockReturnValueOnce(new Promise((resolve, fail) => { release = resolve; reject = fail; }));
    await act(async () => { await vi.advanceTimersByTimeAsync(5000); });
    listDrafts.mockResolvedValue([{ ...draft, revision: draft.revision + 1, body: "New edits" }]);
    const refreshed = result.current.refresh();
    await act(async () => {
      if (fails) reject(new Error("Temporary outbox read failure"));
      else release([]);
      await refreshed;
    });
    expect(listDrafts).toHaveBeenCalledTimes(2);
    expect(result.current.drafts[0].body).toBe("New edits");
    expect(result.current.drafts[0].revision).toBe(draft.revision + 1);
  });

  it("loads drafts and outbox and starts each compose mode through the client", async () => {
    vi.spyOn(mailClient, "listDrafts").mockResolvedValue([draft]);
    vi.spyOn(mailClient, "listOutbox").mockResolvedValue([]);
    const createDraft = vi.spyOn(mailClient, "createDraft").mockImplementation(async (mode, sourceId) => ({
      ...draft,
      mode,
      sourceId: sourceId ?? null,
    }));
    const { result } = renderHook(() => useCorrespondence([], "message-1", "account@example.com", ...snippetArgs, "thread-1"));

    await waitFor(() => expect(result.current.draftCount).toBe(1));
    act(() => result.current.context.compose());
    await waitFor(() => expect(result.current.activeDraft?.mode).toBe("new"));
    expect(createDraft).toHaveBeenCalledWith("new");

    act(() => result.current.context.openInbox());
    // Leaving the composer saves pending edits first, so it closes once that save settles.
    await waitFor(() => expect(result.current.activeDraft).toBeNull());
    act(() => result.current.context.reply());
    await waitFor(() => expect(createDraft).toHaveBeenLastCalledWith("reply", "message-1", "account@example.com"));
  });

  it("reloads drafts after each pull and reopens the open draft on a version saved elsewhere", async () => {
    let onSync: () => void = () => {};
    listenMock.mockImplementation(async (event: string, handler: () => void) => { if (event === "drafts-changed") onSync = handler; return () => {}; });
    Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
    try {
      const opened = { ...draft, subject: "Plans", body: "Started here" };
      const listDrafts = vi.spyOn(mailClient, "listDrafts").mockResolvedValue([opened]);
      vi.spyOn(mailClient, "listOutbox").mockResolvedValue([]);
      vi.spyOn(mailClient, "listScheduledSummaries").mockResolvedValue([]);
      vi.spyOn(mailClient, "listIdentities").mockResolvedValue([]);
      function Harness() {
        const state = useCorrespondence([], undefined, undefined, ...snippetArgs, null);
        return <>
          <button onClick={() => state.openDraft(opened)}>Open</button>
          {state.renderComposer()}
        </>;
      }
      render(<Harness />);
      await waitFor(() => expect(listenMock).toHaveBeenCalledWith("drafts-changed", expect.any(Function)));
      fireEvent.click(screen.getByRole("button", { name: "Open" }));
      expect(screen.getByRole("textbox", { name: "Subject" })).toHaveValue("Plans");

      listDrafts.mockResolvedValue([{ ...opened, revision: opened.revision + 1, subject: "Plans from the laptop", updatedAt: 5 }]);
      await act(async () => { onSync(); });
      await waitFor(() => expect(screen.getByRole("textbox", { name: "Subject" })).toHaveValue("Plans from the laptop"));
    } finally {
      Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
      cleanup();
    }
  });

  it.each(["reply", "replyAll"] as const)("starts a %s to the selected message with only the selected quote", async (mode) => {
    vi.spyOn(mailClient, "listDrafts").mockResolvedValue([]);
    vi.spyOn(mailClient, "listOutbox").mockResolvedValue([]);
    const createDraft = vi.spyOn(mailClient, "createDraft").mockResolvedValue({
      ...draft, revision: 0, mode, body: "\n\nOn Tuesday, Sender wrote:\n> Full message", bodyHtml: "",
    });
    const saveDraft = vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: 1 }));
    const { result } = renderHook(() => useCorrespondence([], "latest", "account@example.com", ...snippetArgs, "thread-1"));

    act(() => result.current.context[mode]("older", "One line\nAnother line"));
    await waitFor(() => expect(result.current.activeDraft?.revision).toBe(1));
    expect(createDraft).toHaveBeenCalledWith(mode, "older", "account@example.com");
    expect(saveDraft.mock.calls[0][0].body).toBe("\n\nOn Tuesday, Sender wrote:\n> One line\n> Another line");
  });

  it("starts a forward to the selected message with only the selected quote", async () => {
    vi.spyOn(mailClient, "listDrafts").mockResolvedValue([]);
    vi.spyOn(mailClient, "listOutbox").mockResolvedValue([]);
    const header = "---------- Forwarded message ----------\nFrom: Sender\nDate: Tuesday\nSubject: Subject\nTo: Me\n\n";
    const createDraft = vi.spyOn(mailClient, "createDraft").mockResolvedValue({
      ...draft, revision: 0, mode: "forward", body: "", forwardedContent: { html: "<p>Full message</p>", text: header + "Full message" },
    });
    const saveDraft = vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: 1 }));
    const { result } = renderHook(() => useCorrespondence([], "latest", "account@example.com", ...snippetArgs, "thread-1"));

    act(() => result.current.context.forward("older", "One line\nAnother line"));
    await waitFor(() => expect(result.current.activeDraft?.revision).toBe(1));
    expect(createDraft).toHaveBeenCalledWith("forward", "older", "account@example.com");
    expect(saveDraft.mock.calls[0][0].forwardedContent?.text).toBe(header + "> One line\n> Another line");
  });

  it("edits a sent message as new: saves only sanitized HTML and fetches its inline images first", async () => {
    vi.spyOn(mailClient, "listDrafts").mockResolvedValue([]);
    vi.spyOn(mailClient, "listOutbox").mockResolvedValue([]);
    const inline = { id: "map", name: "map.png", size: 3, mime: "image/png", ready: false, messageId: "sent", providerId: null, inline: true, contentId: "map@example.com" };
    const file = { ...inline, id: "menu", name: "menu.pdf", mime: "application/pdf", inline: false, contentId: null };
    const createDraftFromSent = vi.spyOn(mailClient, "createDraftFromSent").mockResolvedValue({
      ...draft, revision: 0, subject: "Dinner on Saturday", body: "Join us Saturday",
      bodyHtml: "<p onclick=\"steal()\">Join us <strong>Saturday</strong></p><script>steal()</script><img src=\"https://tracker.example/pixel.gif\"><img src=\"cid:map@example.com\"><a href=\"javascript:steal()\">RSVP</a>",
      attachments: [inline, file],
    });
    const saveDraft = vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    const fetchAttachment = vi.spyOn(mailClient, "fetchAttachment").mockImplementation(async (_id, attachmentId) => ({
      ...draft, revision: 2, attachments: [{ ...inline, ready: attachmentId === "map" }, file],
    }));
    const createDraft = vi.spyOn(mailClient, "createDraft");
    const { result } = renderHook(() => useCorrespondence([], "latest", "account@example.com", ...snippetArgs, "thread-1"));

    act(() => result.current.context.editAsNew("sent"));
    await waitFor(() => expect(result.current.activeDraft?.revision).toBe(2));
    expect(createDraftFromSent).toHaveBeenCalledWith("sent", "account@example.com");
    expect(createDraft).not.toHaveBeenCalled();
    const saved = saveDraft.mock.calls[0][0].bodyHtml ?? "";
    expect(saved).toContain("Join us <strong>Saturday</strong>");
    expect(saved).toContain('src="cid:map@example.com"');
    expect(saved).not.toMatch(/script|onclick|tracker\.example|javascript:/);
    // Only the inline image the body shows downloads before opening; the
    // composer downloads ordinary attachments once it is open.
    expect(fetchAttachment.mock.calls.map(([, attachmentId]) => attachmentId)).toEqual(["map"]);
    expect(result.current.activeDraft?.to).toBe("");
  });

  it("copies an Outbox send from the account that sent it when no conversation is selected", async () => {
    vi.spyOn(mailClient, "listDrafts").mockResolvedValue([]);
    vi.spyOn(mailClient, "listOutbox").mockResolvedValue([]);
    const createDraftFromSent = vi.spyOn(mailClient, "createDraftFromSent").mockResolvedValue({ ...draft, subject: "Dinner" });
    const { result } = renderHook(() => useCorrespondence([], undefined, undefined, ...snippetArgs, null));

    act(() => result.current.context.editAsNew("provider-sent", "me@example.com"));
    await waitFor(() => expect(result.current.activeDraft?.subject).toBe("Dinner"));
    expect(createDraftFromSent).toHaveBeenCalledWith("provider-sent", "me@example.com");
  });

  it("still opens an Edit as New copy when its inline image cannot download", async () => {
    vi.spyOn(mailClient, "listDrafts").mockResolvedValue([]);
    vi.spyOn(mailClient, "listOutbox").mockResolvedValue([]);
    vi.spyOn(console, "warn").mockImplementation(() => {});
    const inline = { id: "map", name: "map.png", size: 3, mime: "image/png", ready: false, messageId: "sent", providerId: null, inline: true, contentId: "map@example.com" };
    vi.spyOn(mailClient, "createDraftFromSent").mockResolvedValue({ ...draft, revision: 0, subject: "Picnic", body: "Hi", bodyHtml: "", attachments: [inline] });
    const saveDraft = vi.spyOn(mailClient, "saveDraft");
    vi.spyOn(mailClient, "fetchAttachment").mockRejectedValue(new Error("offline"));
    const { result } = renderHook(() => useCorrespondence([], "latest", "account@example.com", ...snippetArgs, "thread-1"));

    act(() => result.current.context.editAsNew("sent"));
    await waitFor(() => expect(result.current.activeDraft?.subject).toBe("Picnic"));
    // A plain-text copy has no HTML to sanitize, so nothing extra is saved.
    expect(saveDraft).not.toHaveBeenCalled();
  });

  it("cancels the pending outbox send and refreshes the lists", async () => {
    // Fake timers keep the one-second outbox poll from firing on its own, so
    // every listOutbox call below comes from a load or an explicit refresh.
    vi.useFakeTimers();
    const flush = () => act(async () => { await vi.advanceTimersByTimeAsync(0); });
    vi.spyOn(mailClient, "listDrafts").mockResolvedValue([]);
    const listOutbox = vi.spyOn(mailClient, "listOutbox").mockResolvedValue([outbox]);
    const cancelSend = vi.spyOn(mailClient, "cancelSend").mockResolvedValue(draft);
    const { result } = renderHook(() => useCorrespondence([], undefined, undefined, ...snippetArgs, null));
    await flush();
    expect(result.current.context.canUndoSend).toBe(true);
    expect(listOutbox).toHaveBeenCalledTimes(1);

    act(() => result.current.context.undoSend());
    await flush();
    expect(cancelSend).toHaveBeenCalledTimes(1);
    expect(cancelSend).toHaveBeenCalledWith("outbox-1");
    expect(listOutbox).toHaveBeenCalledTimes(2);
    expect(result.current.activeDraft).toEqual(draft);

    // Control: the poll is what would otherwise have added calls.
    await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
    expect(listOutbox).toHaveBeenCalledTimes(3);
  });

  it("guards duplicate recovery actions until the first request settles", async () => {
    vi.spyOn(mailClient, "listDrafts").mockResolvedValue([]);
    vi.spyOn(mailClient, "listOutbox").mockResolvedValue([]);
    let resolveRecovery!: (value: Draft) => void;
    const recoverSend = vi.spyOn(mailClient, "recoverSend").mockReturnValue(new Promise((resolve) => { resolveRecovery = resolve; }));
    const { result } = renderHook(() => useCorrespondence([], undefined, undefined, ...snippetArgs, null));

    act(() => {
      result.current.restoreFailedSend("outbox-1");
      result.current.restoreFailedSend("outbox-1");
    });
    expect(recoverSend).toHaveBeenCalledTimes(1);
    expect(result.current.pendingOutboxActions.has("outbox-1")).toBe(true);

    await act(async () => resolveRecovery(draft));
    await waitFor(() => expect(result.current.pendingOutboxActions.has("outbox-1")).toBe(false));
  });
  it("shows peer schedules read-only without recipients or invented delivery outcomes", () => {
    const report: ScheduledSendReport={operationId:"remote",ownerInstallationId:"peer",ownerSyncDeviceId:null,ownerNameAtCreation:"MacBook",account:"me@example.com",subject:"Remote schedule",scheduledAt:10,timeZone:"UTC",state:"scheduled",blockedReason:null,reportRevision:1,statusChangedAt:1};
    const undo=vi.fn(),restore=vi.fn(),reconcile=vi.fn(),editAsNew=vi.fn();
    const [entry] = outboxEntries([], [report]);
    render(<OutboxList entries={[entry]} selectedKey={null} clock={20} onSelect={vi.fn()} />);
    expect(screen.getByRole("option", { name: /Remote schedule/ })).toHaveTextContent("Awaiting an update from MacBook");
    expect(screen.getByRole("option")).not.toHaveTextContent(/To /);
    cleanup();
    const { container } = render(<OutboxDetail entry={entry} clock={20} onUndo={undo} onRestore={restore} onReconcile={reconcile} onEditAsNew={editAsNew} />);
    expect(screen.getByText("Scheduled time passed; awaiting an update from MacBook")).toBeInTheDocument();
    expect(screen.getByText("Manage on MacBook")).toBeInTheDocument();
    expect(screen.queryAllByRole("button")).toHaveLength(0);
    expect(container.querySelector(".message-stack")).toBeNull();
    expect(undo).not.toHaveBeenCalled();expect(restore).not.toHaveBeenCalled();expect(reconcile).not.toHaveBeenCalled();expect(editAsNew).not.toHaveBeenCalled();
    cleanup();
  });

  it("keeps local schedules out of the undo banner and deduplicates owner summaries", async () => {
    const report:ScheduledSendReport={operationId:outbox.id,ownerInstallationId:"self",ownerSyncDeviceId:null,ownerNameAtCreation:"Desktop",account:draft.account,subject:draft.subject,scheduledAt:Date.now()+60000,timeZone:"UTC",state:"scheduled",blockedReason:null,reportRevision:1,statusChangedAt:Date.now()};
    vi.spyOn(mailClient,"listDrafts").mockResolvedValue([]);
    vi.spyOn(mailClient,"listOutbox").mockResolvedValue([{...outbox,state:"scheduled",schedule:{report,canManage:true,visibility:"pending"}}]);
    vi.spyOn(mailClient,"listScheduledSummaries").mockResolvedValue([report]);
    const {result}=renderHook(() => useCorrespondence([],undefined,undefined,...snippetArgs,null));
    await waitFor(() => expect(result.current.outboxCount).toBe(1));
    expect(result.current.summaries).toHaveLength(0);expect(result.current.context.canUndoSend).toBe(false);
  });

  it("requires overdue confirmation on its owner and guards duplicate Send now", async () => {
    const report:ScheduledSendReport={operationId:outbox.id,ownerInstallationId:"self",ownerSyncDeviceId:null,ownerNameAtCreation:"Desktop",account:draft.account,subject:draft.subject,scheduledAt:10,timeZone:"UTC",state:"overdue",blockedReason:null,reportRevision:3,statusChangedAt:11};
    const sendNow=vi.spyOn(mailClient,"sendScheduledNow").mockResolvedValue();
    const changed=vi.fn().mockResolvedValue(undefined);
    const [entry] = outboxEntries([{...outbox,state:"overdue",schedule:{report,canManage:true,visibility:"local"}}], []);
    render(<OutboxDetail entry={entry} clock={20} onUndo={vi.fn()} onRestore={vi.fn()} onReconcile={vi.fn()} onChanged={changed} />);
    fireEvent.click(screen.getByRole("button",{name:"Send now"}));fireEvent.click(screen.getByRole("button",{name:"Send now"}));
    await waitFor(() => expect(changed).toHaveBeenCalledTimes(1));expect(sendNow).toHaveBeenCalledTimes(1);expect(sendNow).toHaveBeenCalledWith(outbox.id,3);
    cleanup();
  });

});

describe("outboxStateLabel", () => {
  it.each([
    ["unverifiable", "May have been sent"],
    ["uncertain", "Checking delivery"],
    ["failed", "Not sent"],
  ] as const)("maps %s to a plain label", (state, label) => {
    expect(outboxStateLabel(state)).toBe(label);
  });

  it.each(["scheduled", "overdue", "ready", "sending", "sent", "canceled"] as const)(
    "leaves %s unchanged so existing copy is unaffected",
    (state) => {
      expect(outboxStateLabel(state)).toBe(state);
    },
  );
});

const localEntry = (item: OutboxItem): OutboxEntry => ({ key: `local:${item.id}`, kind: "local", item });
const renderDetail = (item: OutboxItem, props: Partial<Parameters<typeof OutboxDetail>[0]> = {}) =>
  render(<OutboxDetail entry={localEntry(item)} clock={0} onUndo={vi.fn()} onRestore={vi.fn()} onReconcile={vi.fn()} {...props} />);

describe("Outbox send-state labels and Sent-copy notice", () => {
  afterEach(cleanup);

  it("shows the plain unverifiable label and keeps the explanatory error paragraph", () => {
    const item: OutboxItem = {
      ...outbox,
      state: "unverifiable",
      error: "This message may have been sent, but delivery could not be confirmed.",
    };
    render(<OutboxList entries={[localEntry(item)]} selectedKey={null} clock={0} onSelect={vi.fn()} />);
    expect(screen.getByRole("option")).toHaveTextContent("May have been sent");
    cleanup();
    renderDetail(item);
    expect(screen.getByText("May have been sent")).toBeInTheDocument();
    expect(screen.getByText(/delivery could not be confirmed/)).toBeInTheDocument();
    expect(screen.queryByText("unverifiable")).not.toBeInTheDocument();
  });

  it("labels uncertain and failed in plain language", () => {
    render(<OutboxList entries={outboxEntries([{ ...outbox, id: "a", state: "uncertain" }, { ...outbox, id: "b", state: "failed" }], [])} selectedKey={null} clock={0} onSelect={vi.fn()} />);
    expect(screen.getByText("Checking delivery")).toBeInTheDocument();
    expect(screen.getByText("Not sent")).toBeInTheDocument();
  });

  it("shows a one-line notice for an abandoned Sent copy using its plain error", () => {
    const item: OutboxItem = {
      ...outbox,
      state: "sent",
      sentCopy: { state: "abandoned", error: "Your message was sent, but a copy couldn't be saved to your Sent folder because no Sent folder is set up for this account." },
    };
    renderDetail(item);
    expect(screen.getByText(/no Sent folder is set up/)).toBeInTheDocument();
  });

  it.each(["pending", "verify", "done"] as const)("shows nothing for a %s Sent-copy state", (state) => {
    const item: OutboxItem = { ...outbox, state: "sent", sentCopy: { state, error: "should not appear" } };
    renderDetail(item);
    expect(screen.queryByText("should not appear")).not.toBeInTheDocument();
  });
});

describe("Outbox list and reader", () => {
  afterEach(cleanup);
  const sent: OutboxItem = { ...outbox, id: "sent", state: "sent", providerId: "provider-sent", draft: { ...draft, subject: "Dinner", to: "Ana Lima <ana@example.com>, bo@example.com" } };
  const failed: OutboxItem = { ...outbox, id: "failed", state: "failed", draft: { ...draft, subject: "Report" } };
  const canceled: OutboxItem = { ...outbox, id: "canceled", state: "canceled", draft: { ...draft, subject: "Never mind" } };

  it("lists outgoing sends as selectable rows with recipients, subject, and status", () => {
    const onSelect = vi.fn();
    const queued: OutboxItem = { ...sent, id: "queued", state: "undo_pending", deadline: 5_000, providerId: null };
    const entries = outboxEntries([queued, failed, canceled, sent], []);
    render(<OutboxList entries={entries} selectedKey={null} clock={0} onSelect={onSelect} />);
    const rows = screen.getAllByRole("option");
    expect(rows).toHaveLength(2);
    expect(rows[0]).toHaveAttribute("aria-selected", "true");
    expect(rows[0]).toHaveTextContent("To Ana Lima, bo@example.com");
    expect(rows[0]).toHaveTextContent("Dinner");
    expect(rows[0]).toHaveTextContent("Undo available · 5s");
    expect(rows[1]).toHaveAttribute("aria-selected", "false");
    expect(rows[1].querySelector(".outbox-row-status.problem")).toHaveTextContent("Not sent");
    expect(screen.queryByText("Never mind")).not.toBeInTheDocument();
    expect(screen.queryByText("sent")).not.toBeInTheDocument();
    fireEvent.click(rows[1]);
    expect(onSelect).toHaveBeenCalledWith(entries[1].key);
  });

  it("shows the selected send's message and offers Edit as New for a delivered send", () => {
    const editAsNew = vi.fn();
    renderDetail(sent, { onEditAsNew: editAsNew, children: <p>Message body</p> });
    expect(screen.getByRole("heading", { name: "Dinner" })).toBeInTheDocument();
    expect(screen.getByText("Message body")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Edit as New" }));
    expect(editAsNew).toHaveBeenCalledWith("provider-sent", "me@example.com");
  });

  it("offers recovery, not Edit as New, for a send that did not go out", () => {
    const restore = vi.fn();
    renderDetail(failed, { onRestore: restore, onEditAsNew: vi.fn() });
    expect(screen.queryByRole("button", { name: "Edit as New" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Restore Draft" }));
    expect(restore).toHaveBeenCalledWith("failed");
  });

  it("keeps Edit as New hidden until the provider's message id is known", () => {
    renderDetail({ ...sent, providerId: null }, { onEditAsNew: vi.fn() });
    expect(screen.queryByRole("button", { name: "Edit as New" })).not.toBeInTheDocument();
  });
});
