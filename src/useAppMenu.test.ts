import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const created = vi.hoisted(() => ({
  items: new Map<string, { options: { action?: () => void; enabled?: boolean }; setEnabled: ReturnType<typeof vi.fn> }>(),
  setAsAppMenu: vi.fn(() => Promise.resolve(null)),
  setAsWindows: vi.fn(() => Promise.resolve()),
  setAsHelp: vi.fn(() => Promise.resolve()),
  submenus: [] as string[],
}));

vi.mock("@tauri-apps/api/menu", () => ({
  Menu: { new: vi.fn(async () => ({ setAsAppMenu: created.setAsAppMenu })) },
  Submenu: {
    new: vi.fn(async (options: { text: string }) => {
      created.submenus.push(options.text);
      return { setAsWindowsMenuForNSApp: created.setAsWindows, setAsHelpMenuForNSApp: created.setAsHelp };
    }),
  },
  PredefinedMenuItem: { new: vi.fn(async () => ({})) },
  MenuItem: {
    new: vi.fn(async (options: { id: string; action?: () => void; enabled?: boolean }) => {
      const item = { options, setEnabled: vi.fn(() => Promise.resolve()) };
      created.items.set(options.id, item);
      return item;
    }),
  },
}));

import { commands } from "./commands";
import { commandContext } from "./test/commandContext";
import { useAppMenu } from "./useAppMenu";

const setPlatform = (value: string) => Object.defineProperty(navigator, "platform", { value, configurable: true });
const original = navigator.platform;

beforeEach(() => {
  created.items.clear();
  created.submenus.length = 0;
  vi.clearAllMocks();
  setPlatform("MacIntel");
});
afterEach(() => setPlatform(original));

describe("useAppMenu", () => {
  it("builds the menu bar on macOS and marks the Window and Help menus", async () => {
    renderHook(() => useAppMenu(commandContext(), vi.fn(), vi.fn()));
    await waitFor(() => expect(created.setAsHelp).toHaveBeenCalledTimes(1));
    expect(created.setAsAppMenu).toHaveBeenCalledTimes(1);
    expect(created.setAsWindows).toHaveBeenCalledTimes(1);
    expect(created.submenus).toEqual(["ThreeStrands", "File", "Edit", "View", "Mailbox", "Message", "Window", "Help"]);
  });

  it.each(["Win32", "Linux x86_64"])("leaves the default menu alone on %s", async (platform) => {
    setPlatform(platform);
    renderHook(() => useAppMenu(commandContext(), vi.fn(), vi.fn()));
    await act(async () => { await Promise.resolve(); });
    expect(created.setAsAppMenu).not.toHaveBeenCalled();
    expect(created.items.size).toBe(0);
  });

  it("runs a clicked command through the execute path with the registry command", async () => {
    const execute = vi.fn();
    renderHook(() => useAppMenu(commandContext(), execute, vi.fn()));
    await waitFor(() => expect(created.setAsHelp).toHaveBeenCalled());
    created.items.get("draft.new")!.options.action!();
    expect(execute).toHaveBeenCalledWith(commands.find((command) => command.id === "draft.new"));
  });

  it("does not run a command that is disabled or while a dialog is open", async () => {
    const execute = vi.fn();
    const { rerender } = renderHook(({ ctx }) => useAppMenu(ctx, execute, vi.fn()), { initialProps: { ctx: commandContext() } });
    await waitFor(() => expect(created.setAsHelp).toHaveBeenCalled());
    created.items.get("draft.reply")!.options.action!();
    rerender({ ctx: commandContext({ interactionScope: "modal" }) });
    created.items.get("draft.new")!.options.action!();
    expect(execute).not.toHaveBeenCalled();
  });

  it("routes Check for Updates to its own handler", async () => {
    const execute = vi.fn();
    const check = vi.fn();
    renderHook(() => useAppMenu(commandContext(), execute, check));
    await waitFor(() => expect(created.setAsHelp).toHaveBeenCalled());
    created.items.get("app.checkForUpdates")!.options.action!();
    expect(check).toHaveBeenCalledTimes(1);
    expect(execute).not.toHaveBeenCalled();
  });

  it("updates only the items whose enabled state changed", async () => {
    const { rerender } = renderHook(({ ctx }) => useAppMenu(ctx, vi.fn(), vi.fn()), { initialProps: { ctx: commandContext({ selectedId: null }) } });
    await waitFor(() => expect(created.setAsHelp).toHaveBeenCalled());
    for (const item of created.items.values()) item.setEnabled.mockClear();
    rerender({ ctx: commandContext({ selectedId: null }) });
    expect([...created.items.values()].every((item) => item.setEnabled.mock.calls.length === 0)).toBe(true);
    rerender({ ctx: commandContext({ selectedId: "t1" }) });
    const touched = [...created.items].filter(([, item]) => item.setEnabled.mock.calls.length > 0);
    expect(touched.map(([id]) => id)).toContain("draft.reply");
    expect(created.items.get("draft.reply")!.setEnabled).toHaveBeenCalledWith(true);
    expect(created.items.get("draft.new")!.setEnabled).not.toHaveBeenCalled();
    expect(created.items.get("settings.open")!.setEnabled).not.toHaveBeenCalled();
  });
});
