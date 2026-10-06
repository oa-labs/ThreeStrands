export type ContactsView = "all" | "keepInTouch";

export const CONTACTS_VIEWS: readonly ContactsView[] = ["all", "keepInTouch"];
const CONTACTS_VIEW_KEY = "threestrands.contacts.view";

export function readContactsView(): ContactsView {
  try {
    const stored = localStorage.getItem(CONTACTS_VIEW_KEY);
    return CONTACTS_VIEWS.find((name) => name === stored) ?? "all";
  } catch {
    return "all";
  }
}

export function writeContactsView(view: ContactsView) {
  try {
    localStorage.setItem(CONTACTS_VIEW_KEY, view);
  } catch {
    // The chosen view still applies for this session.
  }
}

export function adjacentContactsView(current: ContactsView, direction: -1 | 1): ContactsView {
  const index = Math.max(0, CONTACTS_VIEWS.indexOf(current));
  return CONTACTS_VIEWS[(index + direction + CONTACTS_VIEWS.length) % CONTACTS_VIEWS.length];
}
