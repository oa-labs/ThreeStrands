import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { mailClient } from "./data/client";
import { DEMO_ACCOUNT_ID } from "./data/demoClient";
import { saveSelectedAccountId } from "./settings";

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

  // Wires up the same fake Tauri event-plugin handshake each test uses to
  // fire "unread-counts-changed" by hand, and returns a helper to do so.
  function setupEventBridge() {
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

    return {
      async fireUnreadCountsChanged(payload: string) {
        await waitFor(() => expect(listenIdsByEvent.has("unread-counts-changed")).toBe(true));
        const id = listenIdsByEvent.get("unread-counts-changed");
        await act(async () => {
          handlersById.get(id!)?.({ event: "unread-counts-changed", id, payload });
        });
      },
    };
  }

  it("refreshes the sidebar unread badges when a background account reports a sync", async () => {
    const bridge = setupEventBridge();
    const listUnreadCounts = vi.spyOn(mailClient, "listUnreadCounts");
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    listUnreadCounts.mockClear();

    await bridge.fireUnreadCountsChanged(DEMO_ACCOUNT_ID);

    expect(listUnreadCounts).toHaveBeenCalled();
  });

  it("refreshes the Inbox badge and the visible thread list in the merged All accounts view", async () => {
    // Default activeAccountId (no stored preference) is the merged "All
    // accounts" view, which shows every account's mail, so any account's
    // sync should refresh what's on screen.
    const bridge = setupEventBridge();
    const mailboxUnreadCounts = vi.spyOn(mailClient, "mailboxUnreadCounts");
    const listThreadsPage = vi.spyOn(mailClient, "listThreadsPage");
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    mailboxUnreadCounts.mockClear();
    listThreadsPage.mockClear();

    await bridge.fireUnreadCountsChanged(DEMO_ACCOUNT_ID);

    expect(mailboxUnreadCounts).toHaveBeenCalled();
    expect(listThreadsPage).toHaveBeenCalled();
  });

  it("does not refetch the thread list for a different account's sync while a single account is active", async () => {
    saveSelectedAccountId(DEMO_ACCOUNT_ID);
    const bridge = setupEventBridge();
    const mailboxUnreadCounts = vi.spyOn(mailClient, "mailboxUnreadCounts");
    const listThreadsPage = vi.spyOn(mailClient, "listThreadsPage");
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    mailboxUnreadCounts.mockClear();
    listThreadsPage.mockClear();

    await bridge.fireUnreadCountsChanged("other@example.com");
    // The badge is account-agnostic to refresh (cheap, and background
    // accounts' sidebar totals still need to catch up), but the open thread
    // list belongs to the active account and shouldn't reload for mail that
    // landed in an account the user isn't looking at.
    expect(mailboxUnreadCounts).toHaveBeenCalled();
    expect(listThreadsPage).not.toHaveBeenCalled();

    await bridge.fireUnreadCountsChanged(DEMO_ACCOUNT_ID);

    expect(listThreadsPage).toHaveBeenCalled();
  });
});
