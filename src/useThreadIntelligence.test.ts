import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { saveAiProvider } from "./aiSettings";
import type { ChatAttachmentOption } from "./chatAttachments";
import { mailClient } from "./data/client";
import type { Account, ThreadDetail } from "./domain";
import { proactiveDwellMs } from "./proactiveBrief";
import { DEFAULT_AVAILABILITY_PREFERENCES } from "./settings";
import { useThreadIntelligence } from "./useThreadIntelligence";

const briefResult = {
  summary: { summary: "- Welcome to the app.\n- Nothing is due.", generatedAt: "2099-01-01T00:00:00Z", revision: "2099-01-01T00:00:00Z" },
  analysis: { proposals: [], hiddenCount: 0 },
};

const taskProposal = {
  type: "task" as const, kind: "action" as const, title: "Try the command palette", notes: null,
  dueKind: "none" as const, dueValue: null, timeZone: null, repeatIntervalDays: null, confidence: 0.9,
  evidence: { sourceMessageId: "welcome-message", excerpt: "command palette" },
};

const chatReply = (answer: string, proposals: typeof taskProposal[] = []) => ({
  answer, analysis: { proposals, hiddenCount: 0 }, replyDraft: null, sources: [], searched: [], attachments: [], availability: null,
});

type Props = {
  detail: ThreadDetail | null;
  summary?: boolean;
  suggestions?: boolean;
  proactive?: { enabled: boolean; knownSendersOnly: boolean };
  accounts?: Account[];
};

function renderIntelligence(initial: Props) {
  const applyThreadSummary = vi.fn();
  const view = renderHook(({ detail, summary = true, suggestions = true, proactive = { enabled: false, knownSendersOnly: false }, accounts = [] }: Props) =>
    useThreadIntelligence({
      accounts,
      selected: detail?.thread ?? null,
      visibleDetail: detail,
      isThreadMailbox: true,
      autoReadDelaySeconds: 2,
      availabilityPreferences: DEFAULT_AVAILABILITY_PREFERENCES,
      applyThreadSummary,
      aiProactive: proactive,
      aiSummaryAvailable: summary,
      aiActionAvailable: suggestions,
      aiActionFeatureEnabled: suggestions,
    }), { initialProps: initial });
  return { ...view, applyThreadSummary };
}

