import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { focusContextPanel, handleContextPanelKeyDown } from "./contextPanelFocus";

function Panel({ children, onReturn }: { children: React.ReactNode; onReturn(): void }) {
  return <aside aria-label="Panel" tabIndex={-1} onKeyDown={(event) => handleContextPanelKeyDown(event, onReturn)}>{children}</aside>;
}

describe("focusContextPanel", () => {
  afterEach(cleanup);

  it("prefers a fix in Before you send, then Find Times, then the first control, then the panel", () => {
    const { rerender } = render(<Panel onReturn={vi.fn()}>
      <button>Recent email</button>
      <section className="compose-availability"><button>Find Times</button></section>
      <section className="compose-checks"><div className="compose-check"><button>Use john@acme.com</button></div></section>
    </Panel>);
    const panel = screen.getByRole("complementary", { name: "Panel" });
    focusContextPanel(panel);
    expect(screen.getByRole("button", { name: "Use john@acme.com" })).toHaveFocus();

    rerender(<Panel onReturn={vi.fn()}><button>Recent email</button><section className="compose-availability"><button>Find Times</button></section></Panel>);
    focusContextPanel(panel);
    expect(screen.getByRole("button", { name: "Find Times" })).toHaveFocus();

    rerender(<Panel onReturn={vi.fn()}><div hidden><button>Collapsed</button></div><button>Recent email</button></Panel>);
    focusContextPanel(panel);
    expect(screen.getByRole("button", { name: "Recent email" })).toHaveFocus();

    rerender(<Panel onReturn={vi.fn()}><p>Nothing to press</p></Panel>);
    focusContextPanel(panel);
    expect(panel).toHaveFocus();
  });
});

describe("handleContextPanelKeyDown", () => {
  afterEach(cleanup);

  it("keeps Tab inside the panel and returns to the draft on Escape without letting it close the draft", () => {
    const onReturn = vi.fn();
    const windowEscape = vi.fn();
    window.addEventListener("keydown", windowEscape);
    render(<Panel onReturn={onReturn}><button>First</button><button>Last</button></Panel>);
    const first = screen.getByRole("button", { name: "First" });
    const last = screen.getByRole("button", { name: "Last" });

    last.focus();
    fireEvent.keyDown(last, { key: "Tab" });
    expect(first).toHaveFocus();
    fireEvent.keyDown(first, { key: "Tab", shiftKey: true });
    expect(last).toHaveFocus();

    const escape = new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true });
    last.dispatchEvent(escape);
    expect(onReturn).toHaveBeenCalledTimes(1);
    expect(escape.defaultPrevented).toBe(true);
    // The composer's Escape listener sits on the window and must never see it.
    expect(windowEscape.mock.calls.some(([event]) => (event as KeyboardEvent).key === "Escape")).toBe(false);
    window.removeEventListener("keydown", windowEscape);
  });
});
