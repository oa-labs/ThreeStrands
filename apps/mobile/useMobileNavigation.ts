import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { todayTarget, type NoteTarget } from "../../src/noteDrafts";

export type Page =
  | { screen: "daily"; date: string; edit?: boolean }
  | { screen: "note"; target: NoteTarget; marker?: string }
  | { screen: "notes" | "tasks" | "trash" | "settings" | "newNote" | "newTask" }
  | { screen: "task"; id: string };

export type Workspace = "daily" | "notes" | "tasks";
type Entry = { page: Page; id: number; workspace?: Workspace; parent?: number };

function initialEntry(): Entry {
  return { page: { screen: "daily", date: todayTarget().date }, id: 0, workspace: "daily" };
}

function isWorkspace(screen: Page["screen"]): screen is Workspace {
  return screen === "daily" || screen === "notes" || screen === "tasks";
}

export function useMobileNavigation() {
  const [entry, setEntry] = useState(initialEntry);
  const active = useRef(entry);
  const nextId = useRef(1);
  const positions = useRef(new Map<number, number>());
  const entries = useRef(new Map([[entry.id, entry]]));
  const workspaces = useRef<Partial<Record<Workspace, Entry>>>({ daily: entry });
  const scroller = useRef<HTMLElement>(null);
  const content = useRef<HTMLDivElement>(null);

  useEffect(() => {
    window.history.replaceState(active.current, "");
    const previousRestoration = window.history.scrollRestoration;
    window.history.scrollRestoration = "manual";
    function pop(event: PopStateEvent) {
      const next: Entry = event.state ?? initialEntry();
      active.current = next;
      if (next.workspace) workspaces.current[next.workspace] = next;
      setEntry(next);
    }
    window.addEventListener("popstate", pop);
    return () => {
      window.removeEventListener("popstate", pop);
      window.history.scrollRestoration = previousRestoration;
    };
  }, []);

  const { id, page } = entry;
  useLayoutEffect(() => {
    const panel = scroller.current;
    const inner = content.current;
    if (!panel || !inner) return;
    const destination = positions.current.get(id) ?? 0;
    panel.scrollTop = destination;
    if (panel.scrollTop >= destination || typeof ResizeObserver === "undefined") return;

    // A note or list can still be loading when Back restores its history entry.
    const observer = new ResizeObserver(() => {
      panel.scrollTop = destination;
      if (panel.scrollTop >= destination) stop();
    });
    function stop() {
      observer.disconnect();
      panel?.removeEventListener("pointerdown", stop);
      panel?.removeEventListener("wheel", stop);
      panel?.removeEventListener("keydown", stop);
    }
    observer.observe(inner);
    panel.addEventListener("pointerdown", stop);
    panel.addEventListener("wheel", stop);
    panel.addEventListener("keydown", stop);
    return stop;
  }, [id]);

  function rememberScroll() {
    positions.current.set(active.current.id, scroller.current?.scrollTop ?? 0);
  }

  function navigate(page: Page, { replace = false, scroll = 0 } = {}) {
    rememberScroll();
    const previous = active.current;
    const workspace = isWorkspace(page.screen) ? page.screen
      : page.screen === "settings" ? undefined : previous.workspace;
    const next = { page, id: nextId.current++, workspace, parent: replace ? previous.parent : previous.id };
    positions.current.set(next.id, scroll);
    entries.current.set(next.id, next);
    if (workspace) workspaces.current[workspace] = next;
    if (replace) window.history.replaceState(next, "");
    else window.history.pushState(next, "");
    active.current = next;
    setEntry(next);
  }

  function switchWorkspace(screen: Workspace) {
    if (screen === entry.workspace) return;
    const saved = workspaces.current[screen];
    if (!saved) {
      navigate(screen === "daily" ? initialEntry().page : { screen });
      return;
    }

    // Restore the tab's detail path so both the toolbar and Android Back
    // return through the same screens and scroll positions as before.
    const trail = [saved];
    while (trail[0].page.screen !== screen && trail[0].parent !== undefined) {
      const parent = entries.current.get(trail[0].parent);
      if (!parent || parent.workspace !== screen) break;
      trail.unshift(parent);
    }
    rememberScroll();
    for (const item of trail) window.history.pushState(item, "");
    active.current = saved;
    setEntry(saved);
  }

  return { page, workspace: entry.workspace, entryId: id, scroller, content, rememberScroll, navigate, switchWorkspace,
    back: () => window.history.back() };
}
