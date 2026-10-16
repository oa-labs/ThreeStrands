import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { ComposeContext, ReplyChecks } from "./ComposeContext";
import { AvailabilitySection, RecipientSummary } from "./RecipientSections";
import { mailClient } from "./data/client";
import { ICON_SIZE } from "./iconSizes";
import { SLOTS_PER_DAY } from "./scheduling";
import { saveAiProvider } from "./aiSettings";
import type { Draft } from "./correspondence";
import type { Account, AvailabilityPreferences, ContactActivity, ContactProfile, ContactSuggestion } from "./domain";

vi.mock("./data/client", () => ({ mailClient: {
  listContactSuggestions: vi.fn(), resolveContactIds: vi.fn(), getContactProfile: vi.fn(), contactActivity: vi.fn(),
  contactTimeline: vi.fn(), contactFiles: vi.fn(), domainContext: vi.fn(), listTasks: vi.fn(), listContactTasks: vi.fn(),
  findAvailability: vi.fn(), findTimes: vi.fn(), listScheduleEvents: vi.fn(), fileStars: vi.fn().mockResolvedValue([]),setFileStar: vi.fn().mockResolvedValue(undefined),openAttachment: vi.fn(), listContactGroupRecipients: vi.fn(),
} }));

const accounts = [{ email: "me@acme.com" }, { email: "me@gmail.com" }] as Account[];
const draft: Draft = {
  id: "draft-1", revision: 0, account: "me@acme.com", mode: "new",
  sourceId: null, threadId: null, replyId: null, references: [],
  to: "", cc: "", bcc: "", subject: "Planning", body: "", attachments: [], updatedAt: 0,
};
const preferences: AvailabilityPreferences = {
  timeZone: "UTC",
  workingWindows: [1, 2, 3, 4, 5].map((weekday) => ({ weekday, start: "09:00", end: "17:00" })),
  defaultDurationMinutes: 30,
  slotIncrementMinutes: 30,
};
const ann: ContactProfile = {
  id: "contact:ann", displayName: "Ann Lee", role: "CTO", company: "Partner Co", location: null, bio: null,
  notes: "Prefers short emails", links: [], photoData: null, favorite: false, addresses: ["ann@partner.com"],
  sentCount: 4, receivedCount: 6, lastInteractedAt: null, birthday: null,
  keepInTouch: { intervalDays: null, startedAt: null, snoozedUntil: null, snoozedAt: null, lastTouchAt: null }, keepInTouchDueAt: null,
};
const activity: ContactActivity = { sentCount: 4, receivedCount: 6, threadCount: 3, firstAt: "2025-02-01T00:00:00Z", lastSentAt: "2026-09-20T00:00:00Z", recentReceivedAt: [] };
const known = (email: string, sentCount: number, displayName: string | null = null): ContactSuggestion =>
  ({ email, displayName, sentCount, receivedCount: 1, lastInteractedAt: "2026-09-01T00:00:00Z", pinned: false });

function renderPanel(overrides: Partial<Parameters<typeof ComposeContext>[0]> = {}) {
  const props: Parameters<typeof ComposeContext>[0] = {
    draft, accounts, calendarConnected: false, findTimesAi: false, preferences, taskRefreshKey: 0,
    onAttach: vi.fn(), onReplaceRecipient: vi.fn(), onSwitchAccount: vi.fn(), onMoveToBcc: vi.fn(), onInsertTimes: vi.fn(),
    onOpenCalendarSettings: vi.fn(), onOpenEvent: vi.fn(),
    onOpenThread: vi.fn(), onShowMessage: vi.fn(), onEditTask: vi.fn(), onDraftFollowUp: vi.fn(), onTasksChanged: vi.fn(),
    ...overrides,
  };
  return { props, ...render(<ComposeContext {...props} />) };
}

