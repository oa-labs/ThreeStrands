/**
 * The "try it" inbox. Shortcuts only act while the demo is on screen, are
 * ignored in editable fields, and never call preventDefault — a visitor's
 * browser shortcuts always win.
 */
type DemoKey = "j" | "k" | "e" | "s" | "mod+k";

const LABELS: Record<DemoKey, string> = {
  j: "Next conversation",
  k: "Previous conversation",
  e: "Archive",
  s: "Star",
  "mod+k": "Command palette",
};

function isEditable(target: EventTarget | null) {
  if (!(target instanceof HTMLElement)) return false;
  return target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName);
}

export function initKeyboardDemo(isMac: boolean) {
  const demo = document.querySelector<HTMLElement>("[data-demo]");
  const list = demo?.querySelector<HTMLOListElement>("[data-demo-list]");
  const caption = demo?.querySelector<HTMLElement>("[data-demo-caption]");
  const palette = demo?.querySelector<HTMLElement>("[data-demo-palette]");
  if (!demo || !list || !caption || !palette) return;

  const original = [...list.children].map((row) => row.cloneNode(true) as HTMLElement);
  const modLabel = isMac ? "⌘K" : "Ctrl K";
  let selected = 0;
  let busy = false;

  const rows = () => [...list.querySelectorAll<HTMLElement>("li:not(.is-leaving)")];
  const subject = (row: HTMLElement | undefined) => row?.dataset.subject ?? "";

  const say = (key: string, text: string) => {
    const kbd = document.createElement("kbd");
    kbd.textContent = key;
    caption.replaceChildren(kbd, ` ${text}`);
  };

  const select = (index: number) => {
    const current = rows();
    selected = Math.max(0, Math.min(index, current.length - 1));
    current.forEach((row, position) => row.classList.toggle("is-selected", position === selected));
  };

  const press = (key: DemoKey) => {
    const cap = demo.querySelector<HTMLElement>(`[data-key="${key}"]`);
    cap?.classList.add("is-pressed");
    window.setTimeout(() => cap?.classList.remove("is-pressed"), 140);
  };

  const refill = () => {
    list.replaceChildren(...original.map((row) => row.cloneNode(true)));
    select(0);
  };

  const perform = (key: DemoKey) => {
    if (busy) return;
    press(key);
    if (key !== "mod+k" && !palette.hidden) palette.hidden = true;
    const current = rows();
    switch (key) {
      case "j":
      case "k": {
        const next = selected + (key === "j" ? 1 : -1);
        if (next < 0 || next >= current.length) {
          say(key, key === "j" ? "That's the bottom of the inbox — try k." : "Already at the top — try j.");
          return;
        }
        select(next);
        say(key, `${LABELS[key]}: “${subject(current[next])}”`);
        return;
      }
      case "s": {
        const row = current[selected];
        if (!row) return;
        const starred = row.classList.toggle("is-starred");
        say(key, `${starred ? "Starred" : "Unstarred"} “${subject(row)}”`);
        return;
      }
      case "e": {
        const row = current[selected];
        if (!row) return;
        busy = true;
        row.classList.add("is-leaving");
        say(key, `Archived “${subject(row)}”`);
        window.setTimeout(() => {
          row.remove();
          busy = false;
          if (rows().length === 0) {
            say(key, "Inbox zero. Nicely done — refilling the demo.");
            window.setTimeout(refill, 1400);
            return;
          }
          select(Math.min(selected, rows().length - 1));
        }, 260);
        return;
      }
      case "mod+k": {
        palette.hidden = !palette.hidden;
        say(modLabel, palette.hidden ? "Command palette closed" : "Command palette — every action, searchable");
        return;
      }
    }
  };

  select(0);
  for (const button of demo.querySelectorAll<HTMLButtonElement>("button[data-key]")) {
    button.addEventListener("click", () => perform(button.dataset.key as DemoKey));
  }

  const onScreen = () => {
    const rect = demo.getBoundingClientRect();
    return rect.bottom > window.innerHeight * 0.15 && rect.top < window.innerHeight * 0.85;
  };

  document.addEventListener("keydown", (event) => {
    if (!onScreen() || event.defaultPrevented || event.altKey || isEditable(event.target)) return;
    const key = event.key.toLowerCase();
    if ((event.metaKey || event.ctrlKey) && key === "k") {
      perform("mod+k");
      return;
    }
    if (event.metaKey || event.ctrlKey) return;
    if (key === "escape" && !palette.hidden) {
      palette.hidden = true;
      say("esc", "Command palette closed");
      return;
    }
    if (key === "j" || key === "k" || key === "e" || key === "s") perform(key);
  });
}
