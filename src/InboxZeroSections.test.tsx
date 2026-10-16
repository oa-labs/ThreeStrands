import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import type { CalendarOption, ContactProfile, ThreadTask } from "./domain";
import { InboxZeroSections } from "./InboxZeroSections";

function task(id: string, overrides: Partial<ThreadTask> = {}): ThreadTask {
  return {
    id, accountId: "you@example.com", threadId: null, sourceMessageId: null, subjectSnapshot: null,
    title: `Task ${id}`, notes: null, kind: "action", dueKind: "none", dueValue: null, timeZone: null,
    repeatIntervalDays: null, status: "open", completionSource: null, evidenceText: null, waitAfter: null,
    createdAt: "2026-09-19T10:00:00Z", updatedAt: "2026-09-19T10:00:00Z", completedAt: null, ...overrides,
  };
}

function contact(id: string, name: string | null, dueAt: string | null): ContactProfile {
  return {
    id, displayName: name, role: null, company: null, location: null, bio: null, notes: null, links: [], photoData: null,
    favorite: false, addresses: [`${id}@example.com`], sentCount: 0, receivedCount: 0, lastInteractedAt: null, birthday: null,
    keepInTouch: { intervalDays: 30, startedAt: null, snoozedUntil: null, snoozedAt: null, lastTouchAt: null },
    keepInTouchDueAt: dueAt,
  };
}

const calendars: CalendarOption[] = [{ id: "primary", accountId: "cal@example.com", name: "Work", primary: true, selected: true, writable: true }];
const daysFromNow = (days: number) => new Date(Date.now() + days * 86_400_000).toISOString();

function renderSections(overrides: Partial<Parameters<typeof InboxZeroSections>[0]> = {}) {
  const handlers = { onOpenEvent: vi.fn(), onEditTask: vi.fn(), onOpenTasks: vi.fn(), onOpenContact: vi.fn(), onOpenKeepInTouch: vi.fn() };
  render(<InboxZeroSections accountId="you@example.com" taskRefreshKey={0} calendarConnected={false} calendars={calendars} {...handlers} {...overrides} />);
  return handlers;
}

describe("InboxZeroSections", () => {
  beforeEach(() => {
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [], errors: [] });
  });
  afterEach(() => { cleanup(); vi.restoreAllMocks(); });

  it("lists open tasks for the account in view, soonest due first, and opens them", async () => {
    const listTasks = vi.spyOn(mailClient, "listTasks").mockResolvedValue([
      task("later", { dueKind: "date", dueValue: "2099-01-01" }),
      task("done", { status: "completed", completedAt: "2026-10-01T00:00:00Z" }),
      task("soon", { dueKind: "date", dueValue: "2098-01-01" }),
    ]);
    vi.spyOn(mailClient, "listKeepInTouch").mockResolvedValue([]);
    const { onEditTask, onOpenTasks } = renderSections();

    const section = await screen.findByRole("region", { name: "Open tasks" });
    expect(listTasks).toHaveBeenCalledWith("you@example.com");
    const titles = within(section).getAllByRole("button", { name: /^Task / }).map((button) => button.textContent);
    expect(titles[0]).toContain("Task soon");
    expect(titles[1]).toContain("Task later");
    expect(within(section).queryByText("Task done")).not.toBeInTheDocument();

    fireEvent.click(within(section).getByRole("button", { name: /Task soon/ }));
    expect(onEditTask).toHaveBeenCalledWith(expect.objectContaining({ id: "soon" }));
    fireEvent.click(within(section).getByRole("button", { name: "Open Tasks" }));
    expect(onOpenTasks).toHaveBeenCalled();
  });

  it("lists only people who are due or overdue, and opens their contact", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([]);
    vi.spyOn(mailClient, "listKeepInTouch").mockResolvedValue([
      contact("future", "Future Friend", daysFromNow(20)),
      contact("overdue", "Ada Lovelace", daysFromNow(-3)),
      contact("off", "No Reminder", null),
    ]);
    const { onOpenContact, onOpenKeepInTouch } = renderSections();

    const section = await screen.findByRole("region", { name: "Keep in touch" });
    expect(within(section).getByText("Ada Lovelace")).toBeInTheDocument();
    expect(within(section).getByText(/^Overdue since /)).toBeInTheDocument();
    expect(within(section).queryByText("Future Friend")).not.toBeInTheDocument();
    expect(within(section).queryByText("No Reminder")).not.toBeInTheDocument();

    fireEvent.click(within(section).getByRole("button", { name: /Ada Lovelace/ }));
    expect(onOpenContact).toHaveBeenCalledWith("overdue");
    fireEvent.click(within(section).getByRole("button", { name: "Open Keep in Touch" }));
    expect(onOpenKeepInTouch).toHaveBeenCalled();
  });

  it("leaves out empty sections, and the agenda when no calendar is connected", async () => {
    const listTasks = vi.spyOn(mailClient, "listTasks").mockResolvedValue([]);
    const listKeepInTouch = vi.spyOn(mailClient, "listKeepInTouch").mockResolvedValue([]);
    renderSections({ calendarConnected: false });

    await waitFor(() => expect(listTasks).toHaveBeenCalled());
    await waitFor(() => expect(listKeepInTouch).toHaveBeenCalled());
    expect(screen.queryByRole("region")).not.toBeInTheDocument();
    expect(mailClient.listScheduleEvents).not.toHaveBeenCalled();
  });

  it("shows today's agenda only when a calendar is connected", async () => {
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([]);
    vi.spyOn(mailClient, "listKeepInTouch").mockResolvedValue([]);
    renderSections({ calendarConnected: true });

    expect(await screen.findByRole("region", { name: "Today's agenda" })).toBeInTheDocument();
    await waitFor(() => expect(mailClient.listScheduleEvents).toHaveBeenCalled());
  });

  it("leaves a section out when its data cannot be read", async () => {
    vi.spyOn(mailClient, "listTasks").mockRejectedValue(new Error("locked"));
    vi.spyOn(mailClient, "listKeepInTouch").mockRejectedValue(new Error("locked"));
    renderSections();

    await waitFor(() => expect(mailClient.listKeepInTouch).toHaveBeenCalled());
    expect(screen.queryByRole("region")).not.toBeInTheDocument();
  });
});
