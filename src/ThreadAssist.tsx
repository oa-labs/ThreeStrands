import { useId, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Check, ChevronRight, Copy, MapPin, RefreshCw, RotateCcw, Sparkles, Users, Video } from "lucide-react";
import type { ActionProposal, AvailabilityCandidate, AvailabilityPreferences, MeetingProposal, ScheduleEvent, ThreadDetail } from "./domain";
import type { CalendarAddition, ScheduleSlot } from "./MeetingScheduler";
import { FindTimesCard } from "./FindTimesCard";
import { MoreActionsMenu } from "./MoreActionsMenu";
import { planMeeting } from "./scheduling";
import { safeWebUrl } from "./calendarTime";
import { HoverTooltip } from "./AppChrome";
import { parseAddress } from "./emailAddress";
import { isSummaryStale } from "./threadPresentation";
import { ICON_SIZE } from "./iconSizes";
import { ContextSectionHeader, useRememberedCollapse } from "./ContextSections";
import { plural } from "./plural";
import { latestInvitationMessage, withoutInvitationMeetings } from "./calendarInvitations";

export const THREAD_ASSIST_ID = "thread-assist";

/** The most messages a brief sends; mirrors the native `MAX_MESSAGES`. */
export const BRIEF_MAX_MESSAGES = 15;

/** A summary line recapping the conversation before the new messages. */
const EARLIER_PREFIX = /^Earlier:\s*/i;

// Suggestions keep their object identity while listed, so a card keeps its
// key — and its calendar check — when an earlier card is discarded.
const proposalKeys = new WeakMap<ActionProposal, string>();
let nextProposalKey = 0;
function proposalKey(proposal: ActionProposal): string {
  let key = proposalKeys.get(proposal);
  if (!key) {
    key = `proposal-${nextProposalKey++}`;
    proposalKeys.set(proposal, key);
  }
  return key;
}

/**
 * AI brief and suggestions for the conversation. `enabled` means a provider
 * is chosen and the feature is switched on; `available` additionally
 * requires a configured provider key.
 */
export type ThreadAssistProps = {
  detail: ThreadDetail;
  /**
   * An AI provider is chosen. Without one, this section is the app's single
   * hint that AI features exist; with one, switched-off features are hidden.
   */
  providerChosen: boolean;
  summary: { enabled: boolean; available: boolean; pending: boolean };
  suggestions: {
    enabled: boolean;
    available: boolean;
    /** Whether suggestions were fetched for this revision; chat may add some without it. */
    requested: boolean;
    proposals: ActionProposal[];
    hiddenCount: number;
    /** Valid suggestions past the per-analysis limit, which weren't shown. */
    extraCount?: number;
    onDiscard(index: number): void;
    onReview(index: number, proposal: ActionProposal, intent: "edit" | "accept"): void;
  };
  /** Calendar checks for meeting suggestions; every outcome is reviewed elsewhere. */
  scheduling: {
    calendarConnected: boolean;
    preferences: AvailabilityPreferences;
    onAddToCalendar(index: number, proposal: MeetingProposal, slot: CalendarAddition): void;
    onReplyWithTimes(slots: AvailabilityCandidate[]): void;
    onConfirmTime(slot: ScheduleSlot): void;
    onOpenCalendarSettings(): void;
    /** Opens the calendar event a suggestion is already on the calendar as. */
    onOpenCalendarEvent?(event: ScheduleEvent): void;
  };
  loading: boolean;
  error: string | null;
  /** Who the conversation was shared with, such as "Anthropic"; null when no provider is chosen. */
  providerName: string | null;
  /** Runs whatever is missing, or everything when `force` is set. */
  onRun(force: boolean): void;
  onOpenSettings(): void;
};

/** Explains suggestions left out because one email produced more than are shown at once. */
export function extraSuggestionsText(count: number): string {
  return count === 1
    ? "1 more suggestion wasn\u2019t shown. Ask about it by name to add it."
    : `${count} more suggestions weren\u2019t shown. Ask about one by name to add it.`;
}

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

/**
 * What the brief covered: how many of the conversation's messages were new
 * to it, and how many were sent. `newCount` equals `coveredCount` when the
 * brief read the whole conversation.
 */