describe("useThreadIntelligence", () => {
  let welcome: ThreadDetail;
  let roadmap: ThreadDetail;

  beforeEach(async () => {
    saveAiProvider("openai");
    welcome = await mailClient.getThread("welcome");
    roadmap = await mailClient.getThread("roadmap");
  });

  afterEach(() => {
    saveAiProvider("none");
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  describe("briefs", () => {
    it("keeps offered scheduling options independently of task proposals and scopes them to the thread revision", async () => {
      const offeredTimes = { sourceMessageId: "welcome-message", times: [{ sourceText: "Tuesday at 7 PM", localStart: "2099-01-06T19:00", durationMinutes: 60, timeZone: null }] };
      vi.spyOn(mailClient, "briefThread").mockResolvedValue({ ...briefResult, offeredTimes, analysis: { proposals: [taskProposal], hiddenCount: 0 } });
      const { result, rerender } = renderIntelligence({ detail: welcome });
      await act(() => result.current.runBrief());
      expect(result.current.offeredTimes).toEqual(offeredTimes);
      expect(result.current.actionProposals).toEqual([taskProposal]);
      rerender({ detail: roadmap });
      expect(result.current.offeredTimes).toBeNull();
      rerender({ detail: { ...welcome, thread: { ...welcome.thread, lastMessageAt: "2099-02-01T00:00:00Z" } } });
      expect(result.current.offeredTimes).toBeNull();
    });

    it("gets the summary and suggestions in one request, then only calls again on refresh", async () => {
      const briefThread = vi.spyOn(mailClient, "briefThread").mockResolvedValue(briefResult);
      const summarizeThread = vi.spyOn(mailClient, "summarizeThread");
      const analyzeThread = vi.spyOn(mailClient, "analyzeThread");
      const { result, rerender, applyThreadSummary } = renderIntelligence({ detail: welcome });

      await act(() => result.current.runBrief());
      expect(briefThread).toHaveBeenCalledTimes(1);
      expect(applyThreadSummary).toHaveBeenCalledWith("welcome", briefResult.summary);
      expect(result.current.actionAnalysisRequested).toBe(true);

      // Once the saved summary is on the thread, nothing is missing.
      const summarized = { ...welcome, thread: { ...welcome.thread, summary: briefResult.summary.summary,
        summaryGeneratedAt: briefResult.summary.generatedAt, summaryRevision: welcome.thread.lastMessageAt } };
      rerender({ detail: summarized });
      await act(() => result.current.runBrief());
      await act(() => result.current.runBrief({ only: "summary" }));
      await act(() => result.current.runBrief({ only: "suggestions" }));
      expect(briefThread).toHaveBeenCalledTimes(1);
      expect(summarizeThread).not.toHaveBeenCalled();
      expect(analyzeThread).not.toHaveBeenCalled();

      await act(() => result.current.runBrief({ force: true }));
      expect(briefThread).toHaveBeenCalledTimes(2);
    });

    it("uses the single-purpose request when only one AI feature is on", async () => {
      const briefThread = vi.spyOn(mailClient, "briefThread");
      const analyzeThread = vi.spyOn(mailClient, "analyzeThread").mockResolvedValue({ proposals: [], hiddenCount: 1 });
      const { result } = renderIntelligence({ detail: welcome, summary: false });

      await act(() => result.current.runBrief());

      expect(analyzeThread).toHaveBeenCalledTimes(1);
      expect(briefThread).not.toHaveBeenCalled();
      expect(result.current.actionHiddenCount).toBe(1);
    });

    it("keeps one conversation's pending or failed suggestions from showing on another", async () => {
      let failFirst: (reason: Error) => void = () => {};
      const analyzeThread = vi.spyOn(mailClient, "analyzeThread").mockImplementation((threadId) => threadId === "welcome"
        ? new Promise((_, reject) => { failFirst = reject; })
        : Promise.resolve({ proposals: [], hiddenCount: 0 }));
      const { result, rerender } = renderIntelligence({ detail: welcome, summary: false });

      let first!: Promise<void>;
      act(() => { first = result.current.runBrief(); });
      expect(result.current.actionAnalysisLoading).toBe(true);

      // Open another conversation while the first request is still running.
      rerender({ detail: roadmap, summary: false });
      expect(result.current.actionAnalysisLoading).toBe(false);
      await act(() => result.current.runBrief());
      expect(analyzeThread).toHaveBeenCalledTimes(2);
      expect(analyzeThread.mock.calls[1]?.[0]).toBe("roadmap");
      expect(result.current.actionAnalysisRequested).toBe(true);

      // The first conversation's failure stays with it.
      await act(async () => {
        failFirst(new Error("Suggestions for the welcome thread failed"));
        await first;
      });
      expect(result.current.actionAnalysisError).toBeNull();
    });

    it("keeps a failed brief retryable", async () => {
      const briefThread = vi.spyOn(mailClient, "briefThread")
        .mockRejectedValueOnce(new Error("The AI provider returned malformed thread brief JSON"))
        .mockResolvedValueOnce(briefResult);
      const { result, applyThreadSummary } = renderIntelligence({ detail: welcome });

      await act(() => result.current.runBrief());
      expect(result.current.summaryError).toBe("The AI provider returned malformed thread brief JSON");
      expect(result.current.summaryPending).toBe(false);

      await act(() => result.current.runBrief());
      expect(briefThread).toHaveBeenCalledTimes(2);
      expect(result.current.summaryError).toBeNull();
      expect(applyThreadSummary).toHaveBeenCalledWith("welcome", briefResult.summary);
    });
  });

  describe("proactive briefs", () => {
    const dwell = proactiveDwellMs(2);
    const proactive = { enabled: true, knownSendersOnly: false };

    beforeEach(() => {
      vi.useFakeTimers({ shouldAdvanceTime: true });
    });

    it("prepares the brief once after the reader stays on a qualifying conversation", async () => {
      const briefThread = vi.spyOn(mailClient, "briefThread").mockResolvedValue(briefResult);
      const { rerender, applyThreadSummary } = renderIntelligence({ detail: welcome, proactive });

      // The welcome message offers unsubscribe, so it is treated as a mailing list.
      await vi.advanceTimersByTimeAsync(dwell + 1_000);
      expect(briefThread).not.toHaveBeenCalled();

      rerender({ detail: roadmap, proactive });
      await vi.advanceTimersByTimeAsync(dwell - 1_000);
      expect(briefThread).not.toHaveBeenCalled();
      await vi.advanceTimersByTimeAsync(1_500);
      await waitFor(() => expect(briefThread).toHaveBeenCalledTimes(1));
      expect(briefThread.mock.calls[0][0]).toBe("roadmap");
      await waitFor(() => expect(applyThreadSummary).toHaveBeenCalledWith("roadmap", briefResult.summary));

      await vi.advanceTimersByTimeAsync(dwell * 3);
      expect(briefThread).toHaveBeenCalledTimes(1);
    });

    it("does nothing for a conversation left before the dwell elapses", async () => {
      const briefThread = vi.spyOn(mailClient, "briefThread").mockResolvedValue(briefResult);
      const { rerender } = renderIntelligence({ detail: roadmap, proactive });
      await vi.advanceTimersByTimeAsync(dwell - 1_000);
      rerender({ detail: welcome, proactive });
      await vi.advanceTimersByTimeAsync(dwell * 2);
      expect(briefThread).not.toHaveBeenCalled();
    });

    it("waits for a sender the user has emailed when that filter is on", async () => {
      const briefThread = vi.spyOn(mailClient, "briefThread").mockResolvedValue(briefResult);
      const contacts = vi.spyOn(mailClient, "listContactProfiles").mockResolvedValue([]);
      renderIntelligence({ detail: roadmap, proactive: { enabled: true, knownSendersOnly: true } });
      await vi.advanceTimersByTimeAsync(dwell * 2);
      expect(contacts).toHaveBeenCalled();
      expect(briefThread).not.toHaveBeenCalled();
    });

    it("does not brief a conversation right after the user's own reply", async () => {
      const briefThread = vi.spyOn(mailClient, "briefThread").mockResolvedValue(briefResult);
      const [account] = await mailClient.listAccounts();
      const newest = roadmap.messages.at(-1)!;
      const replied = { ...roadmap, messages: [...roadmap.messages,
        { ...newest, id: "my-reply", sender: `Me <${account.email.toUpperCase()}>`, bodyText: "Happy to talk. I am free Tuesday at 10am." }] };
      renderIntelligence({ detail: replied, proactive, accounts: [account] });
      await vi.advanceTimersByTimeAsync(dwell * 2);
      expect(briefThread).not.toHaveBeenCalled();
    });

    it("stays on request only while proactive suggestions are off", async () => {
      const briefThread = vi.spyOn(mailClient, "briefThread").mockResolvedValue(briefResult);
      renderIntelligence({ detail: roadmap });
      await vi.advanceTimersByTimeAsync(dwell * 2);
      expect(briefThread).not.toHaveBeenCalled();
    });
  });

  describe("suggestions", () => {
    it("drops discarded and added suggestions from the saved copy, matching the original after an edit", async () => {
      const discardMe = { ...taskProposal, title: "Star a conversation" };
      vi.spyOn(mailClient, "analyzeThread").mockResolvedValue({ proposals: [taskProposal, discardMe], hiddenCount: 0 });
      const remove = vi.spyOn(mailClient, "removeThreadSuggestion").mockResolvedValue(true);
      const { result } = renderIntelligence({ detail: welcome, summary: false });
      const revision = welcome.thread.lastMessageAt;
      await act(() => result.current.runBrief());

      act(() => result.current.discardActionProposal(1));
      expect(remove).toHaveBeenCalledWith("welcome", revision, discardMe);
      expect(result.current.actionProposals.map((proposal) => proposal.title)).toEqual(["Try the command palette"]);

      // Edit first, then add: the saved copy still holds the provider's original wording.
      act(() => result.current.updateActionProposal(0, { ...taskProposal, title: "Open the palette with Cmd+K" }));
      const edited = result.current.actionProposals[0]!;
      expect(edited.title).toBe("Open the palette with Cmd+K");
      act(() => result.current.removeActionProposal(result.current.actionProposalSource!, edited));

      expect(remove).toHaveBeenLastCalledWith("welcome", revision, taskProposal);
      expect(remove).toHaveBeenCalledTimes(2);
      expect(result.current.actionProposals).toEqual([]);
    });
  });

  describe("thread chat", () => {
    const shortcuts: ChatAttachmentOption = {
      messageId: "welcome-message", attachmentId: "demo-guide", filename: "threestrands-shortcuts.txt",
    } as ChatAttachmentOption;

    it("updates only the originating chat answer as suggestions are edited and discarded", async () => {
      const second = { ...taskProposal, title: "Another task" };
      vi.spyOn(mailClient, "threadChat")
        .mockResolvedValueOnce(chatReply("Two suggestions.", [taskProposal, second]))
        .mockResolvedValueOnce(chatReply("One more suggestion.", [{ ...second, title: "Separate answer task" }]));
      const remove = vi.spyOn(mailClient, "removeThreadSuggestion").mockResolvedValue(true);
      const { result } = renderIntelligence({ detail: welcome, summary: false });
      await act(() => result.current.askThread("Suggest tasks", false, null, []));
      await act(() => result.current.askThread("Suggest another", false, null, []));
      const answers = () => (result.current.chatByThread.welcome ?? []).filter((entry) => entry.role === "assistant");
      expect(answers().map((entry) => entry.role === "assistant" && entry.addedSuggestions)).toEqual([2, 1]);

      act(() => result.current.updateActionProposal(0, { ...taskProposal, title: "Edited task" }));
      act(() => result.current.discardActionProposal(0));
      expect(answers().map((entry) => entry.role === "assistant" && entry.addedSuggestions)).toEqual([1, 1]);
      expect(remove).toHaveBeenCalledWith("welcome", expect.any(String), taskProposal);

      const another = result.current.actionProposals.findIndex((proposal) => proposal.title === "Another task");
      act(() => result.current.discardActionProposal(another));
      expect(answers().map((entry) => entry.role === "assistant" && [entry.addedSuggestions, entry.handledSuggestions ?? 0]))
        .toEqual([[0, 2], [1, 0]]);
    });

    it("leaves proposals out of the request when Suggestions is off and offers the question again after a failure", async () => {
      const threadChat = vi.spyOn(mailClient, "threadChat")
        .mockRejectedValueOnce(new Error("error sending request for url"))
        .mockResolvedValueOnce(chatReply("Answered."));
      const { result } = renderIntelligence({ detail: welcome, summary: false, suggestions: false });

      await act(() => result.current.askThread("Anything due?", false, null, []));
      expect(threadChat).toHaveBeenLastCalledWith(expect.objectContaining({ includeProposals: false }), "openai", expect.any(String), null);
      expect(result.current.chatFailures.welcome).toEqual(expect.objectContaining({
        message: "Couldn't reach the AI provider. Check your connection and try again.", question: "Anything due?",
      }));
      expect(result.current.chatByThread.welcome).toEqual([]);

      await act(() => result.current.askThread("Anything due?", false, null, []));
      expect(threadChat).toHaveBeenLastCalledWith(expect.objectContaining({ question: "Anything due?", history: [] }), "openai", expect.any(String), null);
      expect(result.current.chatFailures.welcome).toBeUndefined();
      expect(result.current.chatByThread.welcome?.at(-1)?.content).toBe("Answered.");
    });

    it("keeps a shared attachment shared for follow-up questions", async () => {
      const threadChat = vi.spyOn(mailClient, "threadChat").mockResolvedValue(chatReply("It lists the shortcuts."));
      const { result } = renderIntelligence({ detail: welcome });

      await act(() => result.current.askThread("Summarize @threestrands-shortcuts.txt", false, null, [shortcuts]));
      expect(threadChat).toHaveBeenLastCalledWith(expect.objectContaining({
        question: "Summarize @threestrands-shortcuts.txt",
        attachments: [{ messageId: "welcome-message", attachmentId: "demo-guide" }],
      }), "openai", expect.any(String), null);

      await act(() => result.current.askThread("Which one archives?", false, null, []));
      expect(threadChat).toHaveBeenLastCalledWith(expect.objectContaining({
        question: "Which one archives?",
        attachments: [{ messageId: "welcome-message", attachmentId: "demo-guide" }],
      }), "openai", expect.any(String), null);
    });

    it("shares no attachments unless one is chosen", async () => {
      const threadChat = vi.spyOn(mailClient, "threadChat").mockResolvedValue(chatReply("Answered."));
      const { result } = renderIntelligence({ detail: welcome });
      await act(() => result.current.askThread("What does threestrands-shortcuts.txt say?", false, null, []));
      expect(threadChat).toHaveBeenLastCalledWith(expect.objectContaining({ attachments: [] }), "openai", expect.any(String), null);
    });
  });
});
