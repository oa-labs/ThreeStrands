import { readFileSync } from "node:fs";
import postcss from "postcss";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Thread } from "./domain";
import { HighlightedSnippet, ThreadList, ThreadRow } from "./ThreadList";

afterEach(() => { cleanup(); vi.useRealTimers(); });

const thread: Thread = {
  id: "thread-1",
  providerThreadId: "provider-1",
  subject: "Roadmap update",
  snippet: "Plain &amp; safe",
  participants: ["Ada Lovelace <ada@example.com>"],
  lastMessageAt: "2026-09-18T12:00:00Z",
  lastReceivedAt: "2026-09-18T12:00:00Z",
  unread: true,
  starred: true,
  archived: false,
  trashed: false,
  labels: [],
  accountId: "ada@example.com",
  matchSnippet: "Before \u0001matched\u0002 after",
  summary: null,
  summaryGeneratedAt: null,
  summaryRevision: null,
  hasAttachments: true,
};

describe("ThreadList", () => {
  it("suppresses a parked pointer's hover background during keyboard use and restores it on pointer input", () => {
    // Resolve the real background rules with a simulated hover state; jsdom
    // does not provide a physical pointer for the :hover pseudo-class.
    const css = postcss.parse(readFileSync("src/styles.css", "utf8"));
    const background = (row: HTMLElement) => {
      let value = "";
      css.walkRules((rule) => {
        if (!rule.selector.includes(".thread-row")) return;
        if (rule.selectors.some((selector) => row.matches(selector.replaceAll(":hover", ".simulated-hover")))) {
          rule.walkDecls("background", (declaration) => { value = declaration.value; });
        }
      });
      return value;
    };
    render(
      <ThreadList role="listbox" aria-label="Inbox">
        <button className="thread-row simulated-hover">Parked pointer</button>
        <button className="thread-row selected simulated-hover" aria-selected="true">Current conversation</button>
      </ThreadList>,
    );
    const hovered = screen.getByRole("button", { name: "Parked pointer" });
    const selected = screen.getByRole("button", { name: "Current conversation" });
    expect(background(hovered)).toBe("var(--hover)");

    for (const key of ["j", "k", "ArrowDown", "ArrowUp", "Tab"]) {
      fireEvent.keyDown(window, { key });
      expect(background(hovered)).toBe("transparent");
      expect(background(selected)).toBe("var(--selected)");
      expect(selected).toHaveAttribute("aria-selected", "true");
      // Enter/leave can happen when keyboard scrolling moves rows beneath
      // the stationary pointer. Only movement or a press restores hover.
      fireEvent.pointerOver(hovered);
      expect(background(hovered)).toBe("transparent");
      fireEvent.pointerMove(hovered);
      expect(background(hovered)).toBe("var(--hover)");
    }
    fireEvent.keyDown(window, { key: "j" });
    fireEvent.pointerDown(hovered);
    expect(background(hovered)).toBe("var(--hover)");
    expect(background(selected)).toBe("var(--selected)");

    fireEvent.keyDown(window, { key: "Shift" });
    fireEvent.keyDown(window, { key: "j", isComposing: true });
    expect(background(hovered)).toBe("var(--hover)");
  });

  it("refreshes once per deliberate trackpad pull and waits for an in-flight refresh", async () => {
    vi.useFakeTimers();
    let finish!: () => void;
    const refresh = vi.fn(() => new Promise<void>((resolve) => { finish = resolve; }));
    render(<ThreadList role="listbox" aria-label="Inbox" onRefresh={refresh} />);
    const list = screen.getByRole("listbox");
    fireEvent.wheel(list, { deltaY: -39 });
    fireEvent.wheel(list, { deltaY: -40 });
    expect(refresh).not.toHaveBeenCalled();
    fireEvent.wheel(list, { deltaY: -1 });
    expect(refresh).toHaveBeenCalledTimes(1);
    fireEvent.wheel(list, { deltaY: -100 });
    await act(async () => { vi.advanceTimersByTime(300); });
    fireEvent.wheel(list, { deltaY: -100 });
    expect(refresh).toHaveBeenCalledTimes(1);
    await act(async () => { finish(); });
    fireEvent.wheel(list, { deltaY: -100 });
    expect(refresh).toHaveBeenCalledTimes(1);
    await act(async () => { vi.advanceTimersByTime(300); });
    fireEvent.wheel(list, { deltaY: -5, deltaMode: 1 });
    expect(refresh).toHaveBeenCalledTimes(2);
    await act(async () => { finish(); });
  });

  it("ignores scrolling to the top, horizontal movement, zoom, and pulls outside the list", async () => {
    vi.useFakeTimers();
    const refresh = vi.fn(async () => {});
    render(<ThreadList role="listbox" aria-label="Inbox" onRefresh={refresh} />);
    const list = screen.getByRole("listbox");
    list.scrollTop = 100;
    fireEvent.wheel(list, { deltaY: -100 });
    list.scrollTop = 0;
    fireEvent.wheel(list, { deltaY: -100 });
    expect(refresh).not.toHaveBeenCalled();
    for (const event of [{ deltaX: 200, deltaY: -100 }, { deltaY: -100, ctrlKey: true }, { deltaY: 100 }]) {
      await act(async () => { vi.advanceTimersByTime(300); });
      fireEvent.wheel(list, event);
    }
    fireEvent.wheel(window, { deltaY: -100 });
    expect(refresh).not.toHaveBeenCalled();
  });

  it("refreshes on a completed touch pull, ignoring short, sideways, cancelled and multi-touch gestures", async () => {
    const refresh = vi.fn(async () => {});
    render(<ThreadList role="listbox" aria-label="Inbox" onRefresh={refresh} />);
    const list = screen.getByRole("listbox");
    const point = (x: number, y: number) => ({ identifier: 1, clientX: x, clientY: y });
    for (const [x, y, cancel] of [[0, 79, false], [100, 80, false], [0, 100, true]] as const) {
      fireEvent.touchStart(list, { touches: [point(0, 0)] });
      fireEvent.touchMove(list, { touches: [point(x, y)] });
      if (cancel) fireEvent.touchCancel(list);
      fireEvent.touchEnd(list, { touches: [] });
    }
    fireEvent.touchStart(list, { touches: [point(0, 0)] });
    fireEvent.touchMove(list, { touches: [point(0, 100), { ...point(0, 100), identifier: 2 }] });
    fireEvent.touchEnd(list, { touches: [] });
    expect(refresh).not.toHaveBeenCalled();
    fireEvent.touchStart(list, { touches: [point(0, 0)] });
    fireEvent.touchMove(list, { touches: [point(0, 80)] });
    expect(refresh).not.toHaveBeenCalled();
    await act(async () => { fireEvent.touchEnd(list, { touches: [] }); });
    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it("clears a partial pull when the account callback changes", () => {
    const first = vi.fn(async () => {});
    const second = vi.fn(async () => {});
    const { rerender } = render(<ThreadList role="listbox" aria-label="Inbox" onRefresh={first} />);
    fireEvent.wheel(screen.getByRole("listbox"), { deltaY: -60 });
    rerender(<ThreadList role="listbox" aria-label="Inbox" onRefresh={second} />);
    fireEvent.wheel(screen.getByRole("listbox"), { deltaY: -30 });
    expect(first).not.toHaveBeenCalled();
    expect(second).not.toHaveBeenCalled();
  });

  it("renders highlighted search snippets without treating them as markup", () => {
    const { rerender } = render(<HighlightedSnippet thread={thread} />);
    expect(screen.getByText("matched")).toBeInTheDocument();
    expect(screen.getByText("matched").tagName).toBe("MARK");

    rerender(<HighlightedSnippet thread={{ ...thread, matchSnippet: null }} />);
    expect(screen.getByText("Plain & safe")).toBeInTheDocument();

    // The native inbox preview encodes every character the reader decodes
    // (see `list_preview` in src-tauri/src/db.rs).
    const { container } = render(<HighlightedSnippet thread={{ ...thread, matchSnippet: null, snippet: "Fixed &lt;now&gt; &amp; &quot;done&quot; — it&#39;s live." }} />);
    expect(container).toHaveTextContent('Fixed <now> & "done" — it\'s live.');
    expect(container.querySelector("now")).toBeNull();
  });

  it("separates row selection from the checkbox action", () => {
    const onSelect = vi.fn();
    const onToggleCheck = vi.fn();
    render(
      <ThreadRow
        thread={thread}
        selected
        checked
        showAccount
        accountColor="#123456"
        onSelect={onSelect}
        onToggleCheck={onToggleCheck}
      />,
    );

    const row = screen.getByRole("option", { name: /Roadmap update/ });
    expect(row).toHaveAttribute("aria-selected", "true");
    expect(screen.getByText("Selected for batch actions")).toBeInTheDocument();
    expect(screen.getByLabelText("Has attachments")).toBeInTheDocument();

    fireEvent.click(row.querySelector(".row-check")!);
    expect(onToggleCheck).toHaveBeenCalledWith("thread-1");
    expect(onSelect).not.toHaveBeenCalled();

    fireEvent.click(row);
    expect(onSelect).toHaveBeenCalledWith("thread-1");
  });

  it("marks only threads still in the inbox when showing locations", () => {
    const row = (overrides: Partial<typeof thread>, showLocation: boolean) => render(
      <ThreadRow
        thread={{ ...thread, ...overrides }}
        selected={false}
        checked={false}
        showAccount={false}
        onSelect={vi.fn()}
        onToggleCheck={vi.fn()}
        showLocation={showLocation}
      />,
    );
    const inbox = row({ archived: false, trashed: false }, true);
    expect(screen.getByText("Inbox", { selector: ".thread-location-tag" })).toBeInTheDocument();
    inbox.unmount();

    const archived = row({ archived: true }, true);
    expect(screen.queryByText("Inbox", { selector: ".thread-location-tag" })).not.toBeInTheDocument();
    archived.unmount();

    row({ archived: false, trashed: false }, false);
    expect(screen.queryByText("Inbox", { selector: ".thread-location-tag" })).not.toBeInTheDocument();
  });

  it("shows a list paperclip only when a thread has attachments", () => {
    const row = (hasAttachments: boolean) => render(
      <ThreadRow
        thread={{ ...thread, hasAttachments }}
        selected={false}
        checked={false}
        showAccount={false}
        onSelect={vi.fn()}
        onToggleCheck={vi.fn()}
      />,
    );
    const withAttachments = row(true);
    expect(screen.getByLabelText("Has attachments")).toBeVisible();
    withAttachments.unmount();

    row(false);
    expect(screen.queryByLabelText("Has attachments")).toBeNull();
  });

  it("routes modifier clicks to multi-select gestures instead of opening the row", () => {
    const onSelect = vi.fn();
    const onSelectionGesture = vi.fn();
    render(
      <ThreadRow
        thread={thread}
        selected={false}
        checked={false}
        showAccount={false}
        onSelect={onSelect}
        onToggleCheck={vi.fn()}
        onSelectionGesture={onSelectionGesture}
      />,
    );
    const row = screen.getByRole("option", { name: /Roadmap update/ });

    fireEvent.click(row, { metaKey: true });
    fireEvent.click(row, { ctrlKey: true });
    fireEvent.click(row, { shiftKey: true });
    fireEvent.click(row, { shiftKey: true, metaKey: true });
    expect(onSelectionGesture.mock.calls).toEqual([
      ["thread-1", "toggle"],
      ["thread-1", "toggle"],
      ["thread-1", "range"],
      ["thread-1", "range-add"],
    ]);
    expect(onSelect).not.toHaveBeenCalled();

    expect(fireEvent.mouseDown(row, { shiftKey: true })).toBe(false);
    expect(fireEvent.mouseDown(row)).toBe(true);
  });

  it("toggles one checkbox on a click and adds a range on a Shift-click", () => {
    const onToggleCheck = vi.fn();
    const onSelectionGesture = vi.fn();
    const onSelect = vi.fn();
    const { container } = render(
      <ThreadRow thread={thread} selected={false} checked={false} showAccount={false}
        onSelect={onSelect} onToggleCheck={onToggleCheck} onSelectionGesture={onSelectionGesture} />,
    );
    const checkbox = container.querySelector(".row-check")!;
    fireEvent.click(checkbox);
    fireEvent.click(checkbox, { shiftKey: true });
    expect(onToggleCheck.mock.calls).toEqual([["thread-1"]]);
    expect(onSelectionGesture.mock.calls).toEqual([["thread-1", "range-add"]]);
    expect(onSelect).not.toHaveBeenCalled();
  });
});