export function briefScope(detail: ThreadDetail): { newCount: number; coveredCount: number; sentCount: number } {
  const { thread } = detail;
  const revision = thread.summaryRevision ?? thread.lastMessageAt;
  const covered = detail.messages.filter((message) => message.sentAt <= revision);
  const focusFrom = thread.summaryFocusFrom;
  const newCount = focusFrom ? covered.filter((message) => message.sentAt >= focusFrom).length : covered.length;
  return { newCount, coveredCount: covered.length, sentCount: Math.min(covered.length, BRIEF_MAX_MESSAGES) };
}

export function summaryLines(summary: string): string[] {
  return summary
    .split("\n")
    .map((line) => line.replace(/^[-•]\s*/, "").trim())
    .filter(Boolean);
}

export function summaryClipboardText(summary: string): string {
  const lines = summaryLines(summary);
  return lines.length === 1 ? lines[0] : lines.map((line) => `- ${line}`).join("\n");
}

// Suffixes and company forms that follow a comma without reversing a name.
const NAME_SUFFIX = /^(jr|sr|ii|iii|iv|phd|md|esq|inc|llc|ltd|co|corp)\.?$/i;

/** Turns a directory-style "Last, First" into "First Last"; anything else stays as written. */
export function participantName(name: string): string {
  const parts = name.split(",").map((part) => part.trim());
  if (parts.length !== 2) return name;
  const [last, first] = parts;
  if (!last || !first || /\s/.test(last) || NAME_SUFFIX.test(first)) return name;
  return `${first} ${last}`;
}

const LOCATION_URL = /https?:\/\/[^\s<>"]+/;

/**
 * Where a meeting happens, split into its place and a link to join it. The
 * link comes from the calendar copy when there is one, otherwise from a web
 * address written in the location; the place keeps any other words.
 */
export function meetingPlace(location: string | null, calendarConferenceUrl: string | null | undefined): { place: string; joinUrl: string | null } {
  const text = location ?? "";
  // Trailing punctuation belongs to the sentence, not the address.
  const written = text.match(LOCATION_URL)?.[0].replace(/[.,;:!?)\]]+$/, "") ?? null;
  const joinUrl = safeWebUrl(calendarConferenceUrl) ?? safeWebUrl(written);
  // Only words left around a removed address lose their joining punctuation; "Main St." keeps its period.
  const place = written && joinUrl === safeWebUrl(written)
    ? text.replace(LOCATION_URL, "").replace(/\s+/g, " ").replace(/^[\s,;:·.-]+|[\s,;:·.-]+$/g, "")
    : text.trim();
  return { place, joinUrl };
}

/**
 * One suggestion. A meeting keeps Edit and Dismiss in the header's ⋯ so its
 * only row of buttons is the scheduler's. Once it is on the calendar the
 * scheduler shows when it is, so the card drops the email's wording of the
 * time and moves its own actions into the scheduler's row: Dismiss beside
 * Open in Calendar, Edit in ⋯.
 */
