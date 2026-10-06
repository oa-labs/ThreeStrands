import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import { ThreadAssist, type ThreadAssistProps } from "./ThreadAssist";
import type { ActionProposal, ThreadDetail } from "./domain";

const detail: ThreadDetail = {
  thread: {
    id: "thread-1",
    providerThreadId: "provider-1",
    subject: "Website setup",
    snippet: "Please set up the website by Friday.",
    participants: ["client@example.com"],
    lastMessageAt: "2026-09-19T10:00:00Z",
    lastReceivedAt: "2026-09-19T10:00:00Z",
    unread: false,
    starred: false,
    archived: false,
    trashed: false,
    labels: ["INBOX"],
    accountId: "you@example.com",
    summary: null,
    summaryGeneratedAt: null,
    summaryRevision: null,
    hasAttachments: false,
  },
  messages: [{
    id: "message-1",
    threadId: "thread-1",
    sender: "client@example.com",
    recipients: ["you@example.com"],
    sentAt: "2026-09-19T10:00:00Z",
    bodyHtml: "<p>Please set up the website by Friday.</p>",
    bodyText: "Please set up the website by Friday.",
    unread: false,
    unsubscribe: null,
    attachments: [],
  }],
};

const summarized: ThreadDetail = {
  ...detail,
  thread: { ...detail.thread, summary: "- The client needs the website.\n- Due Friday.", summaryGeneratedAt: "2026-09-19T11:00:00Z" },
};

const taskProposal: ActionProposal = {
  type: "task",
  kind: "action",
  title: "Set up the website",
  notes: "Complete the requested setup.",
  dueKind: "date",
  dueValue: "2026-09-25",
  timeZone: "America/New_York",
  repeatIntervalDays: null,
  confidence: 0.86,
  evidence: { sourceMessageId: "message-1", excerpt: "Please set up the website by Friday." },
};

function props(overrides: Partial<Omit<ThreadAssistProps, "summary" | "suggestions" | "scheduling">> & {
  summary?: Partial<ThreadAssistProps["summary"]>;
  suggestions?: Partial<ThreadAssistProps["suggestions"]>;
  scheduling?: Partial<ThreadAssistProps["scheduling"]>;
} = {}): ThreadAssistProps {
  const { summary, suggestions, scheduling, ...rest } = overrides;
  return {
    detail,
    loading: false,
    error: null,
    preview: null,
    onRun: vi.fn(),
    onOpenSettings: vi.fn(),
    ...rest,
    summary: { enabled: true, available: true, pending: false, ...summary },
    suggestions: {
      enabled: true,
      available: true,
      requested: false,
      proposals: [],
      hiddenCount: 0,
      onDiscard: vi.fn(),
      onReview: vi.fn(),
      ...suggestions,
    },
    scheduling: {
      calendarConnected: false,
      preferences: { timeZone: "America/New_York", workingWindows: [], defaultDurationMinutes: 30, slotIncrementMinutes: 30 },
      onAddToCalendar: vi.fn(),
      onReplyWithTimes: vi.fn(),
      onConfirmTime: vi.fn(),
      onMoreTimes: vi.fn(),
      onOpenCalendarSettings: vi.fn(),
      ...scheduling,
    },
  };
}