describe("ComposeContext", () => {
  beforeEach(() => {
    localStorage.clear();
    vi.mocked(mailClient.listContactSuggestions).mockImplementation(async (account) => account === "me@acme.com"
      ? [known("ann@partner.com", 4, "Ann Lee"), known("john@partner.com", 2, "John Roe")]
      : [known("pal@friends.org", 5)]);
    vi.mocked(mailClient.resolveContactIds).mockImplementation(async (emails) =>
      Object.fromEntries(emails.filter((email) => email === "ann@partner.com").map((email) => [email, ann.id])));
    vi.mocked(mailClient.getContactProfile).mockImplementation(async (id) => id === ann.id ? ann : null);
    vi.mocked(mailClient.contactActivity).mockResolvedValue(activity);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([
      { threadId: "t-1", accountId: "me@acme.com", contactEmail: "ann@partner.com", subject: "Q3 budget", snippet: "Numbers inside", sentAt: "2026-09-20T00:00:00Z", labels: [] },
    ]);
    vi.mocked(mailClient.contactFiles).mockResolvedValue({ files: [], total: 0 });
    vi.mocked(mailClient.listContactGroupRecipients).mockResolvedValue([]);
    vi.mocked(mailClient.domainContext).mockResolvedValue({ people: [], threads: [] });
    vi.mocked(mailClient.listTasks).mockResolvedValue([]);
    vi.mocked(mailClient.listContactTasks).mockResolvedValue([]);
    vi.mocked(mailClient.listScheduleEvents).mockResolvedValue({ events: [], errors: [] });
  });
  afterEach(() => { cleanup(); vi.clearAllMocks(); });

  it("asks for a recipient before showing history", () => {
    renderPanel();
    expect(screen.getByRole("complementary", { name: "Compose context" })).toHaveTextContent("Add a recipient to see your history with them.");
    expect(mailClient.resolveContactIds).not.toHaveBeenCalled();
    expect(mailClient.listContactSuggestions).not.toHaveBeenCalled();
    expect(mailClient.listContactGroupRecipients).not.toHaveBeenCalled();
  });

  it("defers history queries until a complete recipient and reuses them during editing", async () => {
    const { props, rerender } = renderPanel();
    rerender(<ComposeContext {...props} draft={{ ...draft, to: "an" }} />);
    expect(mailClient.listContactSuggestions).not.toHaveBeenCalled();
    expect(mailClient.listContactGroupRecipients).not.toHaveBeenCalled();
    rerender(<ComposeContext {...props} draft={{ ...draft, to: "ann@partner.com" }} />);
    await waitFor(() => expect(mailClient.listContactSuggestions).toHaveBeenCalledTimes(2));
    expect(mailClient.listContactGroupRecipients).toHaveBeenCalledTimes(1);
    rerender(<ComposeContext {...props} draft={{ ...draft, to: "ann@partner.com", subject: "Updated", body: "Typing" }} />);
    expect(mailClient.listContactSuggestions).toHaveBeenCalledTimes(2);
    expect(mailClient.listContactGroupRecipients).toHaveBeenCalledTimes(1);
  });

  it("shows what the user saved about the recipient, and recent emails with them", async () => {
    const { props } = renderPanel({ draft: { ...draft, to: "Ann Lee <ann@partner.com>" } });
    const about = await screen.findByRole("region", { name: "About Ann Lee" });
    expect(within(about).getByRole("heading", { name: "About" })).toBeInTheDocument();
    expect(about).toHaveTextContent("CTO · Partner Co");
    expect(about).toHaveTextContent("Prefers short emails");
    // Volume and dates belong to Recent emails, not a second summary above it.
    expect(about).not.toHaveTextContent(/emails since|You last wrote/);
    expect(mailClient.contactActivity).not.toHaveBeenCalled();
    expect(mailClient.contactTimeline).toHaveBeenCalledWith(ann.id, 0, 5);
    fireEvent.click(await screen.findByRole("button", { name: /Q3 budget/ }));
    expect(props.onOpenThread).toHaveBeenCalledWith("t-1");
  });

  it.each([null, "A recipient with a very long display name"])("leaves the section out when nothing is saved about the recipient (name: %s)", (name) => {
    const email = "someone.with.a.long.address@a.very.long.organization.example.com";
    const { container } = render(<RecipientSummary email={email} name={name} profile={null} />);
    expect(container).toBeEmptyDOMElement();
  });

  it.each([
    [{ role: "CTO", company: null, notes: null }, "CTO", null],
    [{ role: null, company: null, notes: "  Prefers mornings  " }, null, "Prefers mornings"],
    [{ role: null, company: null, notes: "   " }, null, null],
  ])("shows only the saved details that exist (%o)", (fields, role, notes) => {
    const { container } = render(<RecipientSummary email={ann.addresses[0]} name={null} profile={{ ...ann, ...fields }} />);
    if (!role && !notes) {
      expect(container).toBeEmptyDOMElement();
      return;
    }
    const about = screen.getByRole("region", { name: "About Ann Lee" });
    expect(about.querySelector(".compose-recipient-role")?.textContent ?? null).toBe(role);
    expect(about.querySelector(".compose-recipient-notes")?.textContent ?? null).toBe(notes);
  });

  it("keys the recipient's tasks and files sections distinctly", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      renderPanel({ draft: { ...draft, to: "Ann Lee <ann@partner.com>" } });
      await screen.findByRole("region", { name: "About Ann Lee" });
      await waitFor(() => expect(mailClient.contactFiles).toHaveBeenCalled());
      await waitFor(() => expect(mailClient.listContactTasks).toHaveBeenCalled());
      expect(consoleError.mock.calls.filter((args) => String(args[0]).includes("same key"))).toEqual([]);
    } finally {
      consoleError.mockRestore();
    }
  });

  it("switches whose history is shown among several recipients", async () => {
    renderPanel({ draft: { ...draft, to: "ann@partner.com", cc: "Pal <pal@friends.org>" } });
    const header = screen.getByRole("group", { name: "Showing history with" });
    expect(header).toHaveTextContent("ann@partner.com");
    fireEvent.click(within(header).getByRole("button", { name: "1 other" }));
    fireEvent.click(within(header).getByRole("button", { name: /^Pal/ }));
    expect(header).toHaveTextContent("Pal");
    expect(within(header).queryByRole("button", { name: /^Pal/ })).not.toBeInTheDocument();
    await waitFor(() => expect(mailClient.contactTimeline).toHaveBeenLastCalledWith("derived:pal@friends.org", 0, 5));
  });

  it("lists the user's colleagues after the people the message is going to", async () => {
    renderPanel({ draft: { ...draft, to: "Sam <sam@acme.com>, ann@partner.com" } });
    const header = screen.getByRole("group", { name: "Showing history with" });
    // The first outside recipient is shown, not the colleague listed first.
    expect(header).toHaveTextContent("ann@partner.com");
    fireEvent.click(within(header).getByRole("button", { name: "1 other" }));
    expect(within(within(header).getByRole("list", { name: "Other people" })).getAllByRole("button").map((button) => button.title)).toEqual(["sam@acme.com"]);
  });

  it("offers the address the user probably meant and applies it through the composer", async () => {
    const { props } = renderPanel({ draft: { ...draft, to: "jonh@partner.com" } });
    const checks = await screen.findByRole("region", { name: "Before you send" });
    expect(checks).toHaveTextContent("You’ve never emailed jonh@partner.com. Did you mean john@partner.com?");
    fireEvent.click(within(checks).getByRole("button", { name: "Use john@partner.com instead of jonh@partner.com" }));
    expect(props.onReplaceRecipient).toHaveBeenCalledWith("jonh@partner.com", "John Roe <john@partner.com>");
  });

  it("offers attaching files and switching to the account the recipient knows", async () => {
    const { props } = renderPanel({ draft: { ...draft, to: "pal@friends.org", body: "I've attached the photos." } });
    const checks = await screen.findByRole("region", { name: "Before you send" });
    fireEvent.click(within(checks).getByRole("button", { name: "Attach Files" }));
    expect(props.onAttach).toHaveBeenCalled();
    fireEvent.click(await within(checks).findByRole("button", { name: "Send From me@gmail.com" }));
    expect(props.onSwitchAccount).toHaveBeenCalledWith("me@gmail.com");
  });

  it("leaves the checks out when nothing needs a look", async () => {
    renderPanel({ draft: { ...draft, to: "ann@partner.com" } });
    await screen.findByRole("region", { name: "About Ann Lee" });
    await waitFor(() => expect(mailClient.listContactSuggestions).toHaveBeenCalledTimes(2));
    expect(screen.queryByRole("region", { name: "Before you send" })).not.toBeInTheDocument();
  });

  it("finds open times on request and inserts the chosen ones into the draft", async () => {
    vi.mocked(mailClient.findAvailability).mockResolvedValue({
      candidates: [
        { start: "2030-01-07T15:00:00Z", end: "2030-01-07T15:30:00Z", status: "verified" },
        { start: "2030-01-08T15:00:00Z", end: "2030-01-08T15:30:00Z", status: "verified" },
      ],
      checkedCalendarCount: 1, totalCalendarCount: 1, errors: [],
    });
    const { props } = renderPanel({ calendarConnected: true, draft: { ...draft, to: "ann@partner.com" } });
    const section = screen.getByRole("region", { name: "Availability" });
    const glyph = within(section).getByRole("heading", { name: "Availability" }).querySelector(".context-row-glyph");
    expect(glyph).toHaveAttribute("aria-hidden", "true");
    expect(glyph?.querySelector("svg.lucide-calendar-days")).toHaveAttribute("width", String(ICON_SIZE.xs));
    const findTimes = within(section).getByRole("button", { name: "Find Times" });
    expect(findTimes).toHaveClass("btn", "btn-sm");
    // Nothing is checked until asked.
    expect(mailClient.findAvailability).not.toHaveBeenCalled();
    fireEvent.click(findTimes);
    await waitFor(() => expect(mailClient.findAvailability).toHaveBeenCalledWith(expect.objectContaining({
      preferences: expect.objectContaining({ defaultDurationMinutes: 30 }),
      maxPerDay: SLOTS_PER_DAY,
    })));
    fireEvent.click(await within(section).findByRole("button", { name: "Insert These Times" }));
    expect(props.onInsertTimes).toHaveBeenCalledWith([
      expect.objectContaining({ start: "2030-01-07T15:00:00Z" }),
      expect.objectContaining({ start: "2030-01-08T15:00:00Z" }),
    ]);
    expect(within(section).queryByRole("button", { name: /Draft Reply/ })).not.toBeInTheDocument();
    // Open times are only offered; booking waits for the recipient to pick one.
    expect(within(section).queryByRole("button", { name: "Add to Calendar" })).not.toBeInTheDocument();
    // Without Draft Assist the coming week is searched, and nothing is sent to an AI provider.
    expect(mailClient.findTimes).not.toHaveBeenCalled();
    expect(within(section).getByRole("button", { name: "Search Again" }).querySelector("svg.lucide-sparkles")).toBeNull();
    expect(within(section).queryByRole("textbox", { name: "Adjust times" })).not.toBeInTheDocument();
    const searchAgain = within(section).getByRole("button", { name: "Search Again" });
    expect(searchAgain).toHaveClass("btn", "btn-sm");
    fireEvent.click(searchAgain);
    await waitFor(() => expect(mailClient.findAvailability).toHaveBeenCalledTimes(2));
  });

  it("leaves out availability without a connected calendar", () => {
    renderPanel();
    expect(screen.queryByRole("region", { name: "Availability" })).not.toBeInTheDocument();
  });
});