function ProposalCard({ proposal, index, source, suggestions, scheduling }: {
  proposal: ActionProposal;
  index: number;
  source: string | null;
  suggestions: Pick<ThreadAssistProps["suggestions"], "onDiscard" | "onReview">;
  scheduling: ThreadAssistProps["scheduling"];
}) {
  const [matchedEvent, setMatchedEvent] = useState<ScheduleEvent | null>(null);
  const matched = matchedEvent !== null;
  // Meetings default to the user's timezone and the next week, so only
  // a task with an unzoned time needs review before use.
  const needsReview = proposal.type === "task" && proposal.dueKind === "datetime" && !proposal.timeZone;
  const uncertain = proposal.confidence < 0.75;
  const kind = proposal.type === "task" ? "Task" : proposal.intent === "attend" ? "Event" : "Meeting";
  const edit = () => suggestions.onReview(index, proposal, "edit");
  const dismiss = <button type="button" className="btn btn-sm" onClick={() => suggestions.onDiscard(index)}>Dismiss</button>;
  const where = proposal.type === "meeting" ? meetingPlace(proposal.location, matchedEvent?.conferenceUrl) : null;
  const people = proposal.type === "meeting" && proposal.participants.length > 0
    ? new Intl.ListFormat(undefined, { type: "conjunction" }).format(proposal.participants.map(participantName))
    : "";
  return <article className="action-proposal-card">
    <div className="action-proposal-card-top">
      <div className="action-proposal-card-header">
        <span className="proposal-kind">{kind}</span>
        {uncertain || needsReview ? <button type="button" className="btn-link proposal-check" title="Some details may be wrong. Edit to check them." onClick={edit}>Check details</button> : null}
      </div>
      {proposal.type === "meeting" && !matched ? <MoreActionsMenu label="More actions" actions={[{ label: "Edit", onSelect: edit }, { label: "Dismiss", onSelect: () => suggestions.onDiscard(index) }]} /> : null}
    </div>
    <strong>{proposal.title}</strong>
    {proposal.type === "meeting" ? <>
      {/* The email's own words, quoted so they read as what was asked rather than as a description. */}
      {matched ? null : proposal.rawTimeLanguage ? <p className="proposal-time-language">{proposal.rawTimeLanguage}</p> : <p>Time not specified</p>}
      {where?.joinUrl ? <p className="proposal-detail"><Video size={ICON_SIZE.sm} aria-hidden="true" /><span>
        {where.place ? <>{where.place} · </> : null}
        {/* The address shows on hover, since it came from the email or the calendar rather than from ThreeStrands. */}
        <a href={where.joinUrl} title={where.joinUrl} onClick={(event) => { event.preventDefault(); void openUrl(where.joinUrl!); }}>{where.place ? "Join" : "Join video meeting"}</a>
      </span></p> : where?.place ? <p className="proposal-detail"><MapPin size={ICON_SIZE.sm} aria-hidden="true" /><span>{where.place}</span></p> : null}
      {people ? <p className="proposal-detail"><Users size={ICON_SIZE.sm} aria-hidden="true" /><span>{people}</span></p> : null}
    </> : <p>{proposal.notes || proposal.kind.replace("_", " ")}{proposal.dueValue ? ` · Due ${proposal.dueValue}` : ""}</p>}
    {proposal.type === "meeting" ? <FindTimesCard
      key={[proposal.intent, proposal.normalizedStart, proposal.normalizedEnd, proposal.eventDate, proposal.searchRangeStart, proposal.searchRangeEnd, proposal.durationMinutes, proposal.timeZone].join("|")}
      plan={planMeeting(proposal, new Date(), scheduling.preferences.defaultDurationMinutes)}
      preferences={scheduling.preferences}
      calendarConnected={scheduling.calendarConnected}
      // The AI section stays read-only; Thread Chat is where to ask for other days in words.
      ai={null}
      onAddToCalendar={(slot) => scheduling.onAddToCalendar(index, proposal, slot)}
      onReplyWithTimes={scheduling.onReplyWithTimes}
      onConfirmTime={scheduling.onConfirmTime}
      onOpenCalendarSettings={scheduling.onOpenCalendarSettings}
      identity={{ title: proposal.title, location: proposal.location, sourceMessageId: proposal.evidence.sourceMessageId }}
      onOpenCalendarEvent={scheduling.onOpenCalendarEvent}
      onMatchChange={setMatchedEvent}
      matchedActions={dismiss}
      moreActions={[{ label: "Edit", onSelect: edit }]}
    /> : null}
    {proposal.type === "task" ? <div className="proposal-actions">
      {/* Icons mark the calendar actions only, so Edit and Dismiss stay plain. */}
      <button type="button" className="btn btn-sm" onClick={edit}>Edit</button>
      <button type="button" className="btn btn-sm" onClick={() => suggestions.onReview(index, proposal, "accept")}>Review &amp; Add Task</button>
      {dismiss}
    </div> : null}
    <details className="proposal-evidence"><summary><ChevronRight className="proposal-evidence-chevron" size={ICON_SIZE.xs} aria-hidden="true" />From the email</summary><blockquote>{proposal.evidence.excerpt}</blockquote>{source ? <small>{source}</small> : null}</details>
  </article>;
}

