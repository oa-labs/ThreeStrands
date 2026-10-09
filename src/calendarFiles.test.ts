import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { CALENDAR_FILE_EVENT, listenForOpenedCalendarFiles } from "./calendarFiles";
import { MAIL_LINK_EVENT, listenForNativeMailLinks, setMailtoHandler } from "./mailtoLink";
import type { OpenedCalendarFile } from "./domain";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const opened: OpenedCalendarFile = { name: "invite.ics", preview: null, error: "unreadable" };

// Captures the native event handler and lets a test fire it.
function nativeEvents() {
  const handlers = new Map<string, () => void>();
  const unlisten = vi.fn();
  vi.mocked(listen).mockImplementation(async (event, handler) => {
    handlers.set(event, handler as () => void);
    return unlisten;
  });
  return { fire: (event: string) => handlers.get(event)?.(), unlisten };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("native open listeners", () => {
  beforeEach(() => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
  });
  afterEach(() => {
    delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    setMailtoHandler(null);
    vi.mocked(invoke).mockReset();
    vi.mocked(listen).mockReset();
  });

  it("delivers calendar files queued before and after the listener started", async () => {
    const events = nativeEvents();
    vi.mocked(invoke).mockResolvedValueOnce([opened]).mockResolvedValueOnce([]).mockResolvedValueOnce([{ ...opened, name: "later.ics" }]);
    const onFiles = vi.fn();

    const stop = listenForOpenedCalendarFiles(onFiles);
    await settle();
    expect(invoke).toHaveBeenCalledWith("take_pending_calendar_files");
    expect(onFiles).toHaveBeenCalledExactlyOnceWith([opened]);

    events.fire(CALENDAR_FILE_EVENT);
    await settle();
    events.fire(CALENDAR_FILE_EVENT);
    await settle();
    expect(onFiles).toHaveBeenCalledTimes(2);
    expect(onFiles).toHaveBeenLastCalledWith([{ ...opened, name: "later.ics" }]);

    stop();
    await settle();
    expect(events.unlisten).toHaveBeenCalled();
  });

  it("parses mail links queued natively and hands them to the composer", async () => {
    const events = nativeEvents();
    vi.mocked(invoke).mockResolvedValueOnce(["mailto:jane@example.com?subject=Hi", "https://not-mail.example/"]).mockResolvedValueOnce(["mailto:alex@example.com"]);
    const handler = vi.fn();
    setMailtoHandler(handler);

    listenForNativeMailLinks();
    await settle();
    events.fire(MAIL_LINK_EVENT);
    await settle();

    expect(invoke).toHaveBeenCalledWith("take_pending_mail_links");
    expect(handler.mock.calls.map(([request]) => [request.to, request.subject])).toEqual([
      ["jane@example.com", "Hi"],
      ["alex@example.com", ""],
    ]);
  });

  it("does nothing outside the native app", () => {
    delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    listenForOpenedCalendarFiles(vi.fn())();
    listenForNativeMailLinks()();
    expect(listen).not.toHaveBeenCalled();
    expect(invoke).not.toHaveBeenCalled();
  });
});
