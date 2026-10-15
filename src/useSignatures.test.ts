import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { listen } from "@tauri-apps/api/event";
import { mailClient } from "./data/client";
import { useSignatures } from "./useSignatures";
import { type SignatureLibrary } from "./signatures";

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(vi.fn()) }));
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.clearAllMocks(); delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__; });

describe("signature library", () => {
  const template = { id: "work", name: "Work", html: "<b>Alex</b>", text: "Alex" };
  it("refreshes changes from another device and disposes its listener", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    const stop = vi.fn(); vi.mocked(listen).mockResolvedValue(stop);
    const read = vi.spyOn(mailClient, "signatureLibrary").mockResolvedValue({ signatures: [], defaults: [] });
    const { result, unmount } = renderHook(useSignatures);
    await waitFor(() => expect(listen).toHaveBeenCalledWith("replicated-sync-status", expect.any(Function)));
    read.mockResolvedValue({ signatures: [template], defaults: [] });
    await act(async () => { (vi.mocked(listen).mock.calls[0][1] as () => void)(); });
    await waitFor(() => expect(result.current.signatures[0]).toEqual(template));
    unmount(); expect(stop).toHaveBeenCalled();
  });

  it("does not replace a fresh mutation response with a stale startup response", async () => {
    let complete!: (library: SignatureLibrary) => void;
    vi.spyOn(mailClient, "signatureLibrary").mockReturnValueOnce(new Promise((resolve) => { complete = resolve; })).mockResolvedValue({ signatures: [template], defaults: [] });
    const save = vi.spyOn(mailClient, "saveSignature").mockResolvedValue(undefined);
    const inherit = vi.spyOn(mailClient, "deleteSignatureDefaults").mockResolvedValue(undefined);
    const { result } = renderHook(useSignatures);
    await act(async () => { await result.current.save(template); });
    expect(save).toHaveBeenCalledWith(template);
    await act(async () => { complete({ signatures: [], defaults: [] }); });
    expect(result.current.signatures).toEqual([template]);
    await act(async () => { await result.current.inheritDefaults("ME@example.com", "ALIAS@example.com"); });
    expect(inherit).toHaveBeenCalledWith("ME@example.com", "ALIAS@example.com");
  });
});
