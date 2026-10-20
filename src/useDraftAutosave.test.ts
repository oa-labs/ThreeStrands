import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { listen } from "@tauri-apps/api/event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Draft, ServerDraftEvent } from "./correspondence";
import { mailClient } from "./data/client";
import { savedStatus, useDraftAutosave } from "./useDraftAutosave";

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(vi.fn()) }));
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.clearAllMocks(); delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__; });

const draft: Draft = {
  id: "draft-1", revision: 1, account: "you@example.com", mode: "new", sourceId: null, threadId: null, replyId: null,
  references: [], to: "", cc: "", bcc: "", subject: "", body: "", attachments: [], updatedAt: 0,
};

describe("savedStatus", () => {
  it("says Draft saved when the draft stays on this device or the server has it", () => {
    expect(savedStatus({}).text).toBe("Draft saved");
    expect(savedStatus({}, { state: "saved" }).text).toBe("Draft saved");
    expect(savedStatus({ tooLargeToSync: true }, null).text).toBe("Draft saved · Too large to sync");
  });
  it("never says Draft saved before the server confirms", () => {
    expect(savedStatus({}, { state: "pending" }).text).toBe("Saving…");
    expect(savedStatus({}, { state: "retrying", error: "offline" })).toEqual({ text: "Saved on this device · Will sync when online", detail: "offline" });
    expect(savedStatus({}, { state: "failed", error: "Reconnect" })).toEqual({ text: "Saved on this device · Couldn't save to server", detail: "Reconnect" });
  });
});

describe("useDraftAutosave server status", () => {
  async function savedPending() {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    const stop = vi.fn(); vi.mocked(listen).mockResolvedValue(stop);
    vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1, serverDraft: { state: "pending" } }));
    const hook = renderHook(() => useDraftAutosave(draft, () => null, () => {}));
    await waitFor(() => expect(listen).toHaveBeenCalledWith("draft-server-status", expect.any(Function)));
    act(() => hook.result.current.edit("subject", "Plans"));
    await act(async () => { await hook.result.current.flush(); });
    expect(hook.result.current.status.text).toBe("Saving…");
    const handler = vi.mocked(listen).mock.calls[0][1] as (event: { payload: ServerDraftEvent }) => void;
    return { hook, stop, emit: (payload: ServerDraftEvent) => act(() => handler({ payload })) };
  }

  it("shows Draft saved once the server confirms the current revision", async () => {
    const { hook, emit } = await savedPending();
    emit({ id: "draft-1", revision: 1, status: { state: "saved" } });
    expect(hook.result.current.status.text).toBe("Saving…");
    emit({ id: "other", revision: 2, status: { state: "saved" } });
    expect(hook.result.current.status.text).toBe("Saving…");
    emit({ id: "draft-1", revision: 2, status: { state: "saved" } });
    expect(hook.result.current.status.text).toBe("Draft saved");
  });

  it("shows a failed push with its reason", async () => {
    const { hook, emit } = await savedPending();
    emit({ id: "draft-1", revision: 2, status: { state: "retrying", error: "The server couldn't be reached." } });
    expect(hook.result.current.status).toEqual({ text: "Saved on this device · Will sync when online", detail: "The server couldn't be reached." });
  });

  it("leaves unsaved edits alone and stops listening on unmount", async () => {
    const { hook, emit, stop } = await savedPending();
    act(() => hook.result.current.edit("subject", "Plans, again"));
    emit({ id: "draft-1", revision: 2, status: { state: "saved" } });
    expect(hook.result.current.status.text).toBe("Unsaved changes");
    hook.unmount();
    expect(stop).toHaveBeenCalled();
  });
});
