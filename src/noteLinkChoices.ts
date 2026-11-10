import { formatDateTime } from "./dateFormatting";
import type { NoteReference } from "./noteEntities";

/** The shared search result used by inline autocomplete and Link to…. */
export type NoteLinkChoice = { label: string; detail: string; sentAt?: string; reference: NoteReference };

export function noteLinkDetail(choice: NoteLinkChoice): string {
  return choice.sentAt ? `${choice.detail} · ${formatDateTime(new Date(choice.sentAt))}` : choice.detail;
}
