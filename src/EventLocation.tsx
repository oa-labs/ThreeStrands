import { openUrl } from "@tauri-apps/plugin-opener";
import { safeWebUrl } from "./calendarTime";

const locationUrlPattern = /(https?:\/\/[^\s<>"]+)/;

// Placeholders and online-meeting names that a maps search would only guess at.
const nonPlaces = new Set([
  "tbd", "tba", "tbc", "n a", "na", "none", "online", "virtual", "remote", "video", "phone", "call",
  "conference call", "zoom", "teams", "microsoft teams", "google meet", "meet", "hangouts", "google hangouts",
  "webex", "cisco webex", "skype", "slack", "slack huddle", "facetime", "discord", "gotomeeting", "whereby",
]);

/** A Google Maps search for a plain-text place, or null for links, placeholders, and online meetings. */
export function mapsUrl(location: string): string | null {
  const place = location.trim();
  // Anything carrying a URL scheme, or without letters (a phone number), is not a street place.
  if (!/\p{L}/u.test(place) || /^\/\/|\b[a-z][a-z0-9+.-]*:\/\/|\b(?:javascript|data|file|mailto|tel|sip):/i.test(place)) return null;
  const name = place.toLowerCase().replace(/[^\p{L}\p{N}]+/gu, " ").trim().replace(/ (?:meeting|call)$/, "");
  if (nonPlaces.has(name)) return null;
  return `https://www.google.com/maps/search/?api=1&query=${encodeURIComponent(place)}`;
}

/** Renders a location with its http(s) URLs opened in the system browser, or a plain place opened in Google Maps. */
export function EventLocation({ location, linkLabel }: { location: string; linkLabel?: string }) {
  const parts = location.split(locationUrlPattern);
  const placeUrl = parts.length === 1 ? mapsUrl(location) : null;
  if (placeUrl) {
    return (
      <a className="calendar-event-location-link" href={placeUrl} onClick={(clickEvent) => {
        clickEvent.preventDefault();
        void openUrl(placeUrl);
      }} title="Open in Google Maps">{location}</a>
    );
  }
  return parts.map((part, index) => {
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