export function ThreadAssist({ detail, providerChosen, summary, suggestions, scheduling, loading, error, providerName, onRun, onOpenSettings }: ThreadAssistProps) {
  const { thread } = detail;
  const [copiedSummary, setCopiedSummary] = useState<string | null>(null);
  const [copyFailed, setCopyFailed] = useState(false);
  const [collapsed, toggleCollapsed] = useRememberedCollapse(THREAD_ASSIST_ID);
  const bodyId = useId();
  if (!providerChosen) {
    return <section id={THREAD_ASSIST_ID} className="context-section thread-assist" aria-label="Brief">
      <p className="context-status">Set up AI to get a brief of each conversation, suggested next steps, and answers to your questions about it.</p>
      <button type="button" className="btn-link context-link-button" onClick={onOpenSettings}>Set Up AI</button>
    </section>;
  }
  // Switched off on purpose: nothing to show, and no reminder.
  if (!summary.enabled && !suggestions.enabled) return null;
  if (!summary.available && !suggestions.available) {
    return <section id={THREAD_ASSIST_ID} className="context-section thread-assist" aria-label="Brief">
      <p className="context-status">Set up an AI provider and API key in AI settings to get a brief of this conversation.</p>
      <button type="button" className="btn-link context-link-button" onClick={onOpenSettings}>AI Settings</button>
    </section>;
  }

  const title = summary.available ? "Brief" : "Suggestions";
  const summaryText = summary.available ? thread.summary : null;
  const copied = Boolean(summaryText) && copiedSummary === summaryText;
  const stale = Boolean(summaryText) && isSummaryStale(thread);
  const busy = loading || summary.pending;
  const generated = Boolean(summaryText) || suggestions.requested;
  const visibleProposals = withoutInvitationMeetings(suggestions.proposals, detail);
  const noSuggestions = suggestions.requested && suggestions.proposals.length === 0 && suggestions.hiddenCount === 0 && !suggestions.extraCount;
  // With nothing suggested, a brief already says whether anything is needed,
  // so an empty Suggested section would only repeat it.
  const showSuggestions = suggestions.available && (suggestions.requested || visibleProposals.length > 0)
    && (visibleProposals.length > 0 || !latestInvitationMessage(detail) || suggestions.hiddenCount > 0 || Boolean(suggestions.extraCount))
    && !(noSuggestions && !busy && summaryText);
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

  const runButton = !failure && (missingSummary || missingSuggestions) ? (
    <HoverTooltip title={missingSummary ? "Get Brief" : "Get Suggestions"} shortcut={missingSummary ? "i" : undefined} placement="bottom">
      <button type="button" className="btn btn-sm thread-assist-run" aria-keyshortcuts={missingSummary ? "i" : undefined} onClick={() => onRun(false)}>
        <Sparkles size={ICON_SIZE.sm} />{missingSummary ? "Get Brief" : "Get Suggestions"}
      </button>
    </HoverTooltip>
  ) : null;
  // Before anything has run, only the button shows, beside the panel's other
  // AI actions; the section opens to full width once there is something to read.
  if (runButton && !busy && !generated && !showSuggestions) {
    return <section id={THREAD_ASSIST_ID} className="context-ai-launcher thread-assist" aria-label={title}>{runButton}</section>;
  }

  const scope = briefScope(detail);
  const lines = summaryText ? summaryLines(summaryText) : [];
  const scopeNote = summaryText && scope.newCount < scope.coveredCount
    ? (scope.newCount === 1 ? "From the latest message" : `From the ${scope.newCount} latest messages`)
    : null;
  const shared = generated && !busy && Boolean(providerName);
  // The recap of earlier messages, if the brief gave one, always reads last.
  const orderedLines = [...lines.filter((line) => !EARLIER_PREFIX.test(line)), ...lines.filter((line) => EARLIER_PREFIX.test(line))];
  const headerActions = busy ? <span className="context-status" role="status">Reading the conversation…</span> : (
    <>
      {runButton}
      {/* Copy is revealed on hover or focus, so Refresh is the action that reads first. */}
      {summaryText ? (
        <HoverTooltip title={copied ? "Copied brief" : "Copy brief"} placement="bottom">
          <button type="button" className="btn-icon btn-icon-sm thread-assist-copy" aria-label={copied ? "Copied brief" : "Copy brief"} onClick={() => void copyBrief()}>{copied ? <Check size={ICON_SIZE.sm} /> : <Copy size={ICON_SIZE.sm} />}</button>
        </HoverTooltip>
      ) : null}
      {generated ? (
        <HoverTooltip title={`Refresh ${title.toLowerCase()}`} placement="bottom">
          <button type="button" className="btn-icon btn-icon-sm" aria-label={`Refresh ${title.toLowerCase()}`} onClick={() => onRun(true)}><RefreshCw size={ICON_SIZE.sm} /></button>
        </HoverTooltip>
      ) : null}
    </>
  );

  return <section id={THREAD_ASSIST_ID} className="context-section context-collapsible thread-assist" aria-labelledby="thread-assist-heading">
    <ContextSectionHeader title={title} titleId="thread-assist-heading" actions={headerActions} toggle={{ collapsed, controls: bodyId, onToggle: toggleCollapsed }} />
    <div id={bodyId} hidden={collapsed}>
      {copyFailed ? <p className="context-status" role="status">Could not copy brief</p> : null}
      {orderedLines.length === 1 ? <p className="thread-assist-summary thread-assist-summary-paragraph">{orderedLines[0]}</p> : summaryText ? <ul className="thread-assist-summary" role="list">
        {orderedLines.map((line, index) => <li key={index} className={EARLIER_PREFIX.test(line) ? "thread-assist-summary-earlier" : undefined}>
          <span className="context-row-glyph thread-assist-summary-marker" aria-hidden="true"><svg width={ICON_SIZE.xs} height={ICON_SIZE.xs} viewBox="0 0 12 12"><circle cx="6" cy="6" r="2" fill="currentColor" /></svg></span><span>{line}</span>
        </li>)}
      </ul> : null}
      {stale ? <p className="context-status">New messages since this brief.</p> : null}
      {/* Where the brief came from and what was sent for it, as one line under it. */}
      {scopeNote || shared ? <p className="thread-assist-shared">
        {scopeNote ? <span>{scopeNote}</span> : null}
        {scopeNote && shared ? <span aria-hidden="true"> · </span> : null}
        {shared ? <>
          <span>Shared {plural(scope.sentCount, "message")} with {providerName}.</span>{" "}
          <button type="button" className="btn-link context-link-button" onClick={onOpenSettings}>AI Activity</button>
        </> : null}
      </p> : null}
      {failure ? <div className="action-analysis-error" role="alert">
        <p>{failure.summary}</p>
        <div className="action-analysis-error-actions">
          {failure.retryable ? <button type="button" className="btn btn-sm" onClick={() => onRun(false)}><RotateCcw size={ICON_SIZE.sm} /> Try Again</button> : null}
          {failure.retryable ? <details className="action-analysis-error-details"><summary>Technical details</summary><p>{error}</p></details> : null}
        </div>
      </div> : null}
      {showSuggestions ? <div className="thread-assist-suggestions">
        {summary.available ? <h4>Suggested</h4> : null}
        {!busy && suggestions.requested && suggestions.proposals.length === 0 && suggestions.hiddenCount === 0 ? <p className="context-status">Nothing to schedule or follow up on.</p> : null}
        {!busy && suggestions.hiddenCount > 0 ? <p className="context-status">{suggestions.hiddenCount === 1 ? "1 suggestion" : `${suggestions.hiddenCount} suggestions`} couldn&rsquo;t be matched to the email, so {suggestions.hiddenCount === 1 ? "it was" : "they were"} hidden.</p> : null}
        {!busy && suggestions.extraCount ? <p className="context-status">{extraSuggestionsText(suggestions.extraCount)}</p> : null}
        <div className="action-proposals">
          {visibleProposals.map((proposal) => <ProposalCard
            key={proposalKey(proposal)}
            proposal={proposal}
            index={suggestions.proposals.indexOf(proposal)}
            source={messageSource(proposal.evidence.sourceMessageId)}
            suggestions={suggestions}
            scheduling={scheduling}
          />)}
        </div>
      </div> : null}
    </div>
  </section>;
}
