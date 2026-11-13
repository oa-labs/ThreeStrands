import { useEffect, useMemo, useRef, useState } from "react";
import { invitationDay, isCalendarAttachment, isCancelledInvitation, MAX_PREVIEWABLE_CALENDAR_SIZE } from "./calendarInvitations";
import { mailClient } from "./data/client";
import type { CalendarEventPreview, Message } from "./domain";

/** Parsed invitations kept so moving back to a conversation does not parse it again. */
const MAX_CACHED_INVITATIONS = 50;
const cache = new Map<string, CalendarEventPreview | null>();

/** Forgets parsed invitations, for tests. */
export function clearInvitationEventCache() {
  cache.clear();
}

async function loadInvitation(message: Message): Promise<CalendarEventPreview | null> {
  for (const attachment of message.attachments) {
    if (!isCalendarAttachment(attachment) || attachment.size > MAX_PREVIEWABLE_CALENDAR_SIZE) continue;
    const key = `${message.id}:${attachment.id}:${attachment.size}`;
    let event = cache.get(key);
    if (event === undefined) {
      try {
        const preview = await mailClient.previewCalendarAttachment(message.id, attachment.id);
        event = preview.events[0] ?? null;
      } catch {
        // The card in the message reports the failure; here it only means no day to show.
        continue;
      }
      cache.set(key, event);
      if (cache.size > MAX_CACHED_INVITATIONS) cache.delete(cache.keys().next().value!);
    }
    if (event) return event;
  }
  return null;
}

/**
 * The event a conversation's invitation describes, and the local day it falls
 * on, for showing what else the user has that day. A cancelled invitation has
 * no day, since there is no decision left to make about it.
 */
export function useInvitationEvent(message: Message | null): { event: CalendarEventPreview; day: Date } | null {
  const [loaded, setLoaded] = useState<{ messageId: string; event: CalendarEventPreview | null } | null>(null);
  const attachmentsKey = message?.attachments.map((attachment) => `${attachment.id}:${attachment.size}`).join("|") ?? "";
  const messageId = message?.id ?? null;
  // The message object changes on every sync; its id and attachments say whether the invitation did.
  const messageRef = useRef(message);
  messageRef.current = message;
  useEffect(() => {
    const current = messageRef.current;
    if (!current) return;
    let active = true;
    void loadInvitation(current).then((event) => { if (active) setLoaded({ messageId: current.id, event }); });
    return () => { active = false; };
  }, [messageId, attachmentsKey]);
  const event = loaded && loaded.messageId === messageId ? loaded.event : null;
  const dayTime = event ? invitationDay(event)?.getTime() ?? null : null;
  return useMemo(() => event && dayTime !== null && !isCancelledInvitation(event) ? { event, day: new Date(dayTime) } : null, [event, dayTime]);
}
