import { useCallback, useEffect, useMemo, useRef, useState, type Dispatch, type SetStateAction } from "react";
import { readAiRequestConfig } from "./aiSettings";
import { attachmentKey, type ChatAttachmentOption } from "./chatAttachments";
import { mailClient } from "./data/client";
import type { Account, ActionProposal, ScheduleEvent, SummaryResult, Thread, ThreadDetail } from "./domain";
import { errorMessage, logBackgroundFailure } from "./errors";
import { hasEmailedBefore, proactiveBriefSender, proactiveDwellMs } from "./proactiveBrief";
import { describeAnalysisError } from "./ThreadAssist";
import { sharedChatAttachments, type ChatEntry } from "./ThreadChat";
import { isSummaryStale } from "./threadPresentation";
import type { useAiAvailability } from "./useAiAvailability";
import type { useAppPreferences } from "./useAppPreferences";

/** Where a suggestion set lives: its state key, and the thread revision its saved copy is stored under. */
export type ProposalSource = { key: string; threadId: string; revision: string };

/** `record` without `key`, or `record` itself when it has no such entry. */
function omitKey<T>(record: Record<string, T>, key: string): Record<string, T> {
  if (!(key in record)) return record;
  const next = { ...record };
  delete next[key];
  return next;
}

// The action-analysis preview mirrors what the assistant is sent: the most
// recent messages, each body truncated.
const ACTION_ANALYSIS_MAX_MESSAGES = 15;
const ACTION_ANALYSIS_MAX_BODY_CHARS = 6000;

type Options = Pick<ReturnType<typeof useAiAvailability>, "aiProactive" | "aiSummaryAvailable" | "aiActionAvailable" | "aiActionFeatureEnabled"> & {
  accounts: Account[];
  selected: Thread | null;
  visibleDetail: ThreadDetail | null;
  isThreadMailbox: boolean;
  autoReadDelaySeconds: number;
  availabilityPreferences: ReturnType<typeof useAppPreferences>["availabilityPreferences"];
  setThreads: Dispatch<SetStateAction<Thread[]>>;
  setDetail: Dispatch<SetStateAction<ThreadDetail | null>>;
};

