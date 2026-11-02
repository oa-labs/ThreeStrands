import { useCallback, useEffect, useState, type SetStateAction } from "react";
import type { ContactsView } from "./contactsView";

function readSelection(key: string): string | null {
  try {
    return localStorage.getItem(key) || null;
  } catch {
    return null;
  }
}

/** Navigation state stays on this device, separate for each account and tab. */
export function useContactsSelection(view: ContactsView, accountId: string | null) {
  const key = `threestrands.contacts.selection.${JSON.stringify([accountId, view])}`;
  const [selection, setSelection] = useState(() => ({ key, id: readSelection(key) }));
  if (selection.key !== key) setSelection({ key, id: readSelection(key) });

  useEffect(() => {
    try {
      if (selection.id) localStorage.setItem(selection.key, selection.id);
      else localStorage.removeItem(selection.key);
    } catch {
      // Navigation still works when storage is unavailable.
    }
  }, [selection]);

  const setId = useCallback((next: SetStateAction<string | null>) => {
    setSelection(current => {
      const previous = current.key === key ? current.id : readSelection(key);
      const id = typeof next === "function" ? next(previous) : next;
      return current.key === key && id === current.id ? current : { key, id };
    });
  }, [key]);
  return [selection.id, setId] as const;
}
