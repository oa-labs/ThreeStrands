import { CalendarDays, ChevronLeft, ChevronRight } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
import { dateInputValue } from "./calendarTime";
import { MonthGrid, shiftMonth } from "./MonthGrid";
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

function CalendarDatePicker({ id, label, value, onSelect, onClose }: {
  id: string;
  label: string;
  value: string;
  onSelect(value: string): void;
  onClose(): void;
}) {
  const [cursor, setCursor] = useState(() => parseCalendarDate(value) ?? new Date());
  useEscapeDismiss(onClose);
  const today = dateInputValue(new Date());
  const title = new Intl.DateTimeFormat(undefined, { month: "long", year: "numeric" }).format(cursor);
  return <div id={id} className="date-picker-panel" role="dialog" aria-label={`Choose ${label.toLowerCase()}`}>
    <div className="date-picker-heading">
      <button type="button" className="btn-icon btn-icon-sm" aria-label="Previous Month" onClick={() => setCursor(shiftMonth(cursor, -1))}><ChevronLeft size={ICON_SIZE.sm} /></button>
      <span id={`${id}-month`} aria-live="polite">{title}</span>
      <button type="button" className="btn-icon btn-icon-sm" aria-label="Next Month" onClick={() => setCursor(shiftMonth(cursor, 1))}><ChevronRight size={ICON_SIZE.sm} /></button>
    </div>
    <MonthGrid labelledBy={`${id}-month`} cursor={cursor} onCursorChange={setCursor} selected={value} onSelect={onSelect} focusCursor="always" />
    <button type="button" className="btn btn-sm date-picker-today" onClick={() => onSelect(today)}>Today</button>
  </div>;
}