describe("Find Times with AI", () => {
  const day = 86_400_000;
  const search = {
    rangeStart: new Date(Date.now() + 3 * day).toISOString(),
    rangeEnd: new Date(Date.now() + 10 * day).toISOString(),
    durationMinutes: null, weekdays: [4, 5], dayStart: "13:00", dayEnd: null,
    reason: "Ann asked to meet Thursday or Friday afternoon next week.",
  };
  const reply: Draft = { ...draft, mode: "reply", sourceId: "message-9", to: "ann@partner.com", subject: "Re: Planning", body: "Here are some times that work for me:" };

  beforeEach(() => {
    saveAiProvider("openai");
    vi.mocked(mailClient.listContactSuggestions).mockResolvedValue([]);
    vi.mocked(mailClient.resolveContactIds).mockResolvedValue({});
    vi.mocked(mailClient.listTasks).mockResolvedValue([]);
    vi.mocked(mailClient.findAvailability).mockResolvedValue({
      candidates: [{ start: search.rangeStart, end: new Date(Date.parse(search.rangeStart) + 1_800_000).toISOString(), status: "verified" }],
      checkedCalendarCount: 1, totalCalendarCount: 1, errors: [],
    });
  });
  // Answers queued for one call must not leak into the next test.
  afterEach(() => { cleanup(); vi.clearAllMocks(); vi.mocked(mailClient.findTimes).mockReset(); localStorage.clear(); });

  const section = () => screen.getByRole("region", { name: "Availability" });

  it("reads the conversation and draft, explains the search, and searches only the days and hours it found", async () => {
    let answer!: (value: typeof search) => void;
    vi.mocked(mailClient.findTimes).mockReturnValue(new Promise((resolve) => { answer = resolve; }));
    renderPanel({ calendarConnected: true, findTimesAi: true, draft: reply });
    expect(within(section()).getByText("Suggest open times from your calendar, based on what this conversation asks for.")).toBeInTheDocument();
    // Nothing is sent until asked.
    expect(mailClient.findTimes).not.toHaveBeenCalled();

    expect(within(section()).getByRole("button", { name: "Find Times" }).querySelector("svg.lucide-sparkles")).not.toBeNull();
    fireEvent.click(within(section()).getByRole("button", { name: "Find Times" }));
    expect(await within(section()).findByRole("status")).toHaveTextContent("Reading the conversation…");
    expect(within(section()).getByRole("button", { name: "Find Times" })).toBeDisabled();
    expect(mailClient.findTimes).toHaveBeenCalledWith({
      sourceMessageId: "message-9", draftSubject: "Re: Planning", draftText: "Here are some times that work for me:",
      instruction: null, current: null, userTimeZone: "UTC",
    }, "openai", "gpt-4o", null);
    answer(search);

    expect(await within(section()).findByText(search.reason)).toBeInTheDocument();
    await waitFor(() => expect(mailClient.findAvailability).toHaveBeenCalled());
    const request = vi.mocked(mailClient.findAvailability).mock.calls[0][0];
    expect(request.rangeStart).toBe(search.rangeStart);
    expect(request.rangeEnd).toBe(search.rangeEnd);
    expect(request.preferences.workingWindows).toEqual([
      { weekday: 4, start: "13:00", end: "17:00" },
      { weekday: 5, start: "13:00", end: "17:00" },
    ]);
    expect(await within(section()).findByRole("button", { name: "Insert This Time" })).toBeInTheDocument();
  });

  it("adjusts the search on screen, including chip changes, from the user's words", async () => {
    vi.mocked(mailClient.findTimes)
      .mockResolvedValueOnce(search)
      .mockResolvedValueOnce({ ...search, weekdays: [4], reason: "Thursday afternoon, as you asked." })
      .mockRejectedValueOnce(new Error("AI provider timed out."));
    renderPanel({ calendarConnected: true, findTimesAi: true, draft: reply });
    fireEvent.click(within(section()).getByRole("button", { name: "Find Times" }));
    await within(section()).findByText(search.reason);

    fireEvent.click(await within(section()).findByRole("button", { name: "Mornings" }));
    await waitFor(() => expect(mailClient.findAvailability).toHaveBeenCalledTimes(2));
    const input = await within(section()).findByRole("textbox", { name: "Adjust times" });
    expect(input).toHaveAttribute("maxLength", "300");
    expect(within(section()).getByRole("button", { name: "Adjust" }).querySelector("svg.lucide-sparkles")).not.toBeNull();
    expect(within(section()).getByRole("button", { name: "Adjust" })).toBeDisabled();
    fireEvent.change(input, { target: { value: "only Thursday" } });
    fireEvent.click(within(section()).getByRole("button", { name: "Adjust" }));

    expect(await within(section()).findByText("Thursday afternoon, as you asked.")).toBeInTheDocument();
    expect(vi.mocked(mailClient.findTimes).mock.calls[1][0]).toEqual({
      sourceMessageId: "message-9", draftSubject: "Re: Planning", draftText: "Here are some times that work for me:",
      instruction: "only Thursday",
      current: {
        rangeStart: new Date(search.rangeStart).toISOString(), rangeEnd: new Date(search.rangeEnd).toISOString(), durationMinutes: 30,
        weekdays: [4, 5], dayStart: null, dayEnd: "12:00", reason: search.reason,
      },
      userTimeZone: "UTC",
    });
    const next = await within(section()).findByRole("textbox", { name: "Adjust times" });
    expect(next).toHaveValue("");

    // A failed adjustment keeps the times on screen and says why.
    fireEvent.change(next, { target: { value: "later" } });
    fireEvent.submit(within(section()).getByRole("textbox", { name: "Adjust times" }));
    expect(await within(section()).findByRole("alert")).toHaveTextContent("AI provider timed out.");
    expect(within(section()).getByText("Thursday afternoon, as you asked.")).toBeInTheDocument();
    expect(within(section()).getByRole("textbox", { name: "Adjust times" })).toHaveValue("later");
  });

  it("falls back to the coming week when the conversation can't be read", async () => {
    vi.mocked(mailClient.findTimes).mockRejectedValue(new Error("No AI API key configured."));
    renderPanel({ calendarConnected: true, findTimesAi: true, draft: reply });
    fireEvent.click(within(section()).getByRole("button", { name: "Find Times" }));

    expect(await within(section()).findByRole("alert")).toHaveTextContent("No AI API key configured. Showing the coming week instead.");
    await waitFor(() => expect(mailClient.findAvailability).toHaveBeenCalledTimes(1));
    const request = vi.mocked(mailClient.findAvailability).mock.calls[0][0];
    expect(Date.parse(request.rangeEnd) - Date.parse(request.rangeStart)).toBe(7 * day);
    expect(request.preferences.workingWindows).toEqual(preferences.workingWindows);
  });

  it("first reuses the range a meeting suggestion already found, then reads afresh on Search Again", async () => {
    vi.mocked(mailClient.findTimes).mockResolvedValue({ ...search, reason: "A fresh read." });
    const suggested = {
      plan: { query: { kind: "range" as const, start: search.rangeStart, end: search.rangeEnd }, durationMinutes: 45, timeZoneAssumed: false },
      reason: "From the suggested meeting: “next week”.",
    };
    const suggest = vi.fn(() => suggested);
    render(<AvailabilitySection preferences={preferences} ai draft={reply} suggest={suggest} onInsertTimes={vi.fn()} onOpenCalendarSettings={vi.fn()} />);

    // Reusing the suggestion sends nothing to the provider, so the first press carries no AI mark.
    expect(within(section()).getByRole("button", { name: "Find Times" }).querySelector("svg.lucide-sparkles")).toBeNull();
    fireEvent.click(within(section()).getByRole("button", { name: "Find Times" }));
    expect(await within(section()).findByText("From the suggested meeting: “next week”.")).toBeInTheDocument();
    await waitFor(() => expect(mailClient.findAvailability).toHaveBeenCalledWith(expect.objectContaining({
      rangeStart: search.rangeStart, rangeEnd: search.rangeEnd, preferences: expect.objectContaining({ defaultDurationMinutes: 45 }),
    })));
    // Nothing was sent to the AI provider; the suggestion's search is adjustable like any other.
    expect(mailClient.findTimes).not.toHaveBeenCalled();
    expect(await within(section()).findByRole("textbox", { name: "Adjust times" })).toBeInTheDocument();

    const again = within(section()).getByRole("button", { name: "Search Again" });
    expect(again.querySelector("svg.lucide-sparkles")).not.toBeNull();
    fireEvent.click(again);
    expect(await within(section()).findByText("A fresh read.")).toBeInTheDocument();
    expect(mailClient.findTimes).toHaveBeenCalledTimes(1);
  });

  it("uses a meeting suggestion's range even without AI", async () => {
    const suggest = () => ({
      plan: { query: { kind: "range" as const, start: search.rangeStart, end: search.rangeEnd }, durationMinutes: 30, timeZoneAssumed: false },
      reason: "From the suggested meeting.",
    });
    render(<AvailabilitySection preferences={preferences} ai={false} draft={reply} suggest={suggest} onInsertTimes={vi.fn()} onOpenCalendarSettings={vi.fn()} />);
    fireEvent.click(within(section()).getByRole("button", { name: "Find Times" }));
    expect(await within(section()).findByText("From the suggested meeting.")).toBeInTheDocument();
    await waitFor(() => expect(mailClient.findAvailability).toHaveBeenCalledWith(expect.objectContaining({ rangeStart: search.rangeStart })));
    expect(within(section()).queryByRole("textbox", { name: "Adjust times" })).not.toBeInTheDocument();
    expect(mailClient.findTimes).not.toHaveBeenCalled();
  });

  it("reads only a new message's own text, and asks again from scratch on Search Again", async () => {
    let first!: (value: typeof search) => void;
    vi.mocked(mailClient.findTimes)
      .mockReturnValueOnce(new Promise((resolve) => { first = resolve; }))
      .mockResolvedValueOnce({ ...search, reason: "Second look." });
    renderPanel({ calendarConnected: true, findTimesAi: true, draft: { ...draft, subject: "Lunch next week?", body: "Free next week?" } });
    fireEvent.click(within(section()).getByRole("button", { name: "Find Times" }));
    await waitFor(() => expect(mailClient.findTimes).toHaveBeenCalledTimes(1));
    // A new message has no conversation; only its own subject and text are read.
    expect(vi.mocked(mailClient.findTimes).mock.calls[0][0]).toEqual(expect.objectContaining({ sourceMessageId: null, draftSubject: "Lunch next week?", draftText: "Free next week?" }));
    first(search);
    expect(await within(section()).findByText(search.reason)).toBeInTheDocument();
    fireEvent.click(within(section()).getByRole("button", { name: "Search Again" }));
    expect(await within(section()).findByText("Second look.")).toBeInTheDocument();
    expect(mailClient.findTimes).toHaveBeenCalledTimes(2);
    expect(vi.mocked(mailClient.findTimes).mock.calls[1][0]).toEqual(expect.objectContaining({ instruction: null, current: null }));
  });
});

