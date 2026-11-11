// Expands an offered or checked time into the day around it. The include
// checkbox (when present) stays a separate choice from opening the day.

import { ChevronDown } from "lucide-react";
import type { ReactNode } from "react";
import type { ScheduleEvent } from "./domain";
import { SlotDayPreview, type SlotContext } from "./SlotDayPreview";
import { ICON_SIZE } from "./iconSizes";

export type DaySlot = { start: string; end: string };

/**
 * A timed slot with an optional leading control (usually the include
 * checkbox), a break warning, and the day preview when expanded. Without a
 * resolvable `slot`, the label stays static so the checkbox is not remounted
 * when the calendar check finishes.
 */
export function SlotDayToggle({
  id,
  label,
  slot,
  timeZone,
  warning = null,
  open,
  onToggle,
  leading,
  checked = false,
  events,
  context,
  children,
}: {
  id: string;
  label: ReactNode;
  slot: DaySlot | null;
  timeZone: string;
  warning?: string | null;
  open: boolean;
  onToggle(): void;
  leading?: ReactNode;
  checked?: boolean;
  /** The day, when the caller has just read it. */
  events?: readonly ScheduleEvent[] | null;
  /** What a calendar check found, so a blocking overlap reads as a conflict. */
  context?: SlotContext;
  /** Status or notes between the day button and the preview. */
  children?: ReactNode;
}) {
  return <div className="meeting-slot-item">
    <div className={`meeting-slot-row${checked ? " meeting-slot-row-checked" : ""}`}>
      {leading}
      {slot ? <button
        type="button"
        className="meeting-slot-day"
        aria-expanded={open}
        aria-controls={open ? id : undefined}
        title="Show this day on your calendar"
        onClick={onToggle}
      >
        <span>{label}</span>
        {warning ? <span className="meeting-slot-warning">{warning}</span> : null}
        <ChevronDown className="meeting-slot-chevron" size={ICON_SIZE.sm} aria-hidden="true" />
      </button> : <span className="meeting-slot-day meeting-slot-day-static"><span>{label}</span></span>}
    </div>
    {children}
    {slot && open ? <SlotDayPreview id={id} slot={slot} timeZone={timeZone} events={events} context={context} /> : null}
  </div>;
}
