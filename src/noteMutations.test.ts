import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { listen } from "@tauri-apps/api/event";
import { afterEach, expect, it, vi } from "vitest";
import { NOTES_CHANGED, useNoteChanges } from "./noteMutations";

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
const original = Object.getOwnPropertyDescriptor(window, "__TAURI_INTERNALS__");
afterEach(() => {
  cleanup(); vi.clearAllMocks();
  if (original) Object.defineProperty(window, "__TAURI_INTERNALS__", original);
  else Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
});
it("refreshes task and backlink consumers after local writes and native note sync", async () => {
  Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
  const stop = vi.fn(), changed = vi.fn();
  vi.mocked(listen).mockResolvedValue(stop);
  const { unmount } = renderHook(() => useNoteChanges(changed));
  await waitFor(() => expect(listen).toHaveBeenCalledOnce());
  expect(vi.mocked(listen).mock.calls[0][0]).toBe("replicated-sync-status");
  act(() => vi.mocked(listen).mock.calls[0][1]({ event: "replicated-sync-status", id: 1, payload: null }));
  expect(changed).toHaveBeenCalledOnce();
  act(() => window.dispatchEvent(new CustomEvent(NOTES_CHANGED)));
  expect(changed).toHaveBeenCalledTimes(2);
  unmount(); expect(stop).toHaveBeenCalledOnce();
  act(() => window.dispatchEvent(new CustomEvent(NOTES_CHANGED)));
  expect(changed).toHaveBeenCalledTimes(2);
});
it("removes a native listener that finishes registering after unmount", async () => {
  Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
  let ready!: (stop: () => void) => void;
  vi.mocked(listen).mockReturnValue(new Promise((resolve) => { ready = resolve; }));
  const { unmount } = renderHook(() => useNoteChanges(vi.fn()));
  unmount(); const stop = vi.fn();
  await act(async () => { ready(stop); });
  expect(stop).toHaveBeenCalledOnce();
});