describe("ReplyChecks", () => {
  afterEach(() => { cleanup(); vi.clearAllMocks(); });

  it("checks a reply's own words but not the quoted message", async () => {
    vi.mocked(mailClient.listContactSuggestions).mockResolvedValue([known("ann@partner.com", 3)]);
    const reply: Draft = { ...draft, mode: "reply", to: "ann@partner.com", subject: "Re: Plan", body: "Sounds good\n\nOn Mon, Ann <ann@partner.com> wrote:\n> See attached" };
    vi.mocked(mailClient.listContactGroupRecipients).mockResolvedValue([]);
    const props = { draft: reply, accounts, onAttach: vi.fn(), onReplaceRecipient: vi.fn(), onSwitchAccount: vi.fn(), onMoveToBcc: vi.fn() };
    const { rerender } = render(<ReplyChecks {...props} />);
    await waitFor(() => expect(mailClient.listContactSuggestions).toHaveBeenCalledTimes(2));
    expect(screen.queryByRole("region", { name: "Before you send" })).not.toBeInTheDocument();
    rerender(<ReplyChecks {...props} draft={{ ...reply, body: `I've attached it.${reply.body}` }} />);
    expect(await screen.findByRole("region", { name: "Before you send" })).toHaveTextContent("Says “attached”, but nothing is attached");
  });
});

