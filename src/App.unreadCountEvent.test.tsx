import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { mailClient } from "./data/client";

// Isolated in its own file (like App.settingsImport.test.tsx) because it puts
// the app in "real Tauri" mode via `__TAURI_INTERNALS__`, which would
// otherwise leak between tests sharing App.test.tsx's demo-mode module
// instance.
describe("background unread count updates", () => {
  const transformCallback = vi.fn();
  const invoke = vi.fn();

  beforeEach(() => {
    localStorage.clear();
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true,
      value: {
        transformCallback,
        unregisterCallback: vi.fn(),
        invoke,
      },
    });
    Object.defineProperty(window, "__TAURI_EVENT_PLUGIN_INTERNALS__", {
      configurable: true,
      value: { unregisterListener: vi.fn() },
    });
  });

  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("refreshes the sidebar unread badges when a background account reports a sync", async () => {
    const handlersById = new Map<number, (event: unknown) => void>();
    const listenIdsByEvent = new Map<string, number>();
    let nextId = 1;

    transformCallback.mockImplementation((callback: (event: unknown) => void) => {
      const id = nextId++;
      handlersById.set(id, callback);
      return id;
    });
    invoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "plugin:event|listen") {
        const handlerId = args?.handler as number;
        listenIdsByEvent.set(args?.event as string, handlerId);
        return Promise.resolve(handlerId);
      }
      return Promise.resolve(1);
    });

    const listUnreadCounts = vi.spyOn(mailClient, "listUnreadCounts");
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    await waitFor(() => expect(listenIdsByEvent.has("unread-counts-changed")).toBe(true));
    listUnreadCounts.mockClear();

    const id = listenIdsByEvent.get("unread-counts-changed");
    await act(async () => {
      handlersById.get(id!)?.({ event: "unread-counts-changed", id, payload: null });
    });

    expect(listUnreadCounts).toHaveBeenCalled();
  });
});
