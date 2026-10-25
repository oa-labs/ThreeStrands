import { openUrl } from "@tauri-apps/plugin-opener";
import { safeWebUrl } from "./calendarTime";

const locationUrlPattern = /(https?:\/\/[^\s<>"]+)/;

/** Renders a location with its http(s) URLs opened in the system browser. */
export function EventLocation({ location, linkLabel }: { location: string; linkLabel?: string }) {
  return location.split(locationUrlPattern).map((part, index) => {
    // Odd indexes are captured URLs; trailing punctuation stays plain text.
    const text = index % 2 === 1 ? part.replace(/[.,;:!?)\]]+$/, "") : part;
    const url = index % 2 === 1 ? safeWebUrl(text) : null;
    if (!url) return part;
    return (
      <span key={index}>
        <a className="calendar-event-location-link" href={url} onClick={(clickEvent) => {
          clickEvent.preventDefault();
          void openUrl(url);
        }} title={linkLabel ? text : undefined}>{linkLabel ?? text}</a>{part.slice(text.length)}
      </span>
    );
  });
}

