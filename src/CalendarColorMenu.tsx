import { Check, MoreHorizontal } from "lucide-react";
import { useEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
import {
  CALENDAR_COLORS,
  type CalendarColorId,
  calendarColorId,
  calendarColorStyle,
  setCalendarColor,
  useCalendarColors,
} from "./calendarColors";
import { queuePortablePreferences } from "./syncedPreferences";
import { useEscapeDismiss } from "./useEscapeDismiss";
import { ICON_SIZE } from "./iconSizes";

/**
 * One calendar row with a hover-revealed options button that opens the color
 * palette. The row carries the calendar's color so its checkbox matches.
 */
export function CalendarColorRow({
  accountId,
  calendarId,
  calendarName,
  children,
}: {
  accountId: string;
  calendarId: string;
  calendarName: string;
  children: ReactNode;
}) {
  const colors = useCalendarColors();
  const [open, setOpen] = useState(false);
  const rowRef = useRef<HTMLDivElement>(null);
  const selected = calendarColorId(colors, accountId, calendarId);

  useEffect(() => {
    if (!open) return;
    const handlePointerDown = (event: MouseEvent) => {
      if (rowRef.current?.contains(event.target as Node)) return;
      setOpen(false);
    };
    document.addEventListener("mousedown", handlePointerDown);
    return () => document.removeEventListener("mousedown", handlePointerDown);
  }, [open]);

  return (
    <div className="calendar-color-row" ref={rowRef} style={calendarColorStyle(colors, accountId, calendarId)}>
      {children}
      <button
        type="button"
        className="btn-icon btn-icon-sm calendar-color-trigger"
        aria-label={`Options for ${calendarName}`}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((current) => !current)}
      >
        <MoreHorizontal size={ICON_SIZE.sm} />
      </button>
      {open ? (
        <CalendarColorPalette
          calendarName={calendarName}
          selected={selected}
          onSelect={(colorId) => {
            setCalendarColor(accountId, calendarId, colorId);
            queuePortablePreferences();
            setOpen(false);
          }}
          onClose={() => setOpen(false)}
        />
      ) : null}
    </div>
  );
}

function CalendarColorPalette({
  calendarName,
  selected,
  onSelect,
  onClose,
}: {
  calendarName: string;
  selected: CalendarColorId | undefined;
  /** Null returns the calendar to the default accent color. */
  onSelect(colorId: CalendarColorId | null): void;
  onClose(): void;
}) {
  useEscapeDismiss(onClose);
  return (
    <div className="calendar-color-menu" role="menu" aria-label={`Color for ${calendarName}`}>
      <button
        type="button"
        role="menuitemradio"
        aria-checked={selected === undefined}
        className="calendar-color-default"
        onClick={() => onSelect(null)}
      >
        <span className="calendar-color-default-swatch" aria-hidden="true">
          {selected === undefined ? <Check size={ICON_SIZE.xs} /> : null}
        </span>
        Default
      </button>
      {CALENDAR_COLORS.map((color) => (
        <button
          key={color.id}
          type="button"
          role="menuitemradio"
          aria-checked={selected === color.id}
          aria-label={color.label}
          title={color.label}
          className="calendar-color-swatch"
          style={{ "--swatch-color": color.value } as CSSProperties}
          onClick={() => onSelect(color.id)}
        >
          {selected === color.id ? <Check size={ICON_SIZE.xs} aria-hidden="true" /> : null}
        </button>
      ))}
    </div>
  );
}
