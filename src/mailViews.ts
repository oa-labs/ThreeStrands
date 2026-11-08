import type { SplitInbox } from "./domain";
import type { SavedSearch } from "./savedSearches";

export const SAVED_VIEW_PREFIX = "saved-search:";
export const MAIL_VIEW_ORDER_KEY = "threestrands.mailViewOrder";
export type SavedSearchView = { id: string; name: string; accountId: string | null; savedSearch: SavedSearch };
export type MailView = SplitInbox | SavedSearchView;
export function isSavedSearchView(view: MailView): view is SavedSearchView { return "savedSearch" in view; }
export function savedSearchView(search: SavedSearch): SavedSearchView {
  return { id: SAVED_VIEW_PREFIX + search.id, name: search.name, accountId: search.accountId, savedSearch: search };
}
export function orderedMailViews(splits: SplitInbox[], searches: SavedSearch[], displayOrder?: string[] | null): MailView[] {
  const views: MailView[] = [...splits, ...searches.map(savedSearchView)];
  try {
    const order: unknown = displayOrder ?? JSON.parse(localStorage.getItem(MAIL_VIEW_ORDER_KEY) ?? "[]");
    if (Array.isArray(order) && order.every((id) => typeof id === "string")) {
      const rank = new Map(order.map((id, index) => [id, index]));
      views.sort((a, b) => (rank.get(a.id) ?? order.length) - (rank.get(b.id) ?? order.length));
    }
  } catch { /* A lost display order must never hide a view. */ }
  let routingIndex = 0;
  return views.map((view) => isSavedSearchView(view) ? view : splits[routingIndex++]);
}
