import type { ContactGroup, ContactGroupRecipients, ContactProfile } from "./domain";
import { parseAddress } from "./emailAddress";

/** Mirrors `MAX_GROUP_NAME` in `src-tauri/src/db/contact_groups.rs`. */
export const MAX_CONTACT_GROUP_NAME = 100;
/** Mirrors `MAX_CONTACT_GROUP_MEMBERS` in `crates/sync-protocol`. */
export const MAX_CONTACT_GROUP_MEMBERS = 500;
/** Mirrors `MAX_CONTACT_GROUPS` in `src-tauri/src/db/contact_groups.rs`. */
export const MAX_CONTACT_GROUPS = 200;
/**
 * More group members than this in To and Cc, where everyone sees everyone's
 * address, prompts a suggestion to use Bcc.
 */
export const GROUP_VISIBLE_RECIPIENT_LIMIT = 10;
/** Most groups offered among one field's suggestions. */
export const MAX_GROUP_SUGGESTIONS = 3;

export function memberCountLabel(count: number): string {
  return `${count} ${count === 1 ? "member" : "members"}`;
}

/** Groups that list `contactId`, in their listed (name) order. */
export function groupsForContact(groups: readonly ContactGroup[], contactId: string): ContactGroup[] {
  return groups.filter((group) => group.memberIds.includes(contactId));
}

export function filterGroups(groups: readonly ContactGroup[], needle: string): ContactGroup[] {
  const query = needle.trim().toLocaleLowerCase();
  return query ? groups.filter((group) => group.name.toLocaleLowerCase().includes(query)) : [...groups];
}

/** The group's member profiles in the group's order; members not in `profiles` are skipped. */
export function groupMembers(group: ContactGroup, profiles: readonly ContactProfile[]): ContactProfile[] {
  const byId = new Map(profiles.map((profile) => [profile.id, profile]));
  return group.memberIds.flatMap((id) => byId.get(id) ?? []);
}

/**
 * Contacts that could join the group: everyone not already in it, skipping
 * a mail-derived entry whose address a member already owns.
 */
export function groupCandidates(group: ContactGroup, profiles: readonly ContactProfile[]): ContactProfile[] {
  const members = new Set(group.memberIds);
  const memberAddresses = new Set(profiles.filter((profile) => members.has(profile.id)).flatMap((profile) => profile.addresses));
  return profiles.filter((profile) => !members.has(profile.id) && !profile.addresses.some((address) => memberAddresses.has(address)));
}

/** A typed recipient such as `Ada <ada@example.com>` reduced to its address, or null when it isn't one. */
export function typedAddress(value: string): string | null {
  const parsed = parseAddress(value.trim());
  const email = parsed.email.trim().toLocaleLowerCase();
  return /^[^\s@]+@[^\s@]+$/.test(email) ? email : null;
}

/** Groups whose name contains `token`, skipping empty ones, best (prefix) matches first. */
export function matchingGroups(groups: readonly ContactGroupRecipients[], token: string): ContactGroupRecipients[] {
  const needle = token.trim().toLocaleLowerCase();
  if (!needle) return [];
  return groups
    .filter((group) => group.members.length > 0 && group.name.toLocaleLowerCase().includes(needle))
    .sort((a, b) => Number(!a.name.toLocaleLowerCase().startsWith(needle)) - Number(!b.name.toLocaleLowerCase().startsWith(needle)))
    .slice(0, MAX_GROUP_SUGGESTIONS);
}

/**
 * Groups with more than the limit of their members among `visibleEmails`
 * (To and Cc). A member counts once, recognized by any of their addresses;
 * `emails` holds the visible addresses to move. Largest first.
 */
export function overexposedGroups(groups: readonly ContactGroupRecipients[], visibleEmails: readonly string[]): { name: string; count: number; emails: string[] }[] {
  const visible = new Set(visibleEmails.map((email) => email.toLocaleLowerCase()));
  const shown = (address: string) => visible.has(address.toLocaleLowerCase());
  return groups
    .map((group) => {
      const members = group.members.filter((member) => member.addresses.some(shown));
      return { name: group.name, count: members.length, emails: members.flatMap((member) => member.addresses.filter(shown).map((address) => address.toLocaleLowerCase())) };
    })
    .filter((group) => group.count > GROUP_VISIBLE_RECIPIENT_LIMIT)
    .sort((a, b) => b.count - a.count);
}
