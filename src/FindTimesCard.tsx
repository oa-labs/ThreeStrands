// The one card for offering open times: compose's Find Times, meeting
// suggestions, and Thread Chat's availability answers all show it.

import { ProposedTimesCard } from "./ProposedTimesCard";
import { type ReactNode, useRef, useState } from "react";
import { Sparkles } from "lucide-react";
import type { AvailabilityCandidate, AvailabilityPreferences, CoworkerCalendars, ScheduleEvent, TimeSearch } from "./domain";
import type { EventIdentity } from "./eventMatching";
import { readAiRequestConfig } from "./aiSettings";
import { mailClient } from "./data/client";
import { errorMessage } from "./errors";
import { ICON_SIZE } from "./iconSizes";
import type { MoreAction } from "./MoreActionsMenu";
import { MeetingScheduler, type CalendarAddition, type EffectiveSearch, type ScheduleSlot } from "./MeetingScheduler";
import { FIND_TIMES_INSTRUCTION_MAX_CHARS, lookaheadRange, planTimeSearch, type SchedulePlan } from "./scheduling";

/** What Find Times shares with the AI provider: a conversation, a draft, or both. Never the calendar. */
export type FindTimesContext = {
  /** A message in the conversation to read, or null for a new message. */
  sourceMessageId: string | null;
  draftSubject: string;
  draftText: string;
};

/** Asks the provider which days and hours to search, optionally adjusting the search on screen. */
export function askFindTimes(
  context: FindTimesContext,
  userTimeZone: string,
  adjustment: { instruction: string; current: TimeSearch } | null,
): Promise<TimeSearch> {
  const { provider, model, endpoint } = readAiRequestConfig("finding times", "findTimes");
  return mailClient.findTimes({
    ...context,
    instruction: adjustment?.instruction ?? null,
    current: adjustment?.current ?? null,
    userTimeZone,
  }, provider, model, endpoint);
}

/**
 * Open times for a plan, with the chips to adjust them and, when Find Times
 * AI is on, a field to adjust them in words. An adjustment replaces the plan
 * and its reason; the card's caller keeps its own plan unchanged.
 */
export function FindTimesCard({
  plan, reason = null, preferences, calendarConnected, ai, intoDraft,
  onAddToCalendar, onReplyWithTimes, onConfirmTime, onOpenCalendarSettings, identity, onOpenCalendarEvent,
  onMatchChange, matchedActions, moreActions, coworkers,
}: {
  plan: SchedulePlan;
  /** Why these days were chosen, shown above the times. */
  reason?: string | null;
  preferences: AvailabilityPreferences;
  calendarConnected: boolean;
  coworkers?: CoworkerCalendars;
  /** Reads what to share when the user adjusts in words; null when Find Times AI is off. */
  ai: (() => FindTimesContext) | null;
  intoDraft?: boolean;
  onAddToCalendar(slot: CalendarAddition): void;
  onReplyWithTimes(slots: AvailabilityCandidate[]): void;
  onConfirmTime(slot: ScheduleSlot): void;
  onOpenCalendarSettings(): void;
  /** What the event is called and where, to recognize it already on the calendar. */
  identity?: EventIdentity;
  onOpenCalendarEvent?(event: ScheduleEvent): void;
  onMatchChange?(event: ScheduleEvent | null): void;
  /** Shown with Open in Calendar once the event is on the calendar. */
  matchedActions?: ReactNode;
  moreActions?: MoreAction[];
}) {
  const [adjusted, setAdjusted] = useState<{ key: number; plan: SchedulePlan; reason: string } | null>(null);
  const [instruction, setInstruction] = useState("");
  const [reading, setReading] = useState(false);
  const [error, setError] = useState("");
  // The search on screen, including chip adjustments, sent back with the user's words.
  const shown = useRef<EffectiveSearch | null>(null);
  const shownReason = adjusted?.reason ?? reason;

  const adjust = async () => {
    const text = instruction.trim();
    const current = shown.current;
    if (!ai || !text || !current || reading) return;
    setError("");
    setReading(true);
    try {
      const result = await askFindTimes(ai(), preferences.timeZone, {
        instruction: text,
        current: {
          rangeStart: current.query.start,
          rangeEnd: current.query.end,
          durationMinutes: current.durationMinutes,
          ...current.bounds,
          reason: shownReason ?? "The days shown.",
        },
      });
      setAdjusted((previous) => ({ key: (previous?.key ?? 0) + 1, plan: planTimeSearch(result, new Date(), preferences.defaultDurationMinutes), reason: result.reason }));
      setInstruction("");
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setReading(false);
    }
  };

  const adjustInput = ai ? <>
    <form className="find-times-adjust" onSubmit={(event) => { event.preventDefault(); void adjust(); }}>
      <input
        aria-label="Adjust times"
        value={instruction}
        maxLength={FIND_TIMES_INSTRUCTION_MAX_CHARS}
        placeholder="e.g. Thursday or Friday afternoon"
        disabled={reading}
        onChange={(event) => setInstruction(event.target.value)}
      />
      <button type="submit" className="btn btn-sm" disabled={reading || !instruction.trim()}><Sparkles size={ICON_SIZE.sm} />Adjust</button>
    </form>
    {reading ? <p className="context-status" role="status">Reading the conversation…</p> : null}
    {error ? <p className="context-status" role="alert">{error}</p> : null}
  </> : null;

  const effectivePlan = adjusted?.plan ?? plan;
  if (effectivePlan.proposedTimes?.length) return <>
    {shownReason ? <p className="find-times-reason">{shownReason}</p> : null}
    <ProposedTimesCard times={effectivePlan.proposedTimes} preferences={preferences} calendarConnected={calendarConnected}
      intoDraft={intoDraft} onReplyWithTimes={onReplyWithTimes} onOpenCalendarSettings={onOpenCalendarSettings}
      onFindOtherTimes={() => setAdjusted((previous) => ({ key: (previous?.key ?? 0) + 1,
        plan: { query: lookaheadRange(new Date()), durationMinutes: effectivePlan.durationMinutes, durationIsDefault: effectivePlan.durationIsDefault, timeZoneAssumed: false },
        reason: "Finding other times in your usual hours." }))} />
  </>;

  return <>
    {shownReason ? <p className="find-times-reason">{shownReason}</p> : null}
    <MeetingScheduler
      key={adjusted?.key ?? 0}
      plan={adjusted?.plan ?? plan}
      preferences={preferences}
      calendarConnected={calendarConnected}
      intoDraft={intoDraft}
      coworkers={coworkers}
      onAddToCalendar={onAddToCalendar}
      onReplyWithTimes={onReplyWithTimes}
      onConfirmTime={onConfirmTime}
      onOpenCalendarSettings={onOpenCalendarSettings}
      identity={identity}
      onOpenCalendarEvent={onOpenCalendarEvent}
      onMatchChange={onMatchChange}
      matchedActions={matchedActions}
      moreActions={moreActions}
      onSearchChange={(effective) => { shown.current = effective; }}
      adjustInput={adjustInput}
    />
  </>;
}