describe("ThreadAssist", () => {
  afterEach(() => { cleanup(); vi.restoreAllMocks(); });

  it("offers one Get Brief action before anything has run and refreshes everything afterwards", () => {
    const onRun = vi.fn();
    const { rerender } = render(<ThreadAssist {...props({ onRun })} />);
    // Section labels carry no icons; the AI mark stays on the action.
    expect(screen.getByRole("heading", { name: "Brief" }).querySelector("svg")).toBeNull();
    const getBrief = screen.getByRole("button", { name: "Get Brief" });
    expect(getBrief.querySelector("svg")).not.toBeNull();
    expect(getBrief).toHaveAttribute("aria-keyshortcuts", "i");
    const briefTooltip = within(getBrief.parentElement as HTMLElement).getByRole("tooltip");
    expect(within(briefTooltip).getByText("Get Brief")).toBeInTheDocument();
    expect(within(briefTooltip).getByText("i").tagName).toBe("KBD");
    fireEvent.click(getBrief);
    expect(onRun).toHaveBeenLastCalledWith(false);
    expect(screen.queryByRole("button", { name: "Refresh brief" })).not.toBeInTheDocument();

    rerender(<ThreadAssist {...props({ onRun, detail: summarized, suggestions: { requested: true } })} />);
    expect(screen.getByText("The client needs the website.")).toBeInTheDocument();
    expect(screen.getByText("Due Friday.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Get Brief" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Refresh brief" }));
    expect(onRun).toHaveBeenLastCalledWith(true);
  });

  it("keeps the run button in the section header so an unrun brief takes one line", () => {
    const { rerender, container } = render(<ThreadAssist {...props()} />);
    const header = container.querySelector(".context-section-header")!;
    expect(within(header as HTMLElement).getByRole("button", { name: "Get Brief" })).toBeInTheDocument();
    expect(container.querySelectorAll(".thread-assist-run")).toHaveLength(1);

    rerender(<ThreadAssist {...props({ detail: summarized, suggestions: { requested: true } })} />);
    expect(screen.queryByRole("button", { name: "Get Brief" })).not.toBeInTheDocument();
    expect(within(header as HTMLElement).getByRole("button", { name: "Refresh brief" })).toBeInTheDocument();

    rerender(<ThreadAssist {...props({ loading: true })} />);
    expect(within(container.querySelector(".context-section-header") as HTMLElement).getByRole("status")).toHaveTextContent("Reading the conversation…");
    expect(screen.queryByRole("button", { name: "Get Brief" })).not.toBeInTheDocument();
  });

  it("asks only for the missing suggestions when a summary was saved earlier", () => {
    const onRun = vi.fn();
    render(<ThreadAssist {...props({ onRun, detail: summarized })} />);
    expect(screen.queryByRole("button", { name: "Get Brief" })).not.toBeInTheDocument();
    const getSuggestions = screen.getByRole("button", { name: "Get Suggestions" });
    expect(getSuggestions).not.toHaveAttribute("aria-keyshortcuts");
    expect(within(getSuggestions.parentElement as HTMLElement).getByRole("tooltip").querySelector("kbd")).toBeNull();
    fireEvent.click(getSuggestions);
    expect(onRun).toHaveBeenCalledWith(false);
  });

  it("flags a summary that predates the newest message", () => {
    render(<ThreadAssist {...props({ detail: { ...summarized, thread: { ...summarized.thread, lastMessageAt: "2026-09-20T09:00:00Z" } } })} />);
    expect(screen.getByText("New messages since this brief.")).toBeInTheDocument();
  });

  it("flags mail that arrived while the brief was being written, even though it predates the save", () => {
    // Written from the thread as of 10:00, saved at 11:00; a message sent at
    // 10:30 arrived during the provider call.
    const thread = { ...summarized.thread, summaryRevision: "2026-09-19T10:00:00Z", summaryGeneratedAt: "2026-09-19T11:00:00Z", lastMessageAt: "2026-09-19T10:30:00Z" };
    render(<ThreadAssist {...props({ detail: { ...summarized, thread } })} />);
    expect(screen.getByText("New messages since this brief.")).toBeInTheDocument();
  });

  it("titles the section Suggestions when summaries are off", () => {
    render(<ThreadAssist {...props({ detail: summarized, summary: { enabled: false, available: false } })} />);
    expect(screen.getByRole("heading", { name: "Suggestions" })).toBeInTheDocument();
    expect(screen.queryByText("The client needs the website.")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Get Suggestions" })).toBeInTheDocument();
  });

  it("shows a non-retryable error verbatim without Try Again or technical details", () => {
    const onRun = vi.fn();
    const message = "AI briefs are disabled for this account";
    const { container } = render(<ThreadAssist {...props({ onRun, error: message })} />);
    expect(screen.getByRole("alert")).toHaveTextContent(message);
    expect(screen.queryByRole("button", { name: "Try Again" })).not.toBeInTheDocument();
    expect(screen.queryByText("Technical details")).not.toBeInTheDocument();
    expect(container.querySelector(".action-analysis-error-details")).toBeNull();
    expect(onRun).not.toHaveBeenCalled();
  });

  it("explains missing provider credentials separately from the feature flags", () => {
    const onOpenSettings = vi.fn();
    const { rerender } = render(<ThreadAssist {...props({ onOpenSettings, summary: { available: false }, suggestions: { available: false } })} />);
    expect(screen.getByText("Set up an AI provider and API key in AI settings to get a brief of this conversation.")).toBeInTheDocument();
    expect(screen.queryByText("AI briefs and suggestions are off.")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "AI Settings" }));
    expect(onOpenSettings).toHaveBeenCalledTimes(1);

    rerender(<ThreadAssist {...props({ onOpenSettings, summary: { enabled: false, available: false }, suggestions: { enabled: false, available: false } })} />);
    expect(screen.getByText("AI briefs and suggestions are off.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Get Brief" })).not.toBeInTheDocument();
  });

  it("previews typed proposals with evidence and review actions", () => {
    const onReview = vi.fn();
    const onDiscard = vi.fn();
    render(<ThreadAssist {...props({ preview: '{"emailContext":{"messages":[]}}', suggestions: { requested: true, proposals: [taskProposal], onReview, onDiscard } })} />);

    expect(screen.getByRole("heading", { name: "Suggested" })).toBeInTheDocument();
    expect(screen.getByText("Set up the website")).toBeInTheDocument();
    fireEvent.click(screen.getByText("From the email"));
    expect(screen.getByText("Please set up the website by Friday.")).toBeInTheDocument();
    expect(screen.getByText(/^client@example\.com · /)).toBeInTheDocument();
    expect(screen.queryByText(/confidence/i)).not.toBeInTheDocument();
    expect(screen.queryByText("Check details")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Review & Add Task" }));
    expect(onReview).toHaveBeenCalledWith(0, taskProposal, "accept");
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    expect(onReview).toHaveBeenLastCalledWith(0, taskProposal, "edit");
    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    expect(onDiscard).toHaveBeenCalledWith(0);
    expect(screen.getByText("What was shared with AI")).toBeInTheDocument();
  });

  it("flags uncertain suggestions for a closer look and schedules an unzoned meeting in the user's time zone", async () => {
    const meeting: ActionProposal = {
      type: "meeting",
      intent: "schedule",
      title: "Website kickoff",
      participants: [],
      location: null,
      rawTimeLanguage: "sometime next week",
      normalizedStart: null,
      normalizedEnd: null,
      searchRangeStart: null,
      searchRangeEnd: null,
      durationMinutes: 30,
      timeZone: null,
      confidence: 0.4,
      evidence: { sourceMessageId: "message-unknown", excerpt: "Let's meet next week." },
    };
    const find = vi.spyOn(mailClient, "findAvailability").mockResolvedValue({ candidates: [], checkedCalendarCount: 1, totalCalendarCount: 1, errors: [] });
    render(<ThreadAssist {...props({ suggestions: { requested: true, proposals: [meeting] }, scheduling: { calendarConnected: true } })} />);

    expect(screen.getByText("Check details")).toBeInTheDocument();
    expect(screen.queryByText(/%/)).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Find Times" })).not.toBeInTheDocument();
    const schedule = screen.getByRole("group", { name: "Schedule" });
    expect(within(schedule).getByText("Using your time zone (America/New_York)")).toBeInTheDocument();
    await waitFor(() => expect(find).toHaveBeenCalledWith(expect.objectContaining({ maxPerDay: 1 })));
    expect(await within(schedule).findByText("No open times in your working hours for this range.")).toBeInTheDocument();
    fireEvent.click(screen.getByText("From the email"));
    expect(screen.queryByText(/message-unknown/)).not.toBeInTheDocument();
  });

  it("schedules meetings only, and hands Add to Calendar the card's own suggestion", async () => {
    const meeting: ActionProposal = {
      type: "meeting", intent: "schedule", title: "Budget review", participants: ["jane@example.com"], location: null,
      rawTimeLanguage: "Thursday at 3", normalizedStart: new Date(Date.now() + 26 * 3_600_000).toISOString(), normalizedEnd: new Date(Date.now() + 26.5 * 3_600_000).toISOString(),
      searchRangeStart: null, searchRangeEnd: null, durationMinutes: 30, timeZone: "America/New_York", confidence: 0.9,
      evidence: { sourceMessageId: "message-1", excerpt: "Please set up the website by Friday." },
    };
    vi.spyOn(mailClient, "checkProposedTime").mockResolvedValue({ status: "free", conflicts: [], checkedCalendarCount: 1, totalCalendarCount: 1, errors: [] });
    const onAddToCalendar = vi.fn();
    render(<ThreadAssist {...props({ suggestions: { requested: true, proposals: [taskProposal, meeting] }, scheduling: { calendarConnected: true, onAddToCalendar } })} />);

    expect(screen.getAllByRole("group", { name: "Schedule" })).toHaveLength(1);
    fireEvent.click(await screen.findByRole("button", { name: "Add to Calendar" }));
    expect(onAddToCalendar).toHaveBeenCalledWith(1, meeting, { kind: "specific", start: new Date(meeting.normalizedStart!).toISOString(), end: new Date(meeting.normalizedEnd!).toISOString() });
  });

  it("reports withheld suggestions instead of claiming there was nothing to do", () => {
    const { rerender } = render(<ThreadAssist {...props({ preview: "{}", suggestions: { requested: true, hiddenCount: 2 } })} />);
    expect(screen.getByText("2 suggestions couldn’t be matched to the email, so they were hidden.")).toBeInTheDocument();
    expect(screen.queryByText("Nothing to schedule or follow up on.")).not.toBeInTheDocument();

    rerender(<ThreadAssist {...props({ preview: "{}", suggestions: { requested: true, hiddenCount: 1 } })} />);
    expect(screen.getByText("1 suggestion couldn’t be matched to the email, so it was hidden.")).toBeInTheDocument();

    rerender(<ThreadAssist {...props({ preview: "{}", suggestions: { requested: true } })} />);
    expect(screen.getByText("Nothing to schedule or follow up on.")).toBeInTheDocument();
  });

  it("explains unreadable provider output in plain language and keeps the details available", () => {
    const onRun = vi.fn();
    render(<ThreadAssist {...props({ onRun, error: "The AI provider returned malformed thread brief JSON" })} />);

    expect(screen.getByRole("alert")).toHaveTextContent("The AI's response couldn't be read. Try again, or choose a different model in AI settings.");
    expect(screen.getByText("The AI provider returned malformed thread brief JSON")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Get Brief" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Try Again" }));
    expect(onRun).toHaveBeenCalledWith(false);
  });

  it("offers no copy action before a brief exists", () => {
    render(<ThreadAssist {...props()} />);
    expect(screen.queryByRole("button", { name: "Copy brief" })).not.toBeInTheDocument();
  });

  it("copies the brief to the clipboard and confirms it until the brief changes", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const bulleted: ThreadDetail = {
      ...summarized,
      thread: { ...summarized.thread, summary: "- The client needs the website.\n• Due Friday." },
    };
    const { rerender } = render(<ThreadAssist {...props({ detail: bulleted })} />);

    fireEvent.click(screen.getByRole("button", { name: "Copy brief" }));

    expect(writeText).toHaveBeenCalledWith("- The client needs the website.\n- Due Friday.");
    expect(await screen.findByRole("button", { name: "Copied brief" })).toBeInTheDocument();

    rerender(<ThreadAssist {...props({ detail: summarized })} />);
    expect(screen.getByRole("button", { name: "Copy brief" })).toBeInTheDocument();
  });

  it("reports when the brief cannot be copied", async () => {
    const writeText = vi.fn().mockRejectedValue(new Error("clipboard unavailable"));
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    render(<ThreadAssist {...props({ detail: summarized })} />);

    fireEvent.click(screen.getByRole("button", { name: "Copy brief" }));

    expect(await screen.findByRole("status")).toHaveTextContent("Could not copy brief");
    expect(screen.getByRole("button", { name: "Copy brief" })).toBeInTheDocument();
  });

  it("stays read-only: no text entry or pickers in the AI section", () => {
    render(<ThreadAssist {...props({ detail: summarized, suggestions: { requested: true, proposals: [taskProposal] } })} />);
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
  });
});
