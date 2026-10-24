import { useEffect, useState, useSyncExternalStore } from "react";
import { CalendarDays } from "lucide-react";
import { CalendarResponseControls } from "./CalendarResponseControls";
import { calendarEventRange } from "./CalendarAttachment";
import { isCalendarAttachment } from "./calendarInvitations";
import { ContextSectionHeader } from "./ContextSections";
import { mailClient } from "./data/client";
import type { AvailabilityCandidate, AvailabilityPreferences, CalendarEventPreview, Message, ScheduleEvent } from "./domain";
import { errorMessage } from "./errors";
import { FindTimesCard } from "./FindTimesCard";
import { ICON_SIZE } from "./iconSizes";
import { clampMeetingMinutes, lookaheadRange, type SchedulePlan } from "./scheduling";
import { refreshScheduleCache, scheduleCacheGeneration, subscribeScheduleCache } from "./calendarScheduleCache";

type Props = {
  message: Message;
  calendarConnected: boolean;
  preferences: AvailabilityPreferences;
  ai: boolean;
  intoDraft: boolean;
  onReplyWithTimes(candidates: AvailabilityCandidate[], sourceMessageId: string): void;
  onOpenCalendarSettings(): void;
};

/** Native invitation actions, independent of the AI brief and its settings. */
export function InvitationSection(props: Props) {
  const key = JSON.stringify([props.message.id, props.message.attachments.filter(isCalendarAttachment)]);
  return <InvitationSectionBody key={key} {...props} />;
}

function InvitationSectionBody(props: Props) {
  const [events, setEvents] = useState<CalendarEventPreview[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const { message } = props;
  const attachmentsKey = JSON.stringify(message.attachments.filter(isCalendarAttachment).map((attachment) => attachment.id));
  useEffect(() => {
    let active = true;
    setError(null);
    setEvents(null);
    const ids = JSON.parse(attachmentsKey) as string[];
    void Promise.all(ids.map((id) => mailClient.previewCalendarAttachment(message.id, id)))
      .then((previews) => {
        if (!active) return;
        const seen = new Set<string>();
        setEvents(previews.flatMap((preview) => preview.events).filter((event) => {
          const key = event.uid?.trim() || JSON.stringify([event.title, event.start, event.end]);
          if (seen.has(key)) return false;
          seen.add(key);
          return true;
        }));
      })
      .catch((reason: unknown) => { if (active) setError(`Could not read invitation: ${errorMessage(reason)}`); });
    return () => { active = false; };
  }, [message.id, attachmentsKey, attempt]);
  return <section className="context-section context-invitation" aria-label="Invitation">
    <ContextSectionHeader title="Invitation" icon={<CalendarDays size={ICON_SIZE.xs} />} />
    <div className="context-invitation-body">
      {error ? <><p className="context-status" role="alert">{error}</p><button type="button" className="btn btn-sm" onClick={() => setAttempt((value) => value + 1)}>Retry</button></>
        : events === null ? <p className="context-status" role="status">Reading invitation…</p>
          : events.length === 0 ? <p className="context-status">No events could be read from this invitation.</p>
            : events.map((event, index) => <InvitationResponse key={`${event.uid ?? index}`} {...props} invitation={event} showTitle={events.length > 1} />)}
    </div>
  </section>;
}

type Lookup = { kind: "checking" } | { kind: "found"; event: ScheduleEvent } | { kind: "missing" } | { kind: "error"; message: string };

function InvitationResponse({ invitation, showTitle, ...props }: Props & { invitation: CalendarEventPreview; showTitle: boolean }) {
  const { calendarConnected, message, preferences, ai, intoDraft, onReplyWithTimes, onOpenCalendarSettings } = props;
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
    setPlan({ query: lookaheadRange(new Date()), durationMinutes: clampMeetingMinutes(duration, preferences.defaultDurationMinutes), timeZoneAssumed: false });
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
          ai={ai ? () => ({ sourceMessageId: message.id, draftSubject: invitation.title, draftText: "" }) : null}
          onReplyWithTimes={(slots) => onReplyWithTimes(slots, message.id)}
          onOpenCalendarSettings={onOpenCalendarSettings}
          onAddToCalendar={() => undefined} onConfirmTime={() => undefined} /> : null}
      </>}
    </>;
  };
  return <div className="context-invitation-response">
    {showTitle ? <strong>{invitation.title}</strong> : null}
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
