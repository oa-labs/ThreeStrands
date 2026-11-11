import { useEffect, useId, useState } from "react";
import { Calendar1, ChevronLeft, ChevronRight } from "lucide-react";
import { ContextSection } from "./ContextSections";
import { parseCalendarDate } from "./DatePickerField";
import { firstDayOfWeek, MonthGrid, monthGridRange, shiftMonth } from "./MonthGrid";
import { mailClient } from "./data/client";
import { logBackgroundFailure } from "./errors";
import { formatShortcut } from "./shortcutLabels";
import { ICON_SIZE } from "./iconSizes";

/**
 * The Notes rail's month: choosing a day opens its daily note, and a dot
 * marks each day whose note has text. The month follows the open daily note,
 * and can be paged without opening anything. `revision` changes whenever a
 * note may have been written, trashed, or synced, so the marks are reread.
 */
export function NotesDayCalendar({ selected, today, onSelect, revision, reveal = 0 }: {
  /** The open daily note's day, or null while a regular note is open. */
  selected: string | null;
  today: string;
  onSelect(date: string): void;
  revision: number;
  /** Changes when a day is chosen again, so a paged month returns to it. */
  reveal?: number;
}) {
  const [cursor, setCursor] = useState(() => parseCalendarDate(selected ?? today) ?? new Date());
  useEffect(() => {
    const day = selected ? parseCalendarDate(selected) : null;
    if (day) setCursor(day);
  }, [selected, reveal]);
  const weekStart = firstDayOfWeek();
  const { from, to } = monthGridRange(cursor, weekStart);
  const [marked, setMarked] = useState<{ from: string; days: ReadonlySet<string> } | null>(null);
  useEffect(() => {
    let active = true;
    void mailClient.listDailyNoteDates(from, to).then((days) => { if (active) setMarked({ from, days: new Set(days) }); }, logBackgroundFailure("Daily note dates"));
    return () => { active = false; };
  }, [from, to, revision]);
  const monthId = useId();
  const month = new Intl.DateTimeFormat(undefined, { month: "long", year: "numeric" }).format(cursor);
  return <ContextSection id="notes-calendar" label="Calendar" className="notes-calendar" rows={[]}
    title={<span id={monthId}>{month}</span>}
    actions={<>
      <button type="button" className="btn-icon btn-icon-sm" aria-label="Previous Month" title="Previous Month" onClick={() => setCursor(shiftMonth(cursor, -1))}><ChevronLeft size={ICON_SIZE.sm} aria-hidden="true" /></button>
      <button type="button" className="btn-icon btn-icon-sm" aria-label="Jump to Today" title={`Jump to Today (${formatShortcut("t")})`} aria-keyshortcuts="t" onClick={() => onSelect(today)}><Calendar1 size={ICON_SIZE.sm} aria-hidden="true" /></button>
      <button type="button" className="btn-icon btn-icon-sm" aria-label="Next Month" title="Next Month" onClick={() => setCursor(shiftMonth(cursor, 1))}><ChevronRight size={ICON_SIZE.sm} aria-hidden="true" /></button>
    </>}
    closing={<MonthGrid labelledBy={monthId} cursor={cursor} onCursorChange={setCursor} selected={selected} onSelect={onSelect} weekStart={weekStart}
      marked={marked?.from === from ? marked.days : undefined} markLabel="has a note" weekdays="narrow" />} />;
}