/** Owns per-conversation AI requests, cached suggestions, and session chat. */
export function useThreadIntelligence({
  accounts, selected, visibleDetail, isThreadMailbox, autoReadDelaySeconds,
  availabilityPreferences, setThreads, setDetail, aiProactive, aiSummaryAvailable,
  aiActionAvailable, aiActionFeatureEnabled,
}: Options) {
  // Keyed by thread id, not a single flag, so summarizing thread A in the
  // background doesn't show "Summarizing…" (or clear it) on thread B just
  // because B is what's currently on screen when A's request settles.
  const summarizingRef = useRef<Set<string>>(new Set());
  const [summarizingIds, setSummarizingIds] = useState<Set<string>>(new Set());
  const [summaryErrors, setSummaryErrors] = useState<Record<string, string>>({});
  /**
   * Always calls the provider, even when a summary is already cached — used
   * for both the first generation and an explicit "Regenerate". Guarded by
   * `summarizingRef` (checked and updated synchronously, not via state) so
   * pressing "i" or Regenerate repeatedly for the same thread while a
   * request is already in flight doesn't fire duplicate provider calls; a
   * different thread can still summarize concurrently in the background.
   */
  const applySummary = useCallback((threadId: string, result: SummaryResult) => {
    setThreads((current) =>
      current.map((thread) =>
        thread.id === threadId
          ? { ...thread, summary: result.summary, summaryGeneratedAt: result.generatedAt, summaryRevision: result.revision }
          : thread,
      ),
    );
    setDetail((current) =>
      current && current.thread.id === threadId
        ? {
            ...current,
            thread: { ...current.thread, summary: result.summary, summaryGeneratedAt: result.generatedAt, summaryRevision: result.revision },
          }
        : current,
    );
  }, [setDetail, setThreads]);

  /** Marks a thread's summary request as in flight; false when one already is. */
  const beginSummary = useCallback((threadId: string) => {
    if (summarizingRef.current.has(threadId)) return false;
    summarizingRef.current.add(threadId);
    setSummarizingIds(new Set(summarizingRef.current));
    setSummaryErrors((current) => omitKey(current, threadId));
    return true;
  }, []);

  const endSummary = useCallback((threadId: string) => {
    summarizingRef.current.delete(threadId);
    setSummarizingIds(new Set(summarizingRef.current));
  }, []);

  const runSummarize = useCallback(async () => {
    if (!selected) return;
    const threadId = selected.id;
    if (!beginSummary(threadId)) return;
    try {
      const { provider, model, endpoint, reasoning } = readAiRequestConfig("summarizing", "summary");
      applySummary(threadId, await mailClient.summarizeThread(threadId, provider, model, endpoint, reasoning));
    } catch (error) {
      setSummaryErrors((current) => ({
        ...current,
        [threadId]: errorMessage(error),
      }));
    } finally {
      endSummary(threadId);
    }
  }, [applySummary, beginSummary, endSummary, selected]);

  const [actionProposalSets, setActionProposalSets] = useState<Record<string, ActionProposal[]>>({});
  const [actionHiddenCounts, setActionHiddenCounts] = useState<Record<string, number>>({});
  // Revisions whose suggestions were fetched. Kept apart from the proposal
  // lists because chat can add proposals before suggestions ever ran.
  const [actionFetchedKeys, setActionFetchedKeys] = useState<ReadonlySet<string>>(() => new Set());
  const markSuggestionsFetched = useCallback((key: string) => {
    setActionFetchedKeys((current) => current.has(key) ? current : new Set(current).add(key));
  }, []);
  // Suggestion requests and their errors are tracked per thread revision, so
  // one conversation's request in flight or failure never shows on (or
  // blocks) another. The ref is the synchronous duplicate-request guard.
  const actionAnalysisLoadingRef = useRef(new Set<string>());
  const [actionAnalysisLoadingKeys, setActionAnalysisLoadingKeys] = useState<ReadonlySet<string>>(() => new Set());
  const [actionAnalysisErrors, setActionAnalysisErrors] = useState<Record<string, string>>({});
  const beginActionAnalysis = useCallback((key: string) => {
    if (actionAnalysisLoadingRef.current.has(key)) return false;
    actionAnalysisLoadingRef.current.add(key);
    setActionAnalysisLoadingKeys(new Set(actionAnalysisLoadingRef.current));
    setActionAnalysisErrors((current) => omitKey(current, key));
    return true;
  }, []);
  const endActionAnalysis = useCallback((key: string) => {
    actionAnalysisLoadingRef.current.delete(key);
    setActionAnalysisLoadingKeys(new Set(actionAnalysisLoadingRef.current));
  }, []);
  const actionProposalKey = visibleDetail
    ? `${visibleDetail.thread.id}:${visibleDetail.thread.lastMessageAt}`
    : null;
  const actionAnalysisLoading = Boolean(actionProposalKey && actionAnalysisLoadingKeys.has(actionProposalKey));
  const actionAnalysisError = actionProposalKey ? actionAnalysisErrors[actionProposalKey] ?? null : null;
  const actionProposalSource = useMemo<ProposalSource | null>(() => visibleDetail && actionProposalKey
    ? { key: actionProposalKey, threadId: visibleDetail.thread.id, revision: visibleDetail.thread.lastMessageAt }
    : null, [actionProposalKey, visibleDetail]);
  const actionProposals = useMemo(
    () => actionProposalKey ? actionProposalSets[actionProposalKey] ?? [] : [],
    [actionProposalKey, actionProposalSets],
  );
  const actionHiddenCount = actionProposalKey ? actionHiddenCounts[actionProposalKey] ?? 0 : 0;
  const actionAnalysisRequested = Boolean(
    actionAnalysisLoading
      || actionAnalysisError
      || (actionProposalKey && actionFetchedKeys.has(actionProposalKey)),
  );
  useEffect(() => {
    // A conversation's suggestion error is shown until the reader leaves it.
    if (!actionProposalKey) return;
    const key = actionProposalKey;
    return () => setActionAnalysisErrors((current) => omitKey(current, key));
  }, [actionProposalKey]);
  const actionAnalysisPreview = useMemo(() => {
    if (!visibleDetail) return null;
    const messages = visibleDetail.messages.slice(-ACTION_ANALYSIS_MAX_MESSAGES).map((message) => ({
      sourceMessageId: message.id,
      sender: message.sender,
      sentAt: message.sentAt,
      bodyText: message.bodyText.slice(0, ACTION_ANALYSIS_MAX_BODY_CHARS),
    }));
    return JSON.stringify({
      userTimeZone: availabilityPreferences.timeZone,
      emailContext: { subject: visibleDetail.thread.subject, messages },
    }, null, 2);
  }, [availabilityPreferences.timeZone, visibleDetail]);

  const runAnalyzeThread = useCallback(async () => {
    if (!visibleDetail || !actionProposalKey) return;
    const proposalKey = actionProposalKey;
    if (!aiActionAvailable) {
      const message = aiActionFeatureEnabled
        ? "Set up an AI provider and API key in AI settings to get suggestions."
        : "Turn on Suggestions in AI settings to get suggestions.";
      setActionAnalysisErrors((current) => ({ ...current, [proposalKey]: message }));
      return;
    }
    if (!beginActionAnalysis(proposalKey)) return;
    try {
      const { provider, model, endpoint } = readAiRequestConfig("getting suggestions", "actionExtraction");
      const { proposals, hiddenCount } = await mailClient.analyzeThread(
        visibleDetail.thread.id,
        availabilityPreferences.timeZone,
        provider,
        model,
        endpoint,
      );
      setActionProposalSets((current) => ({ ...current, [proposalKey]: proposals }));
      markSuggestionsFetched(proposalKey);
      setActionHiddenCounts((current) => ({ ...current, [proposalKey]: hiddenCount }));
    } catch (reason) {
      setActionAnalysisErrors((current) => ({ ...current, [proposalKey]: errorMessage(reason) }));
    } finally {
      endActionAnalysis(proposalKey);
    }
  }, [
    markSuggestionsFetched, actionProposalKey, aiActionAvailable, aiActionFeatureEnabled,
    availabilityPreferences.timeZone, beginActionAnalysis, endActionAnalysis, visibleDetail,
  ]);

  /** Summarizes and extracts suggestions in one provider call. */
  const runCombinedBrief = useCallback(async () => {
    if (!visibleDetail || !actionProposalKey) return;
    const threadId = visibleDetail.thread.id;
    const proposalKey = actionProposalKey;
    if (actionAnalysisLoadingRef.current.has(proposalKey) || !beginSummary(threadId)) return;
    beginActionAnalysis(proposalKey);
    try {
      const { provider, model, endpoint } = readAiRequestConfig("getting a brief", "brief");
      const { summary, analysis } = await mailClient.briefThread(threadId, availabilityPreferences.timeZone, provider, model, endpoint);
      applySummary(threadId, summary);
      setActionProposalSets((current) => ({ ...current, [proposalKey]: analysis.proposals }));
      markSuggestionsFetched(proposalKey);
      setActionHiddenCounts((current) => ({ ...current, [proposalKey]: analysis.hiddenCount }));
    } catch (error) {
      setSummaryErrors((current) => ({ ...current, [threadId]: errorMessage(error) }));
    } finally {
      endSummary(threadId);
      endActionAnalysis(proposalKey);
    }
  }, [
    markSuggestionsFetched, actionProposalKey, applySummary, availabilityPreferences.timeZone,
    beginActionAnalysis, beginSummary, endActionAnalysis, endSummary, visibleDetail,
  ]);

  /**
   * Fetches whatever part of the brief is missing for the visible thread, in
   * one provider call when both the summary and the suggestions are needed.
   * `only` requires that part to be missing before calling the provider at
   * all; `force` regenerates every enabled part.
   */
  const runBrief = useCallback(async ({ force = false, only }: { force?: boolean; only?: "summary" | "suggestions" } = {}) => {
    if (!visibleDetail || !actionProposalKey) return;
    const { thread } = visibleDetail;
    const summaryFresh = Boolean(thread.summary) && !isSummaryStale(thread);
    const needSummary = aiSummaryAvailable && (force || !summaryFresh);
    const needSuggestions = aiActionAvailable
      && (force || !actionFetchedKeys.has(actionProposalKey));
    if (only === "summary" ? !needSummary : only === "suggestions" ? !needSuggestions : !needSummary && !needSuggestions) return;
    if (needSummary && needSuggestions) await runCombinedBrief();
    else if (needSummary) await runSummarize();
    else await runAnalyzeThread();
  }, [
    actionFetchedKeys, actionProposalKey, aiActionAvailable, aiSummaryAvailable, runAnalyzeThread,
    runCombinedBrief, runSummarize, visibleDetail,
  ]);

  // Proactive briefs: once the reader stays on a qualifying conversation for
  // the mark-read delay, fetch whatever part of the brief is missing. Each
  // conversation revision is attempted at most once per session, so a
  // failure is not retried automatically.
  const runBriefRef = useRef(runBrief);
  runBriefRef.current = runBrief;
  const proactiveAttempted = useRef(new Set<string>());
  const proactiveDetailRef = useRef(visibleDetail);
  proactiveDetailRef.current = visibleDetail;
  const proactiveKey = visibleDetail ? `${visibleDetail.thread.id}:${visibleDetail.thread.lastMessageAt}` : null;
  const ownAddresses = useMemo(() => new Set(accounts.map((account) => account.email.toLocaleLowerCase())), [accounts]);
  useEffect(() => {
    const detail = proactiveDetailRef.current;
    if (!aiProactive.enabled || !(aiSummaryAvailable || aiActionAvailable) || !isThreadMailbox || !detail || !proactiveKey) return;
    if (proactiveAttempted.current.has(proactiveKey)) return;
    const sender = proactiveBriefSender(detail, ownAddresses);
    if (!sender) return;
    let active = true;
    const timer = window.setTimeout(() => {
      void (async () => {
        if (aiProactive.knownSendersOnly && !(await hasEmailedBefore(sender).catch(() => false))) return;
        if (!active) return;
        proactiveAttempted.current.add(proactiveKey);
        await runBriefRef.current();
      })();
    }, proactiveDwellMs(autoReadDelaySeconds));
    return () => { active = false; window.clearTimeout(timer); };
  }, [aiActionAvailable, aiProactive, aiSummaryAvailable, autoReadDelaySeconds, isThreadMailbox, ownAddresses, proactiveKey]);

  // Thread chat, kept per conversation for the session. A failed question is
  // removed from the transcript and offered again through Try Again.
  const [chatByThread, setChatByThread] = useState<Record<string, ChatEntry[]>>({});
  const [chatPendingThreads, setChatPendingThreads] = useState<ReadonlySet<string>>(() => new Set());
  const [chatFailures, setChatFailures] = useState<Record<string, { message: string; question: string; searchMailbox: boolean; attachments: ChatAttachmentOption[] }>>({});
  const [chatFocusRequest, setChatFocusRequest] = useState(0);
  const chatEntrySequence = useRef(0);
  const chatSuggestionOrigins = useRef(new WeakMap<ActionProposal, { threadId: string; entryId: string }>());
  const askThread = useCallback(async (question: string, searchMailbox: boolean, contactId: string | null, attachments: ChatAttachmentOption[]) => {
    if (!visibleDetail || !actionProposalKey) return;
    const threadId = visibleDetail.thread.id;
    const proposalKey = actionProposalKey;
    if (chatPendingThreads.has(threadId)) return;
    const earlier = chatByThread[threadId] ?? [];
    // Attachments stay shared for the rest of the chat so follow-up
    // questions can still see them.
    const shared = new Map<string, ChatAttachmentOption>();
    for (const attachment of [...sharedChatAttachments(earlier), ...attachments]) shared.set(attachmentKey(attachment), attachment);
    const nextId = () => `chat-${chatEntrySequence.current += 1}`;
    const questionEntry: ChatEntry = { id: nextId(), role: "user", content: question, searchMailbox, attachments };
    setChatByThread((current) => ({ ...current, [threadId]: [...(current[threadId] ?? []), questionEntry] }));
    setChatPendingThreads((current) => new Set(current).add(threadId));
    setChatFailures((current) => {
      if (!(threadId in current)) return current;
      const next = { ...current };
      delete next[threadId];
      return next;
    });
    try {
      const { provider, model, endpoint } = readAiRequestConfig("asking about a conversation", "threadChat");
      const reply = await mailClient.threadChat({
        threadId,
        question,
        history: earlier.map((entry) => ({ role: entry.role, content: entry.content })),
        searchMailbox,
        includeProposals: aiActionAvailable,
        contactId,
        userTimeZone: availabilityPreferences.timeZone,
        attachments: [...shared.values()].map(({ messageId, attachmentId }) => ({ messageId, attachmentId })),
      }, provider, model, endpoint);
      if (reply.analysis.proposals.length > 0) {
        setActionProposalSets((current) => ({ ...current, [proposalKey]: [...(current[proposalKey] ?? []), ...reply.analysis.proposals] }));
      }
      const answer: ChatEntry = {
        id: nextId(),
        role: "assistant",
        content: reply.answer,
        replyDraft: reply.replyDraft,
        addedSuggestions: reply.analysis.proposals.length,
        hiddenSuggestions: reply.analysis.hiddenCount,
        availability: reply.availability,
        sources: reply.sources,
        searched: reply.searched,
        attachments: reply.attachments,
      };
      for (const proposal of reply.analysis.proposals) {
        chatSuggestionOrigins.current.set(proposal, { threadId, entryId: answer.id });
      }
      setChatByThread((current) => ({ ...current, [threadId]: [...(current[threadId] ?? []), answer] }));
    } catch (error) {
      setChatByThread((current) => ({ ...current, [threadId]: (current[threadId] ?? []).filter((entry) => entry.id !== questionEntry.id) }));
      setChatFailures((current) => ({ ...current, [threadId]: { message: describeAnalysisError(errorMessage(error)).summary, question, searchMailbox, attachments } }));
    } finally {
      setChatPendingThreads((current) => {
        const next = new Set(current);
        next.delete(threadId);
        return next;
      });
    }
  }, [actionProposalKey, aiActionAvailable, availabilityPreferences.timeZone, chatByThread, chatPendingThreads, visibleDetail]);

  // An edited suggestion is a new object; remember the one it replaced, back
  // to the copy the provider returned, so the saved copy can still be matched.
  const proposalOrigins = useRef(new WeakMap<ActionProposal, ActionProposal>());
  const updateActionProposal = useCallback((index: number, proposal: ActionProposal) => {
    if (!actionProposalKey) return;
    setActionProposalSets((current) => ({
      ...current,
      [actionProposalKey]: (current[actionProposalKey] ?? []).map((item, itemIndex) => {
        if (itemIndex !== index) return item;
        proposalOrigins.current.set(proposal, proposalOrigins.current.get(item) ?? item);
        return proposal;
      }),
    }));
  }, [actionProposalKey]);

  // Removes a handled suggestion by identity, so it still matches if the list
  // changed while a dialog was open, and drops it from the saved suggestions
  // so reopening the thread doesn't offer it again.
  const removeActionProposal = useCallback((source: ProposalSource, proposal: ActionProposal, createdEvent?: ScheduleEvent) => {
    setActionProposalSets((current) => ({ ...current, [source.key]: (current[source.key] ?? []).filter((item) => item !== proposal) }));
    const saved = proposalOrigins.current.get(proposal) ?? proposal;
    const chatOrigin = chatSuggestionOrigins.current.get(saved);
    if (chatOrigin) {
      setChatByThread((current) => ({
        ...current,
        [chatOrigin.threadId]: (current[chatOrigin.threadId] ?? []).map((entry) =>
          entry.role === "assistant" && entry.id === chatOrigin.entryId ? {
            ...entry,
            addedSuggestions: Math.max(0, entry.addedSuggestions - 1),
            handledSuggestions: (entry.handledSuggestions ?? 0) + 1,
            calendarEvents: createdEvent ? [...(entry.calendarEvents ?? []), createdEvent] : entry.calendarEvents,
          } : entry),
      }));
    }
    mailClient.removeThreadSuggestion(source.threadId, source.revision, saved)
      .catch(logBackgroundFailure("Saving a handled suggestion"));
  }, []);

  const discardActionProposal = useCallback((index: number) => {
    const proposal = actionProposals[index];
    if (actionProposalSource && proposal) removeActionProposal(actionProposalSource, proposal);
  }, [actionProposalSource, actionProposals, removeActionProposal]);

  const summaryPending = visibleDetail ? summarizingIds.has(visibleDetail.thread.id) : false;
  const summaryError = visibleDetail ? summaryErrors[visibleDetail.thread.id] ?? null : null;
  const focusThreadChat = useCallback(() => setChatFocusRequest((current) => current + 1), []);
  return {
    summaryPending, summaryError, runBrief, actionProposals,
    actionProposalSource, actionAnalysisLoading, actionAnalysisError, actionAnalysisRequested,
    actionAnalysisPreview, actionHiddenCount, chatByThread, chatPendingThreads, chatFailures,
    chatFocusRequest, askThread, focusThreadChat, updateActionProposal, removeActionProposal,
    discardActionProposal, ownAddresses,
  };
}
