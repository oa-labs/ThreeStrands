import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ThreadDetail } from "./domain";
import { useWorkspaces } from "./useWorkspaces";
import { mailClient } from "./data/client";

vi.mock("./data/client", () => ({ mailClient: {
  listKeepInTouch: vi.fn().mockResolvedValue([]),
  listCalendarAccounts: vi.fn().mockResolvedValue([]),
  listCalendarOptions: vi.fn().mockResolvedValue([]),
} }));

const thread = (id: string) => ({ thread: { id }, messages: [] }) as unknown as ThreadDetail;

describe("the person picked for the context panel", () => {
  function setup() {
    const readingMessageIdRef = { current: "m1" as string | null };
    const hook = renderHook((props: { detail: ThreadDetail; reading: string | null }) => useWorkspaces({
      settingsOpen: false,
      settingsSection: "general" as never,
      visibleDetail: props.detail,
      readingMessageId: props.reading,
      readingMessageIdRef,
      openSettingsAt: vi.fn(),
      setNotice: vi.fn(),
    }), { initialProps: { detail: thread("t1"), reading: "m1" } });
    return { hook, readingMessageIdRef };
  }

  it("holds while the reader stays on the message it was picked from", () => {
    const { hook } = setup();
    act(() => hook.result.current.contactCardActions.onSelectPerson("ana@partner.org"));
    expect(hook.result.current.contextPersonEmail).toBe("ana@partner.org");
    expect(hook.result.current.contactCardActions.selectedEmail).toBe("ana@partner.org");
  });

  it("lets go once the reader moves to another message or conversation, and can be picked again", () => {
    const { hook, readingMessageIdRef } = setup();
    act(() => hook.result.current.contactCardActions.onSelectPerson("ana@partner.org"));
    hook.rerender({ detail: thread("t1"), reading: "m2" });
    expect(hook.result.current.contextPersonEmail).toBeNull();

    readingMessageIdRef.current = "m2";
    act(() => hook.result.current.contactCardActions.onSelectPerson("ana@partner.org"));
    expect(hook.result.current.contextPersonEmail).toBe("ana@partner.org");

    hook.rerender({ detail: thread("t2"), reading: "m2" });
    expect(hook.result.current.contextPersonEmail).toBeNull();
  });
});

describe("calendar accounts", () => {
  it("re-reads them when Settings opens, so an account synced from another device appears", async () => {
    const listCalendarAccounts = vi.mocked(mailClient.listCalendarAccounts);
    const hook = renderHook((props: { settingsOpen: boolean }) => useWorkspaces({
      settingsOpen: props.settingsOpen,
      settingsSection: "calendarAccounts",
      visibleDetail: null,
      readingMessageId: null,
      readingMessageIdRef: { current: null },
      openSettingsAt: vi.fn(),
      setNotice: vi.fn(),
    }), { initialProps: { settingsOpen: false } });
    await waitFor(() => expect(listCalendarAccounts).toHaveBeenCalled());
    const synced = { email: "synced@example.com", connectedAt: "2026-10-19T00:00:00Z", status: "needs_reauth" as const };
    listCalendarAccounts.mockResolvedValueOnce([synced]);

    hook.rerender({ settingsOpen: true });

    await waitFor(() => expect(hook.result.current.calendar.accounts).toEqual([synced]));
  });
});

describe("the remembered workspace", () => {
  const render = () => renderHook(() => useWorkspaces({
    settingsOpen: false,
    settingsSection: "general" as never,
    visibleDetail: null,
    readingMessageId: null,
    readingMessageIdRef: { current: null },
    openSettingsAt: vi.fn(),
    setNotice: vi.fn(),
  }));

  afterEach(() => localStorage.removeItem("threestrands.workspace.last"));

  it("reopens the view that was open when the app last closed", () => {
    const first = render();
    act(() => first.result.current.openFilesView());
    first.unmount();

    expect(render().result.current.rightWorkspace).toBe("files");
  });

  it("reopens Mail once the user has gone back to it", () => {
    const first = render();
    act(() => first.result.current.openTasksView());
    act(() => first.result.current.setRightWorkspace(null));
    first.unmount();

    expect(render().result.current.rightWorkspace).toBeNull();
  });
});

describe("Mail's right column", () => {
  const connected = [{ email: "cal@example.com", connectedAt: "2026-10-19T00:00:00Z", status: "connected" as const }];
  const render = (detail: ThreadDetail | null = null) => renderHook(() => useWorkspaces({
    settingsOpen: false,
    settingsSection: "general" as never,
    visibleDetail: detail,
    readingMessageId: null,
    readingMessageIdRef: { current: null },
    openSettingsAt: vi.fn(),
    setNotice: vi.fn(),
  }));

  afterEach(() => {
    localStorage.removeItem("threestrands.workspace.last");
    localStorage.removeItem("threestrands.workspace.sideColumn");
  });

  it("swaps between the context and the schedule, and returns to Mail from another view", async () => {
    vi.mocked(mailClient.listCalendarAccounts).mockResolvedValue(connected);
    const hook = render();
    await waitFor(() => expect(hook.result.current.calendarConnected).toBe(true));

    act(() => hook.result.current.openTasksView());
    act(() => hook.result.current.toggleSchedule());
    await waitFor(() => expect(hook.result.current.scheduleShown).toBe(true));
    expect(hook.result.current.rightWorkspace).toBeNull();

    act(() => hook.result.current.toggleSchedule());
    expect(hook.result.current.sideColumn).toBe("context");
    vi.mocked(mailClient.listCalendarAccounts).mockResolvedValue([]);
  });

  it("brings the context back for actions that show something in it, such as picking a person", async () => {
    vi.mocked(mailClient.listCalendarAccounts).mockResolvedValue(connected);
    const hook = render(thread("t1"));
    await waitFor(() => expect(hook.result.current.calendarConnected).toBe(true));
    act(() => hook.result.current.setSideColumn("schedule"));
    act(() => hook.result.current.openTodayView());

    act(() => hook.result.current.contactCardActions.onSelectPerson("ana@partner.org"));
    expect(hook.result.current.sideColumn).toBe("context");
    expect(hook.result.current.rightWorkspace).toBeNull();

    act(() => hook.result.current.setSideColumn("schedule"));
    act(() => hook.result.current.showMailContext());
    expect(hook.result.current.sideColumn).toBe("context");
    vi.mocked(mailClient.listCalendarAccounts).mockResolvedValue([]);
  });

  it("opens the schedule on a requested day", () => {
    const hook = render();
    const day = new Date(2026, 9, 30);
    act(() => hook.result.current.openCalendarAt(day));
    expect(hook.result.current.sideColumn).toBe("schedule");
    expect(hook.result.current.calendarSidebarStart?.date).toBe(day);
  });
});
