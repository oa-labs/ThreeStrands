import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { Modal } from "./AppChrome";
import { mailClient } from "./data/client";
import { dailyNoteName } from "./noteDates";
import type { MeetingContext, MeetingRecording, ScheduleEvent } from "./domain";
import { errorMessage } from "./errors";
import { localDay } from "./meetingFollowThrough";
import { MeetingFollowThroughDialog } from "./MeetingFollowThroughDialog";
import { meetingEventReference } from "./meetingNoteEntry";
import { noteReferenceHref } from "./noteEntities";
import { useRecording } from "./recordingContext";
import type { Notice } from "./useNotice";

/**
 * The first start on this device. It explains the legal and practical side
 * before anything is captured; Start Recording both records that it was shown
 * and starts.
 */
export function RecordingConsentDialog({ onAccept, onCancel }: { onAccept(): Promise<void>; onCancel(): void }) {
  const [busy, setBusy] = useState(false);
  const start = useRef<HTMLButtonElement>(null);
  const form = useRef<HTMLFormElement>(null);
  const submit = async () => {
    if (busy) return;
    setBusy(true);
    try { await onAccept(); } finally { setBusy(false); }
  };
  const submitShortcut = (event: KeyboardEvent<HTMLFormElement>) => {
    if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
      event.preventDefault();
      form.current?.requestSubmit();
    }
  };
  return <Modal title="Recording Meetings" initialFocusRef={start} onClose={onCancel}>
    <form ref={form} className="modal-form" onKeyDown={submitShortcut} onSubmit={(event) => { event.preventDefault(); void submit(); }}>
      <p className="modal-form-hint">
        Recording laws differ by place, and some require everyone's consent. Tell the people in the meeting that you are recording it before you start.
      </p>
      <p className="modal-form-hint">
        ThreeStrands records your microphone and your Mac's audio. The audio stays on this Mac and is deleted after the time you choose in Settings → Meeting Recording.
        ThreeStrands never posts to a meeting's chat or invitation for you.
      </p>
      <div className="modal-form-actions">
        <button type="button" className="btn" onClick={onCancel}>Cancel</button>
        <button ref={start} type="submit" className="btn btn-primary" disabled={busy}>Start Recording</button>
      </div>
    </form>
  </Modal>;
}

/** Quitting while recording asks first; Cancel is where focus starts, so Enter never quits by accident. */
export function QuitRecordingDialog({ onConfirm, onCancel }: { onConfirm(): Promise<void>; onCancel(): void }) {
  const [busy, setBusy] = useState(false);
  const cancel = useRef<HTMLButtonElement>(null);
  return <Modal title="Stop Recording and Quit?" initialFocusRef={cancel} onClose={onCancel}>
    <div className="modal-form">
      <p className="modal-form-hint">A meeting is still being recorded. Quitting stops the recording and saves the audio recorded so far.</p>
      <div className="modal-form-actions">
        <button ref={cancel} type="button" className="btn" onClick={onCancel}>Cancel</button>
        <button type="button" className="btn btn-primary" disabled={busy} onClick={() => { setBusy(true); void onConfirm().finally(() => setBusy(false)); }}>Stop and Quit</button>
      </div>
    </div>
  </Modal>;
}

function syntheticEventForRecording(recording: MeetingRecording, context: MeetingContext | null): ScheduleEvent | null {
  const ref = recording.eventRef;
  if (!ref) return null;
  return {
    id: ref.id,
    accountId: ref.account ?? "",
    calendarId: ref.calendar ?? "",
    title: context?.event?.title ?? recording.title,
    start: context?.event?.start ?? recording.startedAt,
    end: context?.event?.end ?? recording.endedAt ?? recording.startedAt,
    allDay: context?.event?.allDay ?? false,
    attendees: context?.event?.attendees ?? [],
    seriesId: context?.event?.seriesId ?? ref.series ?? null,
  };
}

