import { useEffect, useState, useSyncExternalStore } from "react";
import { CalendarResponseControls } from "./CalendarResponseControls";
import { calendarEventRange } from "./calendarInvitations";
import { mailClient } from "./data/client";
import type { AvailabilityCandidate, AvailabilityPreferences, CalendarEventPreview, ScheduleEvent } from "./domain";
import { errorMessage } from "./errors";
import { FindTimesCard } from "./FindTimesCard";
import { clampMeetingMinutes, defaultLength, lookaheadRange, type SchedulePlan } from "./scheduling";
import { refreshScheduleCache, scheduleCacheGeneration, subscribeScheduleCache } from "./calendarScheduleCache";

/** Actions offered beside a parsed invitation, independent of AI brief settings. */
export type InvitationActions = {
  calendarConnected: boolean;
  preferences: AvailabilityPreferences;
  ai: boolean;
  intoDraft: boolean;
  onReplyWithTimes(candidates: AvailabilityCandidate[], sourceMessageId: string): void;
  onOpenCalendarSettings(): void;
};

type Lookup = { kind: "checking" } | { kind: "found"; event: ScheduleEvent } | { kind: "missing" } | { kind: "error"; message: string };

export function InvitationResponse({ invitation, sourceMessageId, ...props }: InvitationActions & { invitation: CalendarEventPreview; sourceMessageId: string }) {
  const { calendarConnected, preferences, ai, intoDraft, onReplyWithTimes, onOpenCalendarSettings } = props;
  const [lookup, setLookup] = useState<Lookup>({ kind: "checking" });
  const [attempt, setAttempt] = useState(0);
  const [plan, setPlan] = useState<SchedulePlan | null>(null);
  const uid = invitation.uid?.trim();
  const cancelled = invitation.status?.toLocaleLowerCase() === "cancelled";
  useEffect(() => {
    if (!calendarConnected || !uid || cancelled) return;
    let active = true;
    setLookup({ kind: "checking" });
    void mailClient.findCalendarInvitation(uid)
      .then((event) => { if (active) setLookup(event ? { kind: "found", event } : { kind: "missing" }); })
      .catch((reason: unknown) => { if (active) setLookup({ kind: "error", message: `Could not check invitation: ${errorMessage(reason)}` }); });
    return () => { active = false; };
  }, [calendarConnected, uid, cancelled, attempt]);

  const suggestTimes = () => {
    const range = calendarEventRange(invitation);
    const event = lookup.kind === "found" ? lookup.event : null;
    const duration = event && !event.allDay ? (Date.parse(event.end) - Date.parse(event.start)) / 60_000
      : range && !invitation.allDay ? (range.end.getTime() - range.start.getTime()) / 60_000 : null;
    setPlan({ query: lookaheadRange(new Date()), durationMinutes: clampMeetingMinutes(duration, preferences.defaultDurationMinutes), ...defaultLength(duration), timeZoneAssumed: false });
  };
  const response = () => {
    if (cancelled) return <p className="context-status">This event was cancelled.</p>;
    if (!calendarConnected) return <><p className="context-status">Connect a calendar to respond to this invitation.</p><button type="button" className="btn-link" onClick={onOpenCalendarSettings}>Calendar Settings</button></>;
    if (!uid) return <p className="context-status">This invitation has no event identifier, so it can’t be matched to your calendar.</p>;
    if (lookup.kind === "checking") return <p className="context-status" role="status">Checking your calendar…</p>;
    if (lookup.kind === "error") return <><p className="context-status" role="alert">{lookup.message}</p><button type="button" className="btn btn-sm" onClick={() => setAttempt((value) => value + 1)}>Retry</button></>;
    if (lookup.kind === "missing") return <p className="context-status">This invitation isn’t on a connected calendar yet.</p>;
    return <>
      <CalendarResponseControls invitation event={lookup.event} onUpdated={(event) => setLookup({ kind: "found", event })} />
      <InvitationConflicts event={lookup.event} timeZone={preferences.timeZone} />
      {!lookup.event.canRespond ? <p className="context-status">Already on your calendar. This calendar doesn’t allow you to respond.</p> : <>
        <button type="button" className="btn-link" onClick={suggestTimes}>Suggest a different time</button>
        {plan ? <FindTimesCard plan={plan} preferences={preferences} calendarConnected intoDraft={intoDraft}
          ai={ai ? () => ({ sourceMessageId, draftSubject: invitation.title, draftText: "" }) : null}
          onReplyWithTimes={(slots) => onReplyWithTimes(slots, sourceMessageId)}
          onOpenCalendarSettings={onOpenCalendarSettings}
          onAddToCalendar={() => undefined} onConfirmTime={() => undefined} /> : null}
      </>}
    </>;
  };
  return <div className="context-invitation-response">
    {response()}
  </div>;
}

/** Only other overlapping events are conflicts; the invitation itself never is. */
function InvitationConflicts({ event, timeZone }: { event: ScheduleEvent; timeZone: string }) {
  const [conflicts, setConflicts] = useState<string[]>([]);
  const [error, setError] = useState(false);
  const generation = useSyncExternalStore(subscribeScheduleCache, scheduleCacheGeneration);
  const { start, end, id, accountId, allDay } = event;
  useEffect(() => {
    if (allDay) return;
    let active = true;
    const request = { timeMin: start, timeMax: end, timeZone };
    setConflicts([]);
    setError(false);
    void refreshScheduleCache(request, () => mailClient.listScheduleEvents(start, end, timeZone))
      .then((result) => {
        if (!active) return;
        const other = result.events.filter((candidate) => !(candidate.id === id && candidate.accountId === accountId)
          && !candidate.allDay && Date.parse(candidate.start) < Date.parse(end) && Date.parse(candidate.end) > Date.parse(start));
        setConflicts([...new Set(other.map((candidate) => candidate.title))]);
        setError(result.errors.length > 0);
      })
      .catch(() => { if (active) setError(true); });
    return () => { active = false; };
  }, [start, end, id, accountId, allDay, timeZone, generation]);
  return <>
    {conflicts.length > 0 ? <p className="context-status">Conflicts with {new Intl.ListFormat(undefined, { type: "conjunction" }).format(conflicts.map((title) => `“${title}”`))}</p> : null}
    {error ? <p className="context-status">Some calendars couldn’t be checked for conflicts.</p> : null}
  </>;
}
