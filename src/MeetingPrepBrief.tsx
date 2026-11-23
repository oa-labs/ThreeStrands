import { Calendar, CheckSquare, Handshake, Mail, NotebookText, Sparkles } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import { Modal } from "./AppChrome";
import { AttachmentIcon } from "./AttachmentIcon";
import { readAiRequestConfig } from "./aiSettings";
import { mailClient } from "./data/client";
import type {
  MeetingAgendaSave,
  MeetingCommitment,
  MeetingContext,
  MeetingEvidenceCitation,
  MeetingPrepBrief,
  MeetingPrepSelection,
  MeetingPrepSourceItem,
  MeetingSourcePreview,
  ScheduleEvent,
} from "./domain";
import { errorMessage } from "./errors";
import { ICON_SIZE } from "./iconSizes";
import { saveMeetingAgenda } from "./noteCapture";
import type { NoteReference } from "./noteEntities";

const SOURCE_KIND_LABELS: Record<MeetingPrepSourceItem["kind"], string> = {
  event: "Invitation",
  note: "Notes",
  conversation: "Conversations",
  promise: "Open promises",
  commitment: "Open commitments",
  file: "Files",
};

const SOURCE_KIND_ORDER: readonly MeetingPrepSourceItem["kind"][] = [
  "event",
  "note",
  "conversation",
  "promise",
  "commitment",
  "file",
];

const OWNER_LABELS: Record<string, string> = {
  you: "You",
  attendee: "Attendee",
  unclear: "Unclear",
};

function selectionFromPreview(preview: MeetingSourcePreview): MeetingPrepSelection {
  const noteIds = preview.sources
    .filter((item) => item.kind === "note" && item.selected && !item.excludedByNoteSetting)
    .map((item) => item.id);
  const threadIds = preview.sources
    .filter((item) => item.kind === "conversation" && item.selected)
    .map((item) => item.id);
  const has = (kind: MeetingPrepSourceItem["kind"]) =>
    preview.sources.some((item) => item.kind === kind && item.selected);
  return {
    noteIds,
    threadIds,
    includeInviteDescription: has("event"),
    includeCommitments: has("commitment"),
    includePromises: has("promise"),
    includeFiles: has("file"),
  };
}

function toggleSourceInSelection(
  preview: MeetingSourcePreview,
  item: MeetingPrepSourceItem,
  checked: boolean,
): MeetingPrepSelection {
  const current = selectionFromPreview(preview);
  if (item.kind === "note") {
    const next = new Set(current.noteIds ?? []);
    if (checked) next.add(item.id);
    else next.delete(item.id);
    return { ...current, noteIds: [...next] };
  }
  if (item.kind === "conversation") {
    const next = new Set(current.threadIds ?? []);
    if (checked) next.add(item.id);
    else next.delete(item.id);
    return { ...current, threadIds: [...next] };
  }
  if (item.kind === "event") return { ...current, includeInviteDescription: checked };
  if (item.kind === "commitment") return { ...current, includeCommitments: checked };
  if (item.kind === "promise") return { ...current, includePromises: checked };
  return { ...current, includeFiles: checked };
}

function sourceIcon(kind: string, title: string) {
  if (kind === "note") return <NotebookText size={ICON_SIZE.xs} aria-hidden="true" />;
  if (kind === "conversation") return <Mail size={ICON_SIZE.xs} aria-hidden="true" />;
  if (kind === "promise") return <Handshake size={ICON_SIZE.xs} aria-hidden="true" />;
  if (kind === "commitment") return <CheckSquare size={ICON_SIZE.xs} aria-hidden="true" />;
  if (kind === "file") return <AttachmentIcon filename={title} mimeType="" size="sm" />;
  return <Calendar size={ICON_SIZE.xs} aria-hidden="true" />;
}

/**
 * Source review modal shown before any meeting data leaves the device.
 * Pre-selects the top relevant non-excluded local sources so the user can
 * review, toggle anything on or off, and click Generate Agenda in one step.
 */
