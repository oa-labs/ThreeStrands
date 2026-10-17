import { mailClient } from "./data/client";
import type { ContactProfile, ContactSuggestion } from "./domain";
import { MAX_SEARCH_SUGGESTIONS, type SuggestionContact } from "./searchSuggestions";

/**
 * Most addresses one contact suggestion expands to. Each address is one
 * search term, and a query has a term budget (16) shared with whatever else
 * the user typed.
 */
export const MAX_CONTACT_SEARCH_ADDRESSES = 6;

/**
 * Merges each account's ranked contact suggestions into one list for the
 * search box: pinned contacts first, then the people you exchange the most
 * mail with. A suggestion whose address belongs to an address-book contact
 * with several addresses carries all of them (the suggested one first), and
 * that contact appears once however many of its addresses were suggested.
 */
export function mergeSearchContacts(
  perAccount: readonly ContactSuggestion[][],
  profiles: readonly ContactProfile[] = [],
): SuggestionContact[] {
  const profileByAddress = new Map<string, ContactProfile>();
  for (const profile of profiles) {
    for (const address of profile.addresses) profileByAddress.set(address.toLowerCase(), profile);
  }
  const seen = new Set<string>();
  const merged: SuggestionContact[] = [];
  const ranked = perAccount
    .flat()
    .map((contact, order) => ({ contact, order }))
    .sort((a, b) =>
      Number(b.contact.pinned) - Number(a.contact.pinned)
      || (b.contact.sentCount + b.contact.receivedCount) - (a.contact.sentCount + a.contact.receivedCount)
      || a.order - b.order);
  for (const { contact } of ranked) {
    const profile = profileByAddress.get(contact.email.toLowerCase());
    const key = profile ? `profile:${profile.id}` : contact.email.toLowerCase();
    if (seen.has(key) || seen.has(contact.email.toLowerCase())) continue;
    seen.add(key);
    seen.add(contact.email.toLowerCase());
    const others = (profile?.addresses ?? []).filter((address) => address.toLowerCase() !== contact.email.toLowerCase());
    for (const other of others) seen.add(other.toLowerCase());
    merged.push({
      email: contact.email,
      name: profile?.displayName || contact.displayName,
      addresses: [contact.email, ...others].slice(0, MAX_CONTACT_SEARCH_ADDRESSES),
    });
    if (merged.length === MAX_SEARCH_SUGGESTIONS) break;
  }
  return merged;
}

/**
 * Contact suggestions for the search box across the searched accounts. An
 * account that fails to answer is left out rather than failing the lookup;
 * without address-book profiles each person is matched by one address.
 */
export async function loadSearchContacts(accountIds: readonly string[], query: string): Promise<SuggestionContact[]> {
  const [suggestions, profiles] = await Promise.all([
    Promise.allSettled(
      accountIds.map((accountId) => mailClient.listContactSuggestions(accountId, query, MAX_SEARCH_SUGGESTIONS)),
    ),
    mailClient.listContactProfiles(query, MAX_SEARCH_SUGGESTIONS * 2).catch(() => [] as ContactProfile[]),
  ]);
  return mergeSearchContacts(
    suggestions.flatMap((result) => (result.status === "fulfilled" ? [result.value] : [])),
    profiles,
  );
}
