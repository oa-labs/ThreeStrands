import type { ContactGroup, ContactProfile } from "./domain";
import { parseAddress } from "./emailAddress";

/** Mirrors `MAX_GROUP_NAME` in `src-tauri/src/db/contact_groups.rs`. */
export const MAX_CONTACT_GROUP_NAME = 100;
/** Mirrors `MAX_CONTACT_GROUP_MEMBERS` in `crates/sync-protocol`. */
export const MAX_CONTACT_GROUP_MEMBERS = 500;
/** Mirrors `MAX_CONTACT_GROUPS` in `src-tauri/src/db/contact_groups.rs`. */
export const MAX_CONTACT_GROUPS = 200;

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
