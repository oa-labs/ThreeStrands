import { CalendarDays, ChevronLeft, ChevronRight } from "lucide-react";
import { useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent } from "react";
import { addDays, dateInputValue } from "./calendarTime";
import { ICON_SIZE } from "./iconSizes";
import { useEscapeDismiss } from "./useEscapeDismiss";

/** Parse a local calendar day without UTC conversion or Date's rollover of invalid days. */
export function parseCalendarDate(value: string): Date | null {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(value) || value.startsWith("0000")) return null;
  const date = new Date(`${value}T12:00:00`);
  return Number.isFinite(date.getTime()) && dateInputValue(date) === value ? date : null;
}

export function DatePickerField({ label, value, onChange }: {
  label: string;
  value: string;
  onChange(value: string): void;
}) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const popupId = useId();
  const close = () => { setOpen(false); input.current?.focus(); };
  useEffect(() => {
    input.current?.setCustomValidity(value && !parseCalendarDate(value) ? "Enter a valid date (YYYY-MM-DD)." : "");
  }, [value]);
  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => {
      if (!root.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", outside);
    return () => document.removeEventListener("pointerdown", outside);
  }, [open]);

  return <div className="date-picker-field" ref={root} onBlur={(event) => {
    if (!event.currentTarget.contains(event.relatedTarget)) setOpen(false);
  }}>
    <label><span className="sr-only">{label}</span><input
      ref={input} type="text" value={value} required placeholder="YYYY-MM-DD"
      pattern="\d{4}-\d{2}-\d{2}" maxLength={10} autoComplete="off"
      onChange={(event) => onChange(event.target.value)}
      onKeyDown={(event) => {
        if (event.key === "ArrowDown") { event.preventDefault(); setOpen(true); }
      }}
    /></label>
    <button type="button" className="btn-icon btn-icon-sm date-picker-trigger" aria-label={`Choose ${label.toLowerCase()}`}
      aria-haspopup="dialog" aria-expanded={open} aria-controls={open ? popupId : undefined}
      onClick={() => setOpen(!open)}><CalendarDays size={ICON_SIZE.sm} /></button>
    {open ? <CalendarDatePicker id={popupId} label={label} value={value} onClose={close}
      onSelect={(date) => { onChange(date); close(); }} /> : null}
  </div>;
}

function shiftMonth(date: Date, months: number): Date {
  const month = new Date(date);
  month.setDate(1);
  month.setMonth(month.getMonth() + months);
  const last = new Date(month);
  last.setMonth(last.getMonth() + 1);
  last.setDate(0);
  month.setDate(Math.min(date.getDate(), last.getDate()));
  return month.getFullYear() < 1 || month.getFullYear() > 9999 ? date : month;
}

function CalendarDatePicker({ id, label, value, onSelect, onClose }: {
  id: string;
  label: string;
  value: string;
  onSelect(value: string): void;
  onClose(): void;
}) {
  const [cursor, setCursor] = useState(() => parseCalendarDate(value) ?? new Date());
  const activeDay = useRef<HTMLButtonElement>(null);
  useEscapeDismiss(onClose);
  useLayoutEffect(() => { activeDay.current?.focus(); }, [cursor]);
  const first = new Date(cursor);
  first.setDate(1);
  first.setHours(12, 0, 0, 0);
  const gridStart = addDays(first, -first.getDay());
  const today = dateInputValue(new Date());
  const title = new Intl.DateTimeFormat(undefined, { month: "long", year: "numeric" }).format(cursor);
  const fullDate = new Intl.DateTimeFormat(undefined, { weekday: "long", month: "long", day: "numeric", year: "numeric" });
  const keyDown = (event: KeyboardEvent) => {
    let next: Date;
    switch (event.key) {
      case "ArrowLeft": next = addDays(cursor, -1); break;
      case "ArrowRight": next = addDays(cursor, 1); break;
      case "ArrowUp": next = addDays(cursor, -7); break;
      case "ArrowDown": next = addDays(cursor, 7); break;
      case "Home": next = addDays(cursor, -cursor.getDay()); break;
      case "End": next = addDays(cursor, 6 - cursor.getDay()); break;
      case "PageUp": next = shiftMonth(cursor, event.shiftKey ? -12 : -1); break;
      case "PageDown": next = shiftMonth(cursor, event.shiftKey ? 12 : 1); break;
      default: return;
    }
    event.preventDefault();
    event.stopPropagation();
    if (next.getFullYear() >= 1 && next.getFullYear() <= 9999) setCursor(next);
  };
  return <div id={id} className="date-picker-panel" role="dialog" aria-label={`Choose ${label.toLowerCase()}`}>
    <div className="date-picker-heading">
      <button type="button" className="btn-icon btn-icon-sm" aria-label="Previous month" onClick={() => setCursor(shiftMonth(cursor, -1))}><ChevronLeft size={ICON_SIZE.sm} /></button>
      <span id={`${id}-month`} aria-live="polite">{title}</span>
      <button type="button" className="btn-icon btn-icon-sm" aria-label="Next month" onClick={() => setCursor(shiftMonth(cursor, 1))}><ChevronRight size={ICON_SIZE.sm} /></button>
    </div>
    <table role="grid" aria-labelledby={`${id}-month`} onKeyDown={keyDown}>
      <thead><tr>{Array.from({ length: 7 }, (_, day) => <th key={day} scope="col" abbr={new Intl.DateTimeFormat(undefined, { weekday: "long" }).format(addDays(gridStart, day))}>
        {new Intl.DateTimeFormat(undefined, { weekday: "short" }).format(addDays(gridStart, day))}
      </th>)}</tr></thead>
      <tbody>{Array.from({ length: 6 }, (_, week) => <tr key={week}>{Array.from({ length: 7 }, (_, weekday) => {
        const day = addDays(gridStart, week * 7 + weekday);
        const date = dateInputValue(day);
        const active = date === dateInputValue(cursor);
        return <td key={date} role="gridcell" aria-selected={date === value}>
          <button type="button" ref={active ? activeDay : undefined} tabIndex={active ? 0 : -1}
            className={`btn-icon date-picker-day${day.getMonth() !== cursor.getMonth() ? " date-picker-outside-month" : ""}${date === value ? " date-picker-day-selected" : ""}`}
            aria-label={fullDate.format(day)} aria-current={date === today ? "date" : undefined}
            onClick={() => onSelect(date)}>{day.getDate()}</button>
        </td>;
      })}</tr>)}</tbody>
    </table>
    <button type="button" className="btn btn-sm date-picker-today" onClick={() => onSelect(today)}>Today</button>
  </div>;
}
