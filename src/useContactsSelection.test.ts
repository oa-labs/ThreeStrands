import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useContactsSelection } from "./useContactsSelection";
import type { ContactsView } from "./contactsView";

describe("remembered Contacts selection", () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => { cleanup(); vi.restoreAllMocks(); localStorage.clear(); });

  it("keeps each tab and account independent, including all accounts", () => {
    const { result, rerender, unmount } = renderHook(({ view, accountId }: { view: ContactsView; accountId: string | null }) => useContactsSelection(view, accountId), { initialProps: { view: "all" as ContactsView, accountId: "work" as string | null } });
    act(() => result.current[1]("jane"));
    rerender({ view: "keepInTouch", accountId: "work" });
    expect(result.current[0]).toBeNull();
    act(() => result.current[1]("sam"));
    rerender({ view: "groups", accountId: "work" });
    act(() => result.current[1]("board"));
    rerender({ view: "all", accountId: null });
    expect(result.current[0]).toBeNull();
    act(() => result.current[1]("all-accounts"));
    rerender({ view: "all", accountId: "personal" });
    expect(result.current[0]).toBeNull();
    rerender({ view: "all", accountId: "work" });
    expect(result.current[0]).toBe("jane");
    rerender({ view: "keepInTouch", accountId: "work" });
    expect(result.current[0]).toBe("sam");
    unmount();
    const restored = renderHook(() => useContactsSelection("groups", "work"));
    expect(restored.result.current[0]).toBe("board");
  });

  it("can update and clear a remembered selection", () => {
    const { result, unmount } = renderHook(() => useContactsSelection("all", null));
    act(() => result.current[1]("jane"));
    act(() => result.current[1](current => `${current}-saved`));
    expect(result.current[0]).toBe("jane-saved");
    act(() => result.current[1](null));
    unmount();
    expect(renderHook(() => useContactsSelection("all", null)).result.current[0]).toBeNull();
  });

  it("keeps navigation working when browser storage is unavailable", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("unavailable"); });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("unavailable"); });
    vi.spyOn(Storage.prototype, "removeItem").mockImplementation(() => { throw new Error("unavailable"); });
    const { result } = renderHook(() => useContactsSelection("all", null));
    act(() => result.current[1]("jane"));
    expect(result.current[0]).toBe("jane");
  });
});
