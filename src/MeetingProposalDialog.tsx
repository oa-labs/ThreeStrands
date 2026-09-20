import { useRef, useState } from "react";
import { Modal } from "./AppChrome";
import type { MeetingProposal } from "./domain";

function dateTimeInputValue(value: string | null): string {
  if (!value) return "";
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) return value.slice(0, 16);
  const local = new Date(parsed.getTime() - parsed.getTimezoneOffset() * 60_000);
  return local.toISOString().slice(0, 16);
}

function isoDateTime(value: string): string | null {
  if (!value) return null;
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime()) ? value : parsed.toISOString();
}

export function MeetingProposalDialog({
  proposal,
  onClose,
  onSave,
}: {
  proposal: MeetingProposal;
  onClose(): void;
  onSave(proposal: MeetingProposal): void;
}) {
  const [title, setTitle] = useState(proposal.title);
  const [participants, setParticipants] = useState(proposal.participants.join(", "));
  const [location, setLocation] = useState(proposal.location ?? "");
  const [start, setStart] = useState(dateTimeInputValue(proposal.normalizedStart ?? proposal.searchRangeStart));
  const [end, setEnd] = useState(dateTimeInputValue(proposal.normalizedEnd ?? proposal.searchRangeEnd));
  const [duration, setDuration] = useState(proposal.durationMinutes?.toString() ?? "30");
  const [timeZone, setTimeZone] = useState(proposal.timeZone ?? "");
  const titleRef = useRef<HTMLInputElement>(null);

  const submit = (event: React.FormEvent) => {
    event.preventDefault();
    if (!title.trim()) return;
    onSave({
      ...proposal,
      title: title.trim(),
      participants: participants.split(",").map((value) => value.trim()).filter(Boolean),
      location: location.trim() || null,
      normalizedStart: isoDateTime(start),
      normalizedEnd: isoDateTime(end),
      searchRangeStart: null,
      searchRangeEnd: null,
      durationMinutes: duration ? Number(duration) : null,
      timeZone: timeZone.trim() || null,
    });
  };

  return (
    <Modal title="Edit meeting proposal" className="task-editor-modal" onClose={onClose} initialFocusRef={titleRef}>
      <form className="modal-form" onSubmit={submit} onKeyDown={(event) => {
        if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
          event.preventDefault();
          event.currentTarget.requestSubmit();
        }
      }}>
        <label><span>Title</span><input ref={titleRef} value={title} onChange={(event) => setTitle(event.target.value)} /></label>
        <label><span>Participants</span><input value={participants} onChange={(event) => setParticipants(event.target.value)} placeholder="Comma-separated email addresses" /></label>
        <label><span>Start</span><input type="datetime-local" value={start} onChange={(event) => setStart(event.target.value)} /></label>
        <label><span>End</span><input type="datetime-local" value={end} onChange={(event) => setEnd(event.target.value)} /></label>
        <label><span>Duration (minutes)</span><input type="number" min="5" max="1440" value={duration} onChange={(event) => setDuration(event.target.value)} /></label>
        <label><span>Timezone</span><input value={timeZone} onChange={(event) => setTimeZone(event.target.value)} placeholder="America/New_York" /></label>
        <label><span>Location</span><input value={location} onChange={(event) => setLocation(event.target.value)} /></label>
        <div className="modal-form-evidence"><span>Evidence</span><blockquote>{proposal.evidence.excerpt}</blockquote></div>
        <div className="modal-form-actions"><button type="button" onClick={onClose}>Cancel</button><button type="submit" disabled={!title.trim()}>Save proposal</button></div>
      </form>
    </Modal>
  );
}