export function MeetingPrepSourcesDialog({
  event,
  reference,
  mailScope,
  initialSelection,
  onClose,
  onGenerated,
}: {
  event: ScheduleEvent;
  reference: NoteReference;
  mailScope: string[] | null;
  initialSelection: MeetingPrepSelection | null;
  onClose(): void;
  onGenerated(brief: MeetingPrepBrief, selection: MeetingPrepSelection): void;
}) {
  const [selection, setSelection] = useState<MeetingPrepSelection | null>(initialSelection);
  const [preview, setPreview] = useState<MeetingSourcePreview | null>(null);
  const [loading, setLoading] = useState(true);
  const [generating, setGenerating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const primaryRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    let active = true;
    setLoading(true);
    setError(null);
    void mailClient
      .meetingSourcePreview(reference, mailScope, selection)
      .then((next) => {
        if (!active) return;
        setPreview(next);
        setLoading(false);
      })
      .catch((reason) => {
        if (!active) return;
        setError(errorMessage(reason));
        setLoading(false);
      });
    return () => { active = false; };
  }, [reference, mailScope, selection]);

  const selectedCount = useMemo(
    () => preview?.sources.filter((item) => item.selected && !item.excludedByNoteSetting).length ?? 0,
    [preview],
  );

  const grouped = useMemo(() => {
    if (!preview) return [];
    return SOURCE_KIND_ORDER
      .map((kind) => ({
        kind,
        label: SOURCE_KIND_LABELS[kind],
        items: preview.sources.filter((item) => item.kind === kind),
      }))
      .filter((group) => group.items.length > 0);
  }, [preview]);

  const handleToggle = (item: MeetingPrepSourceItem, checked: boolean) => {
    if (!preview || item.excludedByNoteSetting || generating) return;
    setSelection(toggleSourceInSelection(preview, item, checked));
  };

  const handleSubmit = async (submitted: FormEvent) => {
    submitted.preventDefault();
    if (!preview || generating || loading || preview.overBudget || selectedCount === 0) return;
    setGenerating(true);
    setError(null);
    try {
      const config = readAiRequestConfig("preparing a meeting agenda", "meetingPrep");
      const effectiveSelection = selectionFromPreview(preview);
      const brief = await mailClient.prepareMeetingBrief(
        reference,
        mailScope,
        effectiveSelection,
        preview.payloadRevision,
        config.provider,
        config.model,
        config.endpoint,
        config.reasoning,
      );
      onGenerated(brief, effectiveSelection);
    } catch (reason) {
      setError(errorMessage(reason));
      setGenerating(false);
    }
  };

  const overBudget = Boolean(preview?.overBudget);
  const canGenerate = Boolean(preview && !loading && !generating && !overBudget && selectedCount > 0);

  return (
    <Modal title="Prepare Meeting Agenda" className="meeting-prep-sources-modal" initialFocusRef={primaryRef} onClose={onClose}>
      <form className="modal-form" onSubmit={(submitted) => void handleSubmit(submitted)}>
        <p className="modal-form-context">{event.title}</p>
        <p className="modal-form-hint">
          Only the checked sources below are sent to your AI provider. Review what is included, uncheck anything private, then generate a source-cited agenda.
        </p>
        {preview && preview.excludedNoteTitles.length > 0 ? (
          <p className="modal-form-hint" role="note">
            Excluded from AI ({preview.excludedNoteTitles.length}): {preview.excludedNoteTitles.join(", ")}
          </p>
        ) : null}
        {loading && !preview ? <p className="context-status" role="status">Reviewing local sources…</p> : null}
        {preview ? (
          <>
            {grouped.length === 0 ? (
              <p className="context-status">No local notes, emails, promises, commitments, or files were found for this meeting yet.</p>
            ) : (
              <div className="meeting-prep-sources-list">
                {grouped.map((group) => (
                  <fieldset key={group.kind} className="meeting-prep-source-group" disabled={generating}>
                    <legend className="eyebrow">{group.label}</legend>
                    {group.items.map((item) => (
                      <label key={item.sourceId} className={`modal-form-check meeting-prep-source-item${item.excludedByNoteSetting ? " is-excluded" : ""}`}>
                        <input
                          type="checkbox"
                          checked={item.selected && !item.excludedByNoteSetting}
                          disabled={item.excludedByNoteSetting || generating}
                          onChange={(change) => handleToggle(item, change.target.checked)}
                        />
                        <span className="meeting-prep-source-body">
                          <span className="meeting-prep-source-title">{item.title}</span>
                          <span className="meeting-prep-source-meta">
                            {[
                              item.excludedByNoteSetting ? "Excluded from AI" : null,
                              item.detail,
                              item.characters > 0 ? `${item.characters.toLocaleString()} chars` : null,
                              item.truncated ? "trimmed" : null,
                            ].filter(Boolean).join(" · ")}
                          </span>
                        </span>
                      </label>
                    ))}
                  </fieldset>
                ))}
              </div>
            )}
            <p className={`meeting-prep-budget${overBudget ? " is-over" : ""}`} role="status">
              {selectedCount} {selectedCount === 1 ? "source" : "sources"} selected ·{" "}
              {preview.totalSelectedCharacters.toLocaleString()} / {preview.budgetCharacters.toLocaleString()} characters
              {overBudget ? " — uncheck some sources to fit within the budget." : ""}
            </p>
          </>
        ) : null}
        {error ? <p className="form-error" role="alert">{error}</p> : null}
        <div className="modal-form-actions">
          <button type="button" className="btn" onClick={onClose} disabled={generating}>Cancel</button>
          <button ref={primaryRef} type="submit" className="btn btn-primary" disabled={!canGenerate}>
            <Sparkles size={ICON_SIZE.sm} aria-hidden="true" />
            {generating ? "Generating…" : "Generate Agenda"}
          </button>
        </div>
      </form>
    </Modal>
  );
}

