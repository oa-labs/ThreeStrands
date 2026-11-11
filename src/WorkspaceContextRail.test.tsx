import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { runWorkspaceCommand } from "./workspaceCommands";
import { act } from "react";
import { WorkspaceContextRail } from "./WorkspaceContextRail";

describe("shared workspace context rail",()=>{
  afterEach(()=>{cleanup();localStorage.clear();});
  it("uses the email framing and retains editable content while collapsed",async()=>{
    render(<WorkspaceContextRail label="Task context"><input aria-label="Notes" defaultValue="Draft" /></WorkspaceContextRail>);
    const input=screen.getByRole("textbox",{name:"Notes"});
    expect(screen.getByRole("complementary",{name:"Task context"})).toHaveClass("context-panel");
    fireEvent.change(input,{target:{value:"Unsaved text"}});
    fireEvent.click(screen.getByRole("button",{name:"Hide Task context"}));
    expect(screen.queryByRole("textbox",{name:"Notes"})).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button",{name:"Show Task context"}));
    expect(screen.getByRole("textbox",{name:"Notes"})).toHaveValue("Unsaved text");
    fireEvent.click(screen.getByRole("button",{name:"Hide Task context"}));
    window.dispatchEvent(new CustomEvent("threestrands:focus-context",{detail:"Task context"}));
    await waitFor(()=>expect(input).toHaveFocus());
  });

  describe("hiding from the divider", () => {
    const wide = Object.getOwnPropertyDescriptor(window, "innerWidth");
    const rail = (label = "Note context", onReturn?: () => void) => render(<WorkspaceContextRail label={label} onReturn={onReturn}><button type="button">Inside</button></WorkspaceContextRail>);
    const divider = (label = "Note context") => {
      const handle = screen.getByRole("separator", { name: `Resize ${label}`, hidden: true });
      handle.setPointerCapture = vi.fn();
      handle.releasePointerCapture = vi.fn();
      return handle;
    };
    const width = (container: HTMLElement) => (container.querySelector(".workspace-context-container") as HTMLElement).style.getPropertyValue("--workspace-context-width");
    const isHidden = (label = "Note context") => screen.queryByRole("complementary", { name: label }) === null;
    afterEach(() => { if (wide) Object.defineProperty(window, "innerWidth", wide); });

    it("narrows to the minimum, hides past half of it, and shows again when dragged back out", () => {
      Object.defineProperty(window, "innerWidth", { value: 1600, configurable: true });
      const { container } = rail();
      const handle = divider();
      // The panel is on the divider's right, so dragging right narrows it.
      fireEvent.pointerDown(handle, { button: 0, pointerId: 1, clientX: 1000 });
      fireEvent.pointerMove(handle, { pointerId: 1, clientX: 1100 });
      expect(width(container)).toBe("300px");
      expect(isHidden()).toBe(false);
      fireEvent.pointerMove(handle, { pointerId: 1, clientX: 1200 });
      expect(isHidden()).toBe(true);
      fireEvent.pointerUp(handle, { pointerId: 1, clientX: 1200 });
      expect(screen.getByRole("button", { name: "Show Note context" })).toHaveAccessibleName("Show Note context");
      expect(handle).toHaveAttribute("tabindex", "-1");
      // The width before hiding is kept.
      expect(localStorage.getItem("threestrands.workspaceContext.collapsed.Note context")).toBe("true");

      fireEvent.pointerDown(handle, { button: 0, pointerId: 2, clientX: 1500 });
      fireEvent.pointerMove(handle, { pointerId: 2, clientX: 1400 });
      expect(isHidden()).toBe(true);
      fireEvent.pointerMove(handle, { pointerId: 2, clientX: 1100 });
      expect(isHidden()).toBe(false);
      expect(width(container)).toBe("400px");
      fireEvent.pointerUp(handle, { pointerId: 2, clientX: 1100 });
      expect(handle).toHaveAttribute("tabindex", "0");
    });

    it("hides when the keyboard narrows it past the minimum, and leaves the view focused", () => {
      Object.defineProperty(window, "innerWidth", { value: 1600, configurable: true });
      const onReturn = vi.fn();
      rail("Note context", onReturn);
      const handle = divider();
      fireEvent.keyDown(handle, { key: "Home" });
      expect(isHidden()).toBe(false);
      handle.focus();
      fireEvent.keyDown(handle, { key: "ArrowRight" });
      expect(isHidden()).toBe(true);
      expect(onReturn).not.toHaveBeenCalled();
      fireEvent.click(screen.getByRole("button", { name: "Show Note context" }));
      expect(isHidden()).toBe(false);
    });

    it("toggles with Toggle Side Panel, returning focus to the view when it was inside", () => {
      const onReturn = vi.fn();
      rail("Note context", onReturn);
      act(() => runWorkspaceCommand("sidePanel.toggle"));
      expect(isHidden()).toBe(true);
      expect(onReturn).not.toHaveBeenCalled();
      act(() => runWorkspaceCommand("sidePanel.toggle"));
      expect(isHidden()).toBe(false);
      expect(document.body).toHaveFocus();
      screen.getByRole("button", { name: "Inside" }).focus();
      act(() => runWorkspaceCommand("sidePanel.toggle"));
      expect(onReturn).toHaveBeenCalledOnce();
    });

    it("remembers hiding for each view and shares one width across them", async () => {
      Object.defineProperty(window, "innerWidth", { value: 1600, configurable: true });
      const first = rail("Note context");
      fireEvent.keyDown(divider(), { key: "End" });
      await waitFor(() => expect(localStorage.getItem("threestrands.workspaceContextWidth")).toBe("520"));
      act(() => runWorkspaceCommand("sidePanel.toggle"));
      first.unmount();
      const second = rail("File context");
      expect(isHidden("File context")).toBe(false);
      expect(width(second.container)).toBe("520px");
      second.unmount();
      rail("Note context");
      expect(isHidden()).toBe(true);
    });
  });
});
