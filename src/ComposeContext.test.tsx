import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { ComposeContext, ReplyChecks } from "./ComposeContext";
import { mailClient } from "./data/client";
import type { Draft } from "./correspondence";
import type { Account, AvailabilityPreferences, ContactActivity, ContactProfile, ContactSuggestion } from "./domain";

vi.mock("./data/client", () => ({ mailClient: {
  listContactSuggestions: vi.fn(), resolveContactIds: vi.fn(), getContactProfile: vi.fn(), contactActivity: vi.fn(),
  contactTimeline: vi.fn(), contactFiles: vi.fn(), domainContext: vi.fn(), listTasks: vi.fn(), listContactTasks: vi.fn(),
  findAvailability: vi.fn(), listScheduleEvents: vi.fn(), openAttachment: vi.fn(), listContactGroupRecipients: vi.fn(),
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
    draft, accounts, calendarConnected: false, preferences, taskRefreshKey: 0,
    onAttach: vi.fn(), onReplaceRecipient: vi.fn(), onSwitchAccount: vi.fn(), onMoveToBcc: vi.fn(), onInsertTimes: vi.fn(),
    onAddToCalendar: vi.fn(), onMoreTimes: vi.fn(), onOpenCalendarSettings: vi.fn(), onOpenEvent: vi.fn(),
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
  });

  it("shows who the recipient is, the user's notes, and recent emails with them", async () => {
    const { props } = renderPanel({ draft: { ...draft, to: "Ann Lee <ann@partner.com>" } });
    const about = await screen.findByRole("region", { name: "About Ann Lee" });
    expect(about).toHaveTextContent("CTO · Partner Co");
    expect(about).toHaveTextContent("Prefers short emails");
    await waitFor(() => expect(about).toHaveTextContent("10 emails since"));
    expect(mailClient.contactTimeline).toHaveBeenCalledWith(ann.id, 0, 5);
    fireEvent.click(await screen.findByRole("button", { name: /Q3 budget/ }));
    expect(props.onOpenThread).toHaveBeenCalledWith("t-1");
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
    const chips = screen.getByRole("group", { name: "Show history with" });
    expect(within(chips).getByRole("button", { name: "ann@partner.com" })).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(within(chips).getByRole("button", { name: "Pal" }));
    expect(await screen.findByRole("region", { name: "About Pal" })).toBeInTheDocument();
    await waitFor(() => expect(mailClient.contactActivity).toHaveBeenLastCalledWith("derived:pal@friends.org"));
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
    // Nothing is checked until asked.
    expect(mailClient.findAvailability).not.toHaveBeenCalled();
    fireEvent.click(within(section).getByRole("button", { name: "Find Times" }));
    await waitFor(() => expect(mailClient.findAvailability).toHaveBeenCalledWith(expect.objectContaining({
      preferences: expect.objectContaining({ defaultDurationMinutes: 30 }),
      maxPerDay: 1,
    })));
    fireEvent.click(await within(section).findByRole("button", { name: "Insert These Times" }));
    expect(props.onInsertTimes).toHaveBeenCalledWith([
      expect.objectContaining({ start: "2030-01-07T15:00:00Z" }),
      expect.objectContaining({ start: "2030-01-08T15:00:00Z" }),
    ]);
    expect(within(section).queryByRole("button", { name: /Draft Reply/ })).not.toBeInTheDocument();
  });

  it("leaves out availability without a connected calendar", () => {
    renderPanel();
    expect(screen.queryByRole("region", { name: "Availability" })).not.toBeInTheDocument();
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

