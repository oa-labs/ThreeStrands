import { useId, useLayoutEffect, useRef, useState } from "react";
import { useEscapeDismiss } from "./useEscapeDismiss";

/** Accept exact 24-hour times and familiar inputs such as 8am or 8:50 pm. */
export function parseEventTime(text: string): string | null {
  const match = /^(\d{1,2})(?::(\d{2}))?\s*(am|pm)?$/i.exec(text.trim());
  if (!match) return null;
  let hour = Number(match[1]);
  const minute = Number(match[2] ?? 0);
  const period = match[3]?.toLowerCase();
  if (minute > 59 || hour > (period ? 12 : 23) || (period && hour < 1)) return null;
  if (period) hour = hour % 12 + (period === "pm" ? 12 : 0);
  return `${String(hour).padStart(2, "0")}:${String(minute).padStart(2, "0")}`;
}

const times = Array.from({ length: 96 }, (_, index) => {
  const hour = Math.floor(index / 4);
  const minute = (index % 4) * 15;
  return { value: `${String(hour).padStart(2, "0")}:${String(minute).padStart(2, "0")}`,
    label: `${hour % 12 || 12}:${String(minute).padStart(2, "0")} ${hour < 12 ? "AM" : "PM"}` };
});

export function TimePickerField({ label, value, onChange }: { label: string; value: string; onChange(value: string): void }) {
  const id = useId();
  const input = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const [draft, setDraft] = useState<{ text: string; value: string } | null>(null);
  const text = draft?.value === value ? draft.text : value;
  useEscapeDismiss(() => setOpen(false), open);
  useLayoutEffect(() => {
    const [hour, minute] = (parseEventTime(text) ?? "09:00").split(":").map(Number);
    const nearest = Math.min(times.length - 1, Math.ceil((hour * 60 + minute) / 15));
    if (open) list.current?.children[active >= 0 ? active : nearest]?.scrollIntoView?.({ block: "nearest" });
  }, [open, active, text]);
  useLayoutEffect(() => {
    input.current?.setCustomValidity(text && !parseEventTime(text) ? "Enter a time such as 8:50 AM or 14:30." : "");
  }, [text]);
  function show() {
    const canonical = parseEventTime(text);
    setActive(canonical ? times.findIndex((time) => time.value === canonical) : 36);
    setOpen(true);
  }
  function choose(index: number) {
    if (input.current?.matches(":disabled")) return;
    onChange(times[index].value);
    setDraft(null);
    input.current?.setCustomValidity("");
    setOpen(false);
  }
  return <div className="time-picker-field">
    <input ref={input} aria-label={label} role="combobox" aria-autocomplete="list" aria-expanded={open}
      aria-controls={open ? id : undefined} aria-activedescendant={open && active >= 0 ? `${id}-${active}` : undefined}
      type="text" autoComplete="off" placeholder="HH:MM" required value={text}
      onFocus={(event) => { event.currentTarget.select(); show(); }} onClick={() => { if (!open) show(); }}
      onChange={(event) => {
        const next = event.target.value;
        const parsed = parseEventTime(next);
        setDraft({ text: next, value: parsed ?? "" });
        input.current?.setCustomValidity(next && !parsed ? "Enter a time such as 8:50 AM or 14:30." : "");
        onChange(parsed ?? "");
        setActive(parsed ? times.findIndex((time) => time.value === parsed) : -1);
        setOpen(true);
      }}
      onBlur={() => {
        const parsed = parseEventTime(text);
        if (parsed || !text) setDraft(null);
        setOpen(false);
      }}
      onKeyDown={(event) => {
        if (event.key === "ArrowDown" || event.key === "ArrowUp") {
          event.preventDefault();
          if (!open) show();
          else setActive((previous) => previous < 0
            ? Math.max(0, times.findIndex((time) => time.value >= (parseEventTime(text) ?? "09:00")))
            : (previous + (event.key === "ArrowDown" ? 1 : -1) + times.length) % times.length);
        } else if (event.key === "Enter" && open && !event.metaKey && !event.ctrlKey) {
          event.preventDefault();
          if (active >= 0) choose(active);
          else { setOpen(false); if (parseEventTime(text)) setDraft(null); }
        } else if (event.key === "Escape" && open) {
          event.preventDefault(); event.stopPropagation(); setOpen(false);
        }
      }} />
    {open ? <div ref={list} id={id} role="listbox" aria-label={`${label} suggestions`} className="time-picker-options">
      {times.map((time, index) => <div key={time.value} id={`${id}-${index}`} role="option" aria-selected={value === time.value}
        data-active={active === index} onMouseDown={(event) => { event.preventDefault(); choose(index); }}>
        {time.label}
      </div>)}
    </div> : null}
  </div>;
}
