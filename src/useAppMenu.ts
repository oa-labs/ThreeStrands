import { useEffect, useRef } from "react";
import { Menu, MenuItem, PredefinedMenuItem, Submenu } from "@tauri-apps/api/menu";
import type { Command, CommandContext } from "./commands";
import { commands } from "./commands";
import {
  CHECK_FOR_UPDATES_ID, MENU_LAYOUT, diffEnabled, menuEnabledStates, menuItemEnabled, resolveCommandItem,
} from "./appMenu";
import { logBackgroundFailure } from "./errors";
import { currentShortcutPlatform } from "./shortcutLabels";

const reportMenuFailure = logBackgroundFailure("Updating the menu bar");

/**
 * Builds the macOS menu bar once, then keeps each item's enabled state in step
 * with the command registry. A click runs the command through `execute`, the
 * same path as its keyboard shortcut. Other platforms keep their default menu.
 */
export function useAppMenu(context: CommandContext, execute: (command: Command) => void, checkForUpdates: () => void) {
  const contextRef = useRef(context);
  contextRef.current = context;
  const executeRef = useRef(execute);
  executeRef.current = execute;
  const checkRef = useRef(checkForUpdates);
  checkRef.current = checkForUpdates;
  const items = useRef<Map<string, MenuItem> | null>(null);
  const applied = useRef<Map<string, boolean>>(new Map());

  const syncEnabled = () => {
    const built = items.current;
    if (!built) return;
    const next = menuEnabledStates(contextRef.current);
    for (const [id, enabled] of diffEnabled(applied.current, next)) {
      applied.current.set(id, enabled);
      built.get(id)?.setEnabled(enabled).catch(reportMenuFailure);
    }
  };

  useEffect(() => {
    if (currentShortcutPlatform() !== "mac") return;
    let cancelled = false;
    const build = async () => {
      const built = new Map<string, MenuItem>();
      const initial = new Map<string, boolean>();
      const submenus: Submenu[] = [];
      for (const section of MENU_LAYOUT) {
        const children = [];
        for (const entry of section.entries) {
          if (entry.kind === "separator") children.push(await PredefinedMenuItem.new({ item: "Separator" }));
          else if (entry.kind === "predefined") {
            children.push(await PredefinedMenuItem.new({ item: entry.item === "About" ? { About: null } : entry.item, text: entry.title }));
          } else {
            const resolved = resolveCommandItem(entry);
            const enabled = menuItemEnabled(resolved.id, contextRef.current);
            initial.set(resolved.id, enabled);
            const item = await MenuItem.new({
              id: resolved.id,
              text: resolved.title,
              accelerator: resolved.accelerator ?? undefined,
              enabled,
              action: () => {
                const current = contextRef.current;
                if (!menuItemEnabled(resolved.id, current)) return;
                if (resolved.id === CHECK_FOR_UPDATES_ID) { checkRef.current(); return; }
                const found = commands.find((candidate) => candidate.id === resolved.id);
                if (found) executeRef.current(found);
              },
            });
            built.set(resolved.id, item);
            children.push(item);
          }
        }
        submenus.push(await Submenu.new({ text: section.title, items: children }));
      }
      const menu = await Menu.new({ items: submenus });
      if (cancelled) return;
      await menu.setAsAppMenu();
      const windowIndex = MENU_LAYOUT.findIndex((section) => section.role === "window");
      const helpIndex = MENU_LAYOUT.findIndex((section) => section.role === "help");
      if (windowIndex >= 0) await submenus[windowIndex].setAsWindowsMenuForNSApp();
      if (helpIndex >= 0) await submenus[helpIndex].setAsHelpMenuForNSApp();
      applied.current = initial;
      items.current = built;
      syncEnabled();
    };
    build().catch(reportMenuFailure);
    return () => { cancelled = true; };
  }, []);

  useEffect(syncEnabled, [context]);
}
