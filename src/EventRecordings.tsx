import { useCallback, useEffect, useState } from "react";
import { ClipboardCheck, MessageSquareText, RotateCw, Trash2 } from "lucide-react";
import { mailClient } from "./data/client";
import { ContextRow, ContextSection } from "./ContextSections";
import { AttachmentIcon } from "./AttachmentIcon";
import { formatCompactDate } from "./dateFormatting";
import type { MeetingRecording } from "./domain";
import { errorMessage } from "./errors";
import { ICON_SIZE } from "./iconSizes";
import { InlineConfirm } from "./InlineConfirm";
import { noteReferenceKey, type NoteReference } from "./noteEntities";
import { canRetryTranscription, canShowTranscript, recordingDetail } from "./recordingPolicy";
import { listenForRecordingEvent } from "./recordingEvents";
import { TranscriptDialog } from "./TranscriptDialog";
import { useRecording } from "./recordingContext";

/** The recordings of one occurrence, read again when the recorder says they changed. */
export function useEventRecordings(occurrence: NoteReference, enabled: boolean, version: number) {
  const [rows, setRows] = useState<MeetingRecording[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [reload, setReload] = useState(0);
  const key = noteReferenceKey(occurrence);
  const refresh = useCallback(() => setReload((count) => count + 1), []);
  useEffect(() => {
    let active = true;
    setRows([]);
    setError(null);
    if (!enabled) return;
    mailClient.listRecordings(occurrence).then((found) => { if (active) setRows(found); }, (reason: unknown) => { if (active) setError(errorMessage(reason)); });
    return () => { active = false; };
    // The reference's content is the identity, not the caller's object.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, enabled, version, reload]);
  return { rows, error, refresh };
}

/** The latest whole percent of each transcription, from `transcription-progress`. */
function useTranscriptionProgress(enabled: boolean) {
  const [percents, setPercents] = useState<Record<string, number>>({});
  useEffect(() => {
    if (!enabled) return;
    return listenForRecordingEvent("transcription-progress", ({ recordingId, percent }) => setPercents((current) => (current[recordingId] === percent ? current : { ...current, [recordingId]: percent })));
  }, [enabled]);
  return percents;
}

/**
 * An event's Recording section, in the Meeting context after Notes: one row
 * per recording of this occurrence. It is left out while there are none.
 */
export function EventRecordings({ occurrence, onReview }: { occurrence: NoteReference; onReview?(recording: MeetingRecording): void }) {
  const recording = useRecording();
  const { rows, error, refresh } = useEventRecordings(occurrence, recording.supported, recording.recordingsVersion);
  const [confirming, setConfirming] = useState<MeetingRecording | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [viewing, setViewing] = useState<MeetingRecording | null>(null);
  const percents = useTranscriptionProgress(recording.supported);
  const retry = async (target: MeetingRecording) => {
    setDeleteError(null);
    try {
      await mailClient.transcribeRecording(target.id);
      refresh();
    } catch (reason) {
      setDeleteError(errorMessage(reason));
    }
  };
  const remove = async (target: MeetingRecording) => {
    setBusy(true);
    setDeleteError(null);
    try {
      await mailClient.deleteRecording(target.id);
      setConfirming(null);
      refresh();
    } catch (reason) {
      setDeleteError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  };
  if (!recording.supported) return null;
  const failure = error ?? deleteError;
  const dialog = viewing ? <TranscriptDialog recording={viewing} onClose={() => setViewing(null)} onDeleted={() => { setViewing(null); refresh(); }} /> : null;
  // Rows are briefly empty while they are read again; the open transcript stays.
  if (!rows.length && !failure) return dialog;
  return <>
    {dialog}
    {failure ? <p role="alert" className="form-error">{failure}</p> : null}
    {rows.length ? <ContextSection
      id="event-recordings"
      title="Recording"
      count={rows.length}
      rows={rows.map((row) => (
        <ContextRow
          key={row.id}
          icon={<AttachmentIcon filename="recording.m4a" mimeType="audio/mp4" size="sm" />}
          title="Recording"
          date={formatCompactDate(new Date(row.startedAt))}
          detail={recordingDetail(row, recording.transcriptionModel, percents[row.id] ?? null, recording.usingSpeechAnalyzer)}
          actions={row.state === "recording" ? undefined : [
            ...(canShowTranscript(row) ? [
              <button key="review" type="button" className="btn-icon btn-icon-sm" aria-label="Review" title="Review" onClick={() => (onReview ? onReview(row) : recording.openReview(row))}>
                <ClipboardCheck size={ICON_SIZE.sm} aria-hidden="true" />
              </button>,
              <button key="transcript" type="button" className="btn-icon btn-icon-sm" aria-label="Show Transcript" title="Show Transcript" onClick={() => setViewing(row)}>
                <MessageSquareText size={ICON_SIZE.sm} aria-hidden="true" />
              </button>,
            ] : []),
            ...(canRetryTranscription(row) ? [
              <button key="retry" type="button" className="btn-icon btn-icon-sm" aria-label="Retry Transcription" title="Retry Transcription" onClick={() => void retry(row)}>
                <RotateCw size={ICON_SIZE.sm} aria-hidden="true" />
              </button>,
            ] : []),
            <button key="delete" type="button" className="btn-icon btn-icon-sm" aria-label="Delete Recording" title="Delete Recording" onClick={() => { setDeleteError(null); setConfirming(row); }}>
              <Trash2 size={ICON_SIZE.sm} aria-hidden="true" />
            </button>,
          ]}
        />
      ))}
      closing={confirming ? <InlineConfirm
        ariaLabel="Confirm Delete Recording"
        title="Delete this recording?"
        cancelLabel="Cancel"
        onCancel={() => setConfirming(null)}
        disabled={busy}
        stacked
        focusCancel
        actions={[{ label: "Delete Recording", className: "btn-danger", onClick: () => void remove(confirming) }]}
      >
        The recording and its audio are removed from this Mac. This can't be undone.
      </InlineConfirm> : undefined}
    /> : null}
  </>;
}
