import { useLayoutEffect, useRef, type KeyboardEvent } from "react";
import { addDays, dateInputValue } from "./calendarTime";

type WeekInfoLocale = Intl.Locale & { getWeekInfo?(): { firstDay?: number }; weekInfo?: { firstDay?: number } };

/**
 * The day a week starts on where `locale` is used, counted as `Date.getDay()`
 * counts (0 is Sunday). Sunday when the runtime can't say.
 */
export function firstDayOfWeek(locale = Intl.DateTimeFormat().resolvedOptions().locale): number {
  try {
    const info = new Intl.Locale(locale) as WeekInfoLocale;
    const firstDay = (info.getWeekInfo?.() ?? info.weekInfo)?.firstDay;
    return typeof firstDay === "number" && firstDay >= 1 && firstDay <= 7 ? firstDay % 7 : 0;
  } catch {
    return 0;
  }
}

/** The same day `months` months away, or the month's last day when it is shorter. Stays within years 1 to 9999. */
export function shiftMonth(date: Date, months: number): Date {
  const month = new Date(date);
  month.setDate(1);
  month.setMonth(month.getMonth() + months);
  const last = new Date(month);
  last.setMonth(last.getMonth() + 1);
  last.setDate(0);
  month.setDate(Math.min(date.getDate(), last.getDate()));
  return month.getFullYear() < 1 || month.getFullYear() > 9999 ? date : month;
}

const daysIntoWeek = (date: Date, weekStart: number) => (date.getDay() - weekStart + 7) % 7;

/** The six weeks a month grid shows for `cursor`'s month, as `YYYY-MM-DD` days. */
export function monthGridRange(cursor: Date, weekStart: number): { start: Date; from: string; to: string } {
  const first = new Date(cursor);
  first.setDate(1);
  first.setHours(12, 0, 0, 0);
  const start = addDays(first, -daysIntoWeek(first, weekStart));
  return { start, from: dateInputValue(start), to: dateInputValue(addDays(start, 41)) };
}

/**
 * A month of days as an ARIA grid: arrows move by day and week, Home and End
 * to the week's ends, Page Up and Page Down by month (with Shift, by year).
 * The grid moves `cursor`; choosing a day is a click or Enter on it. The
 * caller draws the month's heading and labels the grid with `labelledBy`.
 */
export function MonthGrid({ labelledBy, cursor, onCursorChange, selected, onSelect, marked, markLabel, focusCursor = "keyboard", weekStart = firstDayOfWeek() }: {
  labelledBy: string;
  cursor: Date;
  onCursorChange(date: Date): void;
  /** The chosen day, `YYYY-MM-DD`. */
  selected: string | null;
  onSelect(date: string): void;
  /** Days that carry a mark, such as a written daily note. */
  marked?: ReadonlySet<string>;
  /** What a mark means, added to a marked day's name for screen readers. */
  markLabel?: string;
  /**
   * When the cursor's day takes focus: "always" for a picker that opens onto
   * the grid; "keyboard" for a grid on the page, which must not take focus
   * when it appears or when its month changes from a button.
   */
  focusCursor?: "always" | "keyboard";
  weekStart?: number;
}) {
  const activeDay = useRef<HTMLButtonElement>(null);
  const moved = useRef(false);
  useLayoutEffect(() => {
    if (focusCursor === "always" || moved.current) activeDay.current?.focus();
    moved.current = false;
  }, [cursor, focusCursor]);
  const { start } = monthGridRange(cursor, weekStart);
  const today = dateInputValue(new Date());
  const cursorDate = dateInputValue(cursor);
  const fullDate = new Intl.DateTimeFormat(undefined, { weekday: "long", month: "long", day: "numeric", year: "numeric" });
  const keyDown = (event: KeyboardEvent) => {
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    let next: Date;
    switch (event.key) {
      case "ArrowLeft": next = addDays(cursor, -1); break;
      case "ArrowRight": next = addDays(cursor, 1); break;
      case "ArrowUp": next = addDays(cursor, -7); break;
      case "ArrowDown": next = addDays(cursor, 7); break;
      case "Home": next = addDays(cursor, -daysIntoWeek(cursor, weekStart)); break;
      case "End": next = addDays(cursor, 6 - daysIntoWeek(cursor, weekStart)); break;
      case "PageUp": next = shiftMonth(cursor, event.shiftKey ? -12 : -1); break;
      case "PageDown": next = shiftMonth(cursor, event.shiftKey ? 12 : 1); break;
      default: return;
    }
    event.preventDefault();
    event.stopPropagation();
    if (next.getFullYear() < 1 || next.getFullYear() > 9999) return;
    moved.current = true;
    onCursorChange(next);
  };
  return <table role="grid" className="month-grid" aria-labelledby={labelledBy} onKeyDown={keyDown}>
    <thead><tr>{Array.from({ length: 7 }, (_, day) => <th key={day} scope="col" abbr={new Intl.DateTimeFormat(undefined, { weekday: "long" }).format(addDays(start, day))}>
      {new Intl.DateTimeFormat(undefined, { weekday: "short" }).format(addDays(start, day))}
    </th>)}</tr></thead>
    <tbody>{Array.from({ length: 6 }, (_, week) => <tr key={week}>{Array.from({ length: 7 }, (_, weekday) => {
      const day = addDays(start, week * 7 + weekday);
      const date = dateInputValue(day);
      const active = date === cursorDate;
      const mark = marked?.has(date) ?? false;
      return <td key={date} role="gridcell" aria-selected={date === selected}>
        <button type="button" ref={active ? activeDay : undefined} tabIndex={active ? 0 : -1}
          className={["btn-icon", "month-grid-day", day.getMonth() !== cursor.getMonth() ? "month-grid-outside-month" : "", date === selected ? "month-grid-day-selected" : "", mark ? "month-grid-day-marked" : ""].filter(Boolean).join(" ")}
          aria-label={mark && markLabel ? `${fullDate.format(day)}, ${markLabel}` : fullDate.format(day)} aria-current={date === today ? "date" : undefined}
          onClick={() => onSelect(date)}>{day.getDate()}</button>
      </td>;
    })}</tr>)}</tbody>
  </table>;
}
