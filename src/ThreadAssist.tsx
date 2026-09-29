import { useState } from "react";
import { Check, Copy, Pencil, RefreshCw, RotateCcw, Sparkles } from "lucide-react";
import type { ActionProposal, AvailabilityCandidate, AvailabilityPreferences, MeetingProposal, ThreadDetail } from "./domain";
import { MeetingScheduler, type ScheduleSlot } from "./MeetingScheduler";
import { planMeeting } from "./scheduling";
import { HoverTooltip } from "./AppChrome";
import { parseAddress } from "./emailAddress";

export const THREAD_ASSIST_ID = "thread-assist";

/**
 * AI brief and suggestions for the conversation. `enabled` reflects the
 * feature flag; `available` additionally requires a configured provider key.
 */
export type ThreadAssistProps = {
  detail: ThreadDetail;
  summary: { enabled: boolean; available: boolean; pending: boolean };
  suggestions: {
    enabled: boolean;
    available: boolean;
    /** Whether suggestions were fetched for this revision; chat may add some without it. */
    requested: boolean;
    proposals: ActionProposal[];
    hiddenCount: number;
    onDiscard(index: number): void;
    onReview(index: number, proposal: ActionProposal, intent: "edit" | "accept"): void;
  };
  /** Calendar checks for meeting suggestions; every outcome is reviewed elsewhere. */
  scheduling: {
    calendarConnected: boolean;
    preferences: AvailabilityPreferences;
    onAddToCalendar(index: number, proposal: MeetingProposal, slot: ScheduleSlot): void;
    onReplyWithTimes(slots: AvailabilityCandidate[]): void;
    onConfirmTime(slot: ScheduleSlot): void;
    onMoreTimes(day: Date, durationMinutes: number): void;
    onOpenCalendarSettings(): void;
  };
  loading: boolean;
  error: string | null;
  /** The exact bounded content sent for analysis, shown once suggestions ran. */
  preview: string | null;
  /** Runs whatever is missing, or everything when `force` is set. */
  onRun(force: boolean): void;
  onOpenSettings(): void;
};

export function describeAnalysisError(message: string): { summary: string; retryable: boolean } {
  if (/^(the )?ai provider returned/i.test(message) || /^the ai provider (cited|included)/i.test(message)) {
    return {
      summary: "The AI's response couldn't be read. Try again, or choose a different model in AI settings.",
      retryable: true,
    };
  }
  if (/error sending request|timed out|connection|dns/i.test(message)) {
    return { summary: "Couldn't reach the AI provider. Check your connection and try again.", retryable: true };
  }
  return { summary: message, retryable: false };
}

export function summaryLines(summary: string): string[] {
  return summary
    .split("\n")
    .map((line) => line.replace(/^[-•]\s*/, "").trim())
    .filter(Boolean);
}

export function summaryClipboardText(summary: string): string {
  return summaryLines(summary)
    .map((line) => `- ${line}`)
    .join("\n");
}

