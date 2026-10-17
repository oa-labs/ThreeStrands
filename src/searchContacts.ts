import { mailClient } from "./data/client";
import type { ContactSuggestion } from "./domain";
import { MAX_SEARCH_SUGGESTIONS, type SuggestionContact } from "./searchSuggestions";

/**
 * Merges each account's ranked contact suggestions into one list for the
 * search box: pinned contacts first, then the people you exchange the most
 * mail with, one entry per address. An account that fails to answer is left
 * out rather than failing the whole lookup.
 */
export function mergeSearchContacts(perAccount: readonly ContactSuggestion[][]): SuggestionContact[] {
  const seen = new Set<string>();
  return perAccount
    .flat()
    .map((contact, order) => ({ contact, order }))
    .sort((a, b) =>
      Number(b.contact.pinned) - Number(a.contact.pinned)
      || (b.contact.sentCount + b.contact.receivedCount) - (a.contact.sentCount + a.contact.receivedCount)
      || a.order - b.order)
    .filter(({ contact }) => {
      const key = contact.email.toLowerCase();
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    })
    .slice(0, MAX_SEARCH_SUGGESTIONS)
    .map(({ contact }) => ({ email: contact.email, name: contact.displayName }));
}

export async function loadSearchContacts(accountIds: readonly string[], query: string): Promise<SuggestionContact[]> {
  const results = await Promise.allSettled(
    accountIds.map((accountId) => mailClient.listContactSuggestions(accountId, query, MAX_SEARCH_SUGGESTIONS)),
  );
  return mergeSearchContacts(results.flatMap((result) => (result.status === "fulfilled" ? [result.value] : [])));
}
