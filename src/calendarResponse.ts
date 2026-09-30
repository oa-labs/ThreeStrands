import type { ScheduleEvent } from "./domain";

export function responseLabel(event: ScheduleEvent): string | null {
  switch (event.responseStatus) {
    case "accepted": return "Going";
    case "declined": return "Not going";
    case "tentative": return "Maybe";
    case "needsAction": return "Awaiting response";
    default: return event.canRespond ? "Awaiting response" : null;
  }
}
