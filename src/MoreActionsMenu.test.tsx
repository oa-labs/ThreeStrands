import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { MoreActionsMenu } from "./MoreActionsMenu";

afterEach(cleanup);

describe("MoreActionsMenu onBeforeOpen", () => {
  const renderMenu = () => {
    const onBeforeOpen = vi.fn();
    render(<MoreActionsMenu label="Actions" onBeforeOpen={onBeforeOpen} actions={[{ label: "Do", onSelect: vi.fn() }]} />);
    return { onBeforeOpen, trigger: screen.getByRole("button", { name: "Actions" }) };
  };

  it("runs as a pointer press begins, once, not again for its click", () => {
    const { onBeforeOpen, trigger } = renderMenu();
    fireEvent.pointerDown(trigger);
    fireEvent.click(trigger, { detail: 1 });
    expect(onBeforeOpen).toHaveBeenCalledOnce();
    expect(screen.getByRole("menu")).toBeInTheDocument();
  });

  it("runs for a keyboard click and an arrow key, but not when the menu is already open", () => {
    const { onBeforeOpen, trigger } = renderMenu();
    fireEvent.click(trigger, { detail: 0 });
    expect(onBeforeOpen).toHaveBeenCalledOnce();
    fireEvent.click(trigger, { detail: 0 });
    fireEvent.keyDown(trigger, { key: "ArrowDown" });
    expect(onBeforeOpen).toHaveBeenCalledTimes(2);
    fireEvent.keyDown(trigger, { key: "ArrowDown" });
    expect(onBeforeOpen).toHaveBeenCalledTimes(2);
  });
});