/**
 * Displays the generated AI meeting preparation brief with clickable evidence
 * citations and one-click insertion of selected agenda/follow-up items into
 * the meeting's note (with Undo).
 */
export function MeetingPrepBriefCard({
  event,
  context,
  brief,
  ownAddresses,
  onOpenNote,
  onOpenThread,
  onOpenTask,
  onChangeSources,
  onDismiss,
  onAgendaSaved,
}: {
  event: ScheduleEvent;
  context: MeetingContext;
  brief: MeetingPrepBrief;
  ownAddresses: readonly string[];
  onOpenNote(noteId: string, href?: string): void;
  onOpenThread(threadId: string): void;
  onOpenTask(commitment: MeetingCommitment): void;
  onChangeSources(): void;
  onDismiss(): void;
  onAgendaSaved?(saved: MeetingAgendaSave & { undo(): Promise<void> }): void;
}) {
  const [checkedAgenda, setCheckedAgenda] = useState<Set<number>>(
    () => new Set(brief.agendaItems.map((_, index) => index)),
  );
  const [checkedFollowUps, setCheckedFollowUps] = useState<Set<number>>(
    () => new Set(brief.followUps.map((_, index) => index)),
  );
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [savedOnce, setSavedOnce] = useState(false);

  useEffect(() => {
    setCheckedAgenda(new Set(brief.agendaItems.map((_, index) => index)));
    setCheckedFollowUps(new Set(brief.followUps.map((_, index) => index)));
    setSaveError(null);
    setSavedOnce(false);
  }, [brief]);

  const openCitation = (citation: MeetingEvidenceCitation) => {
    if (citation.kind === "note") {
      onOpenNote(citation.id);
      return;
    }
    if (citation.kind === "conversation" || citation.kind === "promise") {
      onOpenThread(citation.id);
      return;
    }
    if (citation.kind === "commitment") {
      const found = context.commitments.items.find((item) => item.id === citation.id);
      if (found) {
        onOpenTask(found);
      } else {
        onOpenTask({
          id: citation.id,
          title: citation.title,
          status: "open",
          noteId: null,
          threadId: null,
          accountId: null,
          origin: "meetingNote",
          candidate: false,
        });
      }
      return;
    }
    if (citation.kind === "file") {
      const file = context.files?.items.find((item) => item.attachmentId === citation.id);
      if (file?.source === "mail" && file.threadId) onOpenThread(file.threadId);
      else if (file?.source === "note" && file.noteId) onOpenNote(file.noteId);
    }
  };

  const renderCitations = (citations: readonly MeetingEvidenceCitation[]) => (
    <div className="meeting-prep-citations">
      {citations.map((citation) => {
        const interactive = citation.kind === "note"
          || citation.kind === "conversation"
          || citation.kind === "promise"
          || citation.kind === "commitment"
          || (citation.kind === "file" && Boolean(context.files?.items.find((item) => item.attachmentId === citation.id)));
        return interactive ? (
          <button
            key={citation.sourceId}
            type="button"
            className="meeting-prep-citation"
            onClick={() => openCitation(citation)}
          >
            {sourceIcon(citation.kind, citation.title)}
            <span>{citation.title}</span>
          </button>
        ) : (
          <span key={citation.sourceId} className="meeting-prep-citation is-static">
            {sourceIcon(citation.kind, citation.title)}
            <span>{citation.title}</span>
          </span>
        );
      })}
    </div>
  );

  const selectedItems = useMemo(() => {
    const agendaLines = brief.agendaItems
      .filter((_, index) => checkedAgenda.has(index))
      .map((item) => (item.detail ? `${item.topic} — ${item.detail}` : item.topic));
    const followUpLines = brief.followUps
      .filter((_, index) => checkedFollowUps.has(index))
      .map((item) => `Follow-up: ${item.text}`);
    return [...agendaLines, ...followUpLines];
  }, [brief, checkedAgenda, checkedFollowUps]);

  const handleAddToNote = async () => {
    if (!selectedItems.length || saving) return;
    setSaving(true);
    setSaveError(null);
    try {
      const preferred = context.thisMeeting.items.find((item) => item.kind === "excerpt")
        ?? context.thisMeeting.items[0]
        ?? null;
      const destination = preferred
        ? { noteId: preferred.noteId, dailyDate: preferred.dailyDate, revision: preferred.revision }
        : null;
      const saved = await saveMeetingAgenda(
        event,
        ownAddresses,
        destination,
        crypto.randomUUID(),
        selectedItems,
      );
      setSavedOnce(true);
      onAgendaSaved?.(saved);
    } catch (reason) {
      setSaveError(errorMessage(reason));
    } finally {
      setSaving(false);
    }
  };

  const hasContent = brief.agendaItems.length > 0 || brief.followUps.length > 0 || brief.recentContext.length > 0;

  return (
    <section className="meeting-prep-brief" aria-label="Suggested agenda brief">
      <div className="meeting-prep-brief-header">
        <span className="eyebrow meeting-prep-brief-title">
          <Sparkles size={ICON_SIZE.xs} aria-hidden="true" />
          Suggested Agenda
        </span>
        <div className="meeting-prep-brief-header-actions">
          <button type="button" className="btn-link" onClick={onChangeSources}>Change Sources…</button>
          <button type="button" className="btn-link" onClick={onDismiss}>Dismiss</button>
        </div>
      </div>

      {!hasContent ? (
        <p className="context-status">No verified agenda items could be grounded in the selected sources.</p>
      ) : null}

      {brief.agendaItems.length > 0 ? (
        <ul className="meeting-prep-brief-list" aria-label="Suggested discussion items">
          {brief.agendaItems.map((item, index) => (
            <li key={`${index}:${item.topic}`} className="meeting-prep-brief-item">
              <label className="meeting-prep-brief-check">
                <input
                  type="checkbox"
                  checked={checkedAgenda.has(index)}
                  onChange={(change) => {
                    setSavedOnce(false);
                    setCheckedAgenda((current) => {
                      const next = new Set(current);
                      if (change.target.checked) next.add(index);
                      else next.delete(index);
                      return next;
                    });
                  }}
                />
                <span className="meeting-prep-brief-item-body">
                  <span className="meeting-prep-brief-topic">{item.topic}</span>
                  {item.detail ? <span className="meeting-prep-brief-detail">{item.detail}</span> : null}
                </span>
              </label>
              {renderCitations(item.citations)}
            </li>
          ))}
        </ul>
      ) : null}

      {brief.followUps.length > 0 ? (
        <div className="meeting-prep-brief-sub">
          <h5 className="eyebrow meeting-prep-brief-subhead">Follow-ups &amp; Past Promises</h5>
          <ul className="meeting-prep-brief-list" aria-label="Follow-ups and past promises">
            {brief.followUps.map((item, index) => (
              <li key={`${index}:${item.text}`} className="meeting-prep-brief-item">
                <label className="meeting-prep-brief-check">
                  <input
                    type="checkbox"
                    checked={checkedFollowUps.has(index)}
                    onChange={(change) => {
                      setSavedOnce(false);
                      setCheckedFollowUps((current) => {
                        const next = new Set(current);
                        if (change.target.checked) next.add(index);
                        else next.delete(index);
                        return next;
                      });
                    }}
                  />
                  <span className="meeting-prep-brief-item-body">
                    <span className="meeting-prep-brief-topic">
                      {item.text}{" "}
                      <span className="meeting-prep-owner-badge">{OWNER_LABELS[item.owner] ?? "Unclear"}</span>
                    </span>
                  </span>
                </label>
                {renderCitations(item.citations)}
              </li>
            ))}
          </ul>
        </div>
      ) : null}

      {brief.recentContext.length > 0 ? (
        <div className="meeting-prep-brief-sub">
          <h5 className="eyebrow meeting-prep-brief-subhead">Recent Context</h5>
          <ul className="meeting-prep-brief-context" aria-label="Recent context">
            {brief.recentContext.map((item, index) => (
              <li key={`${index}:${item.summary}`} className="meeting-prep-brief-item">
                <p className="meeting-prep-brief-detail">{item.summary}</p>
                {renderCitations(item.citations)}
              </li>
            ))}
          </ul>
        </div>
      ) : null}

      {brief.withheldCount > 0 ? (
        <p className="context-status">
          {brief.withheldCount === 1
            ? "1 unverified suggestion was withheld."
            : `${brief.withheldCount} unverified suggestions were withheld.`}
        </p>
      ) : null}

      {saveError ? <p className="form-error" role="alert">{saveError}</p> : null}

      {brief.agendaItems.length > 0 || brief.followUps.length > 0 ? (
        <div className="meeting-prep-brief-footer">
          <button
            type="button"
            className="btn btn-sm btn-primary"
            disabled={selectedItems.length === 0 || saving}
            onClick={() => void handleAddToNote()}
          >
            {saving ? "Adding…" : savedOnce ? "Added to Note" : "Add Selected to Note"}
          </button>
        </div>
      ) : null}
    </section>
  );
}