describe("group recipient checks", () => {
  afterEach(() => { cleanup(); vi.clearAllMocks(); });
  const members = (count: number) => Array.from({ length: count }, (_, index) => ({
    contactId: `contact:${index}`, displayName: `Person ${index}`, email: `p${index}@partner.com`, addresses: [`p${index}@partner.com`, `p${index}@home.example`],
  }));
  const addresses = (count: number) => members(count).map((member) => member.email).join(", ");
  const renderReply = (to: string, bcc = "") => {
    const onMoveToBcc = vi.fn();
    render(<ReplyChecks draft={{ ...draft, to, bcc, subject: "Board update" }} accounts={accounts} onAttach={vi.fn()} onReplaceRecipient={vi.fn()} onSwitchAccount={vi.fn()} onMoveToBcc={onMoveToBcc} />);
    return onMoveToBcc;
  };

  it("suggests Bcc once more than ten of a group's members are in To or Cc", async () => {
    vi.mocked(mailClient.listContactSuggestions).mockResolvedValue([]);
    vi.mocked(mailClient.listContactGroupRecipients).mockResolvedValue([{ id: "g1", name: "Board", members: members(11) }]);
    const onMoveToBcc = renderReply(addresses(11));
    const section = await screen.findByRole("region", { name: "Before you send" });
    await waitFor(() => expect(section).toHaveTextContent("11 people from Board are in To or Cc, so each will see everyone else’s address"));
    fireEvent.click(within(section).getByRole("button", { name: "Move Board to Bcc" }));
    expect(onMoveToBcc).toHaveBeenCalledWith(members(11).map((member) => member.email));
  });

  it("stays quiet at exactly ten, or when the rest are already in Bcc", async () => {
    vi.mocked(mailClient.listContactSuggestions).mockResolvedValue([]);
    vi.mocked(mailClient.listContactGroupRecipients).mockResolvedValue([{ id: "g1", name: "Board", members: members(11) }]);
    renderReply(addresses(10));
    await waitFor(() => expect(mailClient.listContactGroupRecipients).toHaveBeenCalled());
    await waitFor(() => expect(mailClient.listContactSuggestions).toHaveBeenCalled());
    expect(screen.queryByText(/people from Board/)).not.toBeInTheDocument();
    cleanup();
    renderReply(addresses(10), "p10@partner.com");
    await waitFor(() => expect(mailClient.listContactGroupRecipients).toHaveBeenCalledTimes(2));
    expect(screen.queryByText(/people from Board/)).not.toBeInTheDocument();
  });
});