function RecordingReviewHost({
  target,
  onClose,
  onOpenNote,
  onAddMeetingNote,
  onNotice,
}: {
  target: MeetingRecording;
  onClose(): void;
  onOpenNote?(noteId: string, href?: string): void;
  onAddMeetingNote?(event: ScheduleEvent): void;
  onNotice?(notice: Notice): void;
}) {
  const [selectedEvent, setSelectedEvent] = useState<ScheduleEvent | null>(null);
  const [context, setContext] = useState<MeetingContext | null>(null);
  const [todaysEvents, setTodaysEvents] = useState<ScheduleEvent[]>([]);
  const [ready, setReady] = useState(!target.eventRef);
  const today = localDay(new Date(target.startedAt));
  const activeRef = useMemo(
    () => (selectedEvent ? meetingEventReference(selectedEvent) : target.eventRef),
    [selectedEvent, target.eventRef],
  );

  useEffect(() => {
    let live = true;
    if (!activeRef) {
      setContext(null);
      setReady(true);
      return;
    }
    setReady(false);
    void mailClient.meetingContext(activeRef).then(
      (found) => { if (live) { setContext(found); setReady(true); } },
      () => { if (live) { setContext(null); setReady(true); } },
    );
    return () => { live = false; };
  }, [activeRef]);

  useEffect(() => {
    if (target.eventRef) return;
    let live = true;
    const now = new Date();
    const start = new Date(now.getFullYear(), now.getMonth(), now.getDate(), 0, 0, 0).toISOString();
    const end = new Date(now.getFullYear(), now.getMonth(), now.getDate(), 23, 59, 59, 999).toISOString();
    const timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone;
    void mailClient.listScheduleEvents(start, end, timeZone).then(
      (res) => { if (live) setTodaysEvents(res.events.filter((ev) => Boolean(meetingEventReference(ev)))); },
      () => { if (live) setTodaysEvents([]); },
    );
    return () => { live = false; };
  }, [target.eventRef, target.id]);

  if (!ready && target.eventRef && !context) {
    return <Modal title="Review Follow-through" onClose={onClose}>
      <p className="modal-form-hint" role="status">Loading meeting review…</p>
    </Modal>;
  }

  const effectiveEvent = selectedEvent ?? syntheticEventForRecording(target, context);
  const effectiveRef = effectiveEvent ? meetingEventReference(effectiveEvent) : null;

  return <MeetingFollowThroughDialog
    recording={target}
    event={effectiveEvent}
    context={context}
    todaysEvents={todaysEvents}
    onSelectEvent={(next) => setSelectedEvent(next)}
    today={today}
    onClose={onClose}
    onAddNote={effectiveEvent && onAddMeetingNote ? () => onAddMeetingNote(effectiveEvent) : undefined}
    onSaved={(saved) => {
      onClose();
      const href = effectiveRef ? noteReferenceHref(effectiveRef) : undefined;
      onNotice?.({
        message: `Follow-through saved to ${saved.note.dailyDate ? dailyNoteName(saved.note.dailyDate) : saved.note.title}`,
        action: onOpenNote ? { label: "Open Note", run: () => onOpenNote(saved.note.id, href) } : undefined,
        undo: () => void saved.undo().catch((reason) => onNotice?.({ message: errorMessage(reason) })),
      });
    }}
  />;
}

/** The recording dialogs App shows: first-use consent, the quit confirmation, and recording review. */
export function RecordingDialogs({
  onOpenNote,
  onAddMeetingNote,
  onNotice,
}: {
  onOpenNote?(noteId: string, href?: string): void;
  onAddMeetingNote?(event: ScheduleEvent): void;
  onNotice?(notice: Notice): void;
} = {}) {
  const recording = useRecording();
  return <>
    {recording.consentPending ? <RecordingConsentDialog onAccept={recording.acceptConsent} onCancel={recording.cancelConsent} /> : null}
    {recording.quitRequested ? <QuitRecordingDialog onConfirm={recording.confirmQuit} onCancel={recording.cancelQuit} /> : null}
    {recording.reviewTarget ? (
      <RecordingReviewHost
        key={recording.reviewTarget.id}
        target={recording.reviewTarget}
        onClose={recording.closeReview}
        onOpenNote={onOpenNote}
        onAddMeetingNote={onAddMeetingNote}
        onNotice={onNotice}
      />
    ) : null}
  </>;
}
