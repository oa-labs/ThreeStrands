import { Building, House, MapPin } from "lucide-react";
import type { ButtonHTMLAttributes, CSSProperties } from "react";
import { formatEventTime } from "./calendarTime";
import type { ScheduleEvent } from "./domain";
import { ICON_SIZE } from "./iconSizes";

/** A working location says where the user is that day, not something they are doing. */
export const isWorkingLocation = (event: ScheduleEvent) => event.eventType === "workingLocation";

/**
 * Each place once, in order. Every calendar that shares a working location
 * repeats it, so one place can arrive several times for the same day.
 */
export function distinctWorkingLocations(events: ScheduleEvent[]): ScheduleEvent[] {
  const seen = new Set<string>();
  return events.filter((event) => {
    if (!isWorkingLocation(event)) return false;
    const key = `${event.title.trim().toLocaleLowerCase()}|${event.allDay ? "" : `${event.start}|${event.end}`}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

export function WorkingLocationIcon({ type }: { type: ScheduleEvent["workingLocationType"] }) {
  const Icon = type === "home" ? House : type === "office" ? Building : MapPin;
  return <Icon size={ICON_SIZE.sm} aria-hidden="true" />;
}

/**
 * A working location in a calendar's all-day row: the place and its icon on
 * a thin rule, unfilled, so it reads as where the user is rather than as an
 * event. One set for part of a day names its hours.
 */
export function WorkingLocationChip({ event, style, ...button }: { event: ScheduleEvent; style?: CSSProperties } & Omit<ButtonHTMLAttributes<HTMLButtonElement>, "children" | "type" | "className">) {
  const hours = event.allDay ? null : formatEventTime(event);
  return (
    <button
      type="button"
      className="calendar-working-location"
      data-working-location={event.workingLocationType ?? "custom"}
      aria-label={`${event.title}, working location${hours ? `, ${hours}` : ""}`}
      title={`Working location${hours ? `, ${hours}` : ""}`}
      style={style}
      {...button}
    >
      <WorkingLocationIcon type={event.workingLocationType} />
      <span className="calendar-working-location-name">{event.title}</span>
      {hours ? <span className="calendar-working-location-hours">{hours}</span> : null}
    </button>
  );
}
