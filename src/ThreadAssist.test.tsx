import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
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

function props(overrides: Partial<Omit<ThreadAssistProps, "summary" | "suggestions">> & {
  summary?: Partial<ThreadAssistProps["summary"]>;
  suggestions?: Partial<ThreadAssistProps["suggestions"]>;
} = {}): ThreadAssistProps {
  const { summary, suggestions, ...rest } = overrides;
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
      onFindTimes: vi.fn(),
      ...suggestions,
    },
  };
}

describe("ThreadAssist", () => {
  afterEach(cleanup);

  it("offers one Get Brief action before anything has run and refreshes everything afterwards", () => {
    const onRun = vi.fn();
    const { rerender } = render(<ThreadAssist {...props({ onRun })} />);
    expect(screen.getByRole("heading", { name: "Brief" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Get Brief" }));
    expect(onRun).toHaveBeenLastCalledWith(false);
    expect(screen.queryByRole("button", { name: "Refresh brief" })).not.toBeInTheDocument();

    rerender(<ThreadAssist {...props({ onRun, detail: summarized, suggestions: { requested: true } })} />);
    expect(screen.getByText("The client needs the website.")).toBeInTheDocument();
    expect(screen.getByText("Due Friday.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Get Brief" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Refresh brief" }));
    expect(onRun).toHaveBeenLastCalledWith(true);
  });

  it("asks only for the missing suggestions when a summary was saved earlier", () => {
    const onRun = vi.fn();
    render(<ThreadAssist {...props({ onRun, detail: summarized })} />);
    expect(screen.queryByRole("button", { name: "Get Brief" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Get Suggestions" }));
    expect(onRun).toHaveBeenCalledWith(false);
  });

  it("flags a summary that predates the newest message", () => {
    render(<ThreadAssist {...props({ detail: { ...summarized, thread: { ...summarized.thread, lastMessageAt: "2026-09-20T09:00:00Z" } } })} />);
    expect(screen.getByText("New messages since this brief.")).toBeInTheDocument();
  });

  it("titles the section Suggestions when summaries are off", () => {
    render(<ThreadAssist {...props({ detail: summarized, summary: { enabled: false, available: false } })} />);
    expect(screen.getByRole("heading", { name: "Suggestions" })).toBeInTheDocument();
    expect(screen.queryByText("The client needs the website.")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Get Suggestions" })).toBeInTheDocument();
  });

  it("shows progress without offering to run again while a request is in flight", () => {
    render(<ThreadAssist {...props({ loading: true })} />);
    expect(screen.getByRole("status")).toHaveTextContent("Reading the conversation…");
    expect(screen.queryByRole("button", { name: "Get Brief" })).not.toBeInTheDocument();
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

  it("flags uncertain suggestions for a closer look instead of showing a confidence score", () => {
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
    const onFindTimes = vi.fn();
    render(<ThreadAssist {...props({ suggestions: { requested: true, proposals: [meeting], onFindTimes } })} />);

    expect(screen.getByText("Check details")).toBeInTheDocument();
    expect(screen.queryByText(/%/)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Find Times" })).toBeDisabled();
    fireEvent.click(screen.getByText("From the email"));
    expect(screen.queryByText(/message-unknown/)).not.toBeInTheDocument();
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

  it("stays read-only: no text entry or pickers in the AI section", () => {
    render(<ThreadAssist {...props({ detail: summarized, suggestions: { requested: true, proposals: [taskProposal] } })} />);
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
  });
});
