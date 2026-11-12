import { TimePickerField } from "./TimePickerField";
import { DatePickerField } from "./DatePickerField";

/** Keep the date's calendar popup separate from the directly editable time. */
export function EventDateTimeField({ label, allDay, value, onChange }: {
  label: "Starts" | "Ends";
  allDay: boolean;
  value: string;
  onChange(value: string): void;
}) {
  const prefix = label === "Starts" ? "Start" : "End";
  const [date, time = ""] = value.split("T");
  return <fieldset className={`event-date-time-field${allDay ? " event-date-time-all-day" : ""}`}>
    <legend>{label}</legend>
    <div className="event-date-time-inputs">
      <DatePickerField label={`${prefix} date`} value={date} onChange={(next) => onChange(allDay ? next : `${next}T${time}`)} />
      {!allDay ? <TimePickerField label={`${prefix} time`} value={time} onChange={(next) => onChange(`${date}T${next}`)} /> : null}
    </div>
  </fieldset>;
}