export function ThreadAssist({ detail, summary, suggestions, scheduling, loading, error, preview, onRun, onOpenSettings }: ThreadAssistProps) {
  const { thread } = detail;
  const [copiedSummary, setCopiedSummary] = useState<string | null>(null);
  const [copyFailed, setCopyFailed] = useState(false);
  if (!summary.enabled && !suggestions.enabled) {
    return <section id={THREAD_ASSIST_ID} className="context-section thread-assist" aria-label="Brief">
      <p className="context-status">AI briefs and suggestions are off.</p>
      <button type="button" className="context-link-button" onClick={onOpenSettings}>AI Settings</button>
    </section>;
  }
  if (!summary.available && !suggestions.available) {
    return <section id={THREAD_ASSIST_ID} className="context-section thread-assist" aria-label="Brief">
      <p className="context-status">Set up an AI provider and API key in AI settings to get a brief of this conversation.</p>
      <button type="button" className="context-link-button" onClick={onOpenSettings}>AI Settings</button>
    </section>;
  }

  const title = summary.available ? "Brief" : "Suggestions";
  const summaryText = summary.available ? thread.summary : null;
  const copied = Boolean(summaryText) && copiedSummary === summaryText;
  const stale = Boolean(summaryText && thread.summaryGeneratedAt && thread.lastMessageAt > thread.summaryGeneratedAt);
  const busy = loading || summary.pending;
  const generated = Boolean(summaryText) || suggestions.requested;
  const showSuggestions = suggestions.available && (suggestions.requested || suggestions.proposals.length > 0);
  const missingSummary = summary.available && !summaryText;
  const missingSuggestions = suggestions.available && !suggestions.requested;
  const failure = error ? describeAnalysisError(error) : null;
  const messageSource = (messageId: string) => {
    const message = detail.messages.find((candidate) => candidate.id === messageId);
    if (!message) return null;
    const sender = parseAddress(message.sender);
    return `${sender.name || sender.email} · ${new Date(message.sentAt).toLocaleDateString(undefined, { month: "short", day: "numeric" })}`;
  };
  const copyBrief = async () => {
    if (!summaryText) return;
    setCopyFailed(false);
    try {
      await navigator.clipboard.writeText(summaryClipboardText(summaryText));
      setCopiedSummary(summaryText);
    } catch {
      setCopyFailed(true);
    }
  };

  return <section id={THREAD_ASSIST_ID} className="context-section thread-assist" aria-labelledby="thread-assist-heading">
    <header className="context-section-header">
      <h3 id="thread-assist-heading"><Sparkles size={13} />{title}</h3>
      {busy ? <span className="context-status" role="status">Reading the conversation…</span> : (
        <div className="context-section-header-actions">
          {summaryText ? (
            <HoverTooltip title={copied ? "Copied brief" : "Copy brief"} placement="bottom">
              <button type="button" className="context-icon-button" aria-label={copied ? "Copied brief" : "Copy brief"} onClick={() => void copyBrief()}>{copied ? <Check size={14} /> : <Copy size={14} />}</button>
            </HoverTooltip>
          ) : null}
          {generated ? (
            <HoverTooltip title={`Refresh ${title.toLowerCase()}`} placement="bottom">
              <button type="button" className="context-icon-button" aria-label={`Refresh ${title.toLowerCase()}`} onClick={() => onRun(true)}><RefreshCw size={14} /></button>
            </HoverTooltip>
          ) : null}
        </div>
      )}
    </header>
    {copyFailed ? <p className="context-status" role="status">Could not copy brief</p> : null}
    {summaryText ? <ul className="thread-assist-summary">
      {summaryLines(summaryText).map((line, index) => <li key={index}>{line}</li>)}
    </ul> : null}
    {stale ? <p className="context-status">New messages since this brief.</p> : null}
    {!busy && !failure && (missingSummary || missingSuggestions) ? (
      <button type="button" className="thread-assist-run" onClick={() => onRun(false)}>
        <Sparkles size={14} />{missingSummary ? "Get Brief" : "Get Suggestions"}
      </button>
    ) : null}
    {failure ? <div className="action-analysis-error" role="alert">
      <p>{failure.summary}</p>
      <div className="action-analysis-error-actions">
        {failure.retryable ? <button type="button" onClick={() => onRun(false)}><RotateCcw size={13} /> Try Again</button> : null}
        {failure.retryable ? <details className="action-analysis-error-details"><summary>Technical details</summary><p>{error}</p></details> : null}
      </div>
    </div> : null}
    {showSuggestions ? <div className="thread-assist-suggestions">
      {summary.available ? <h4>Suggested</h4> : null}
      {!busy && suggestions.requested && suggestions.proposals.length === 0 && suggestions.hiddenCount === 0 ? <p className="context-status">Nothing to schedule or follow up on.</p> : null}
      {!busy && suggestions.hiddenCount > 0 ? <p className="context-status">{suggestions.hiddenCount === 1 ? "1 suggestion" : `${suggestions.hiddenCount} suggestions`} couldn&rsquo;t be matched to the email, so {suggestions.hiddenCount === 1 ? "it was" : "they were"} hidden.</p> : null}
      <div className="action-proposals">
        {suggestions.proposals.map((proposal, index) => {
          const source = messageSource(proposal.evidence.sourceMessageId);
          // Meetings default to the user's timezone and the next week, so only
          // a task with an unzoned time needs review before use.
          const needsReview = proposal.type === "task" && proposal.dueKind === "datetime" && !proposal.timeZone;
          const uncertain = proposal.confidence < 0.75;
          return <article className="action-proposal-card" key={`${proposal.type}-${index}`}>
            <div className="action-proposal-card-header"><span className="proposal-kind">{proposal.type === "meeting" ? "Meeting" : "Task"}</span>{uncertain || needsReview ? <span className="proposal-check">Check details</span> : null}</div>
            <strong>{proposal.title}</strong>
            {proposal.type === "meeting" ? <><p>{proposal.rawTimeLanguage || "Time not specified"}</p>{proposal.location ? <p>{proposal.location}</p> : null}{proposal.participants.length > 0 ? <p>{proposal.participants.join(", ")}</p> : null}</> : <p>{proposal.notes || proposal.kind.replace("_", " ")}{proposal.dueValue ? ` · Due ${proposal.dueValue}` : ""}</p>}
            {proposal.type === "meeting" ? <MeetingScheduler
              key={[proposal.normalizedStart, proposal.normalizedEnd, proposal.searchRangeStart, proposal.searchRangeEnd, proposal.durationMinutes, proposal.timeZone].join("|")}
              plan={planMeeting(proposal, new Date(), scheduling.preferences.defaultDurationMinutes)}
              preferences={scheduling.preferences}
              calendarConnected={scheduling.calendarConnected}
              onAddToCalendar={(slot) => scheduling.onAddToCalendar(index, proposal, slot)}
              onReplyWithTimes={scheduling.onReplyWithTimes}
              onConfirmTime={scheduling.onConfirmTime}
              onMoreTimes={scheduling.onMoreTimes}
              onOpenCalendarSettings={scheduling.onOpenCalendarSettings}
            /> : null}
            <details className="proposal-evidence"><summary>From the email</summary><blockquote>{proposal.evidence.excerpt}</blockquote>{source ? <small>{source}</small> : null}</details>
            <div className="proposal-actions">
              <button type="button" onClick={() => suggestions.onReview(index, proposal, "edit")}><Pencil size={13} /> Edit</button>
              {proposal.type === "task" ? <button type="button" onClick={() => suggestions.onReview(index, proposal, "accept")}>Review &amp; Add Task</button> : null}
              <button type="button" onClick={() => suggestions.onDiscard(index)}>Discard</button>
            </div>
          </article>;
        })}
      </div>
    </div> : null}
    {showSuggestions && preview ? <details className="action-analysis-preview"><summary>What was shared with AI</summary><pre>{preview}</pre></details> : null}
  </section>;
}
