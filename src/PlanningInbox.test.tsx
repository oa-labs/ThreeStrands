import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PlanningInbox, uniqueInboxEntries, interleaveInboxCategories, INBOX_FILTER_KEY, INBOX_PAGE_SIZE, type InboxEntry } from "./PlanningInbox";
import { ICON_SIZE } from "./iconSizes";

const entry = (id: string, category: InboxEntry["category"] = "tasks", identities = [id]): InboxEntry => ({ id, identities, category, title: id, source: null, row: { onActivate: vi.fn() }, schedule: vi.fn(), actions: [{ label: "Add to Today's Plan", onSelect: vi.fn() }] });
const defaults = { scope: undefined, status: () => null, loading: () => false, hasMore: () => false, onLoadMore: vi.fn(), drag: { startDrag: vi.fn(), clickSuppressed: () => false, draggingId: null as string | null, enabled: true } };
afterEach(() => { cleanup(); localStorage.clear(); vi.clearAllMocks(); });

describe("PlanningInbox", () => {
  it("uses the keep-in-touch empty state only after Contacts finishes loading successfully", () => {
    const view = render(<PlanningInbox {...defaults} entries={[]} loading={() => true} />);
    fireEvent.click(screen.getByRole("tab", { name: "Contacts" }));
    expect(screen.queryByText("You're up to date with all keep in touch contact goals")).not.toBeInTheDocument();
    view.rerender(<PlanningInbox {...defaults} entries={[]} />);
    expect(screen.getByText("You're up to date with all keep in touch contact goals")).toBeInTheDocument();
    expect(screen.queryByText("Nothing to schedule here.")).not.toBeInTheDocument();
    view.rerender(<PlanningInbox {...defaults} entries={[]} unavailable={() => true} />);
    expect(screen.queryByText("You're up to date with all keep in touch contact goals")).not.toBeInTheDocument();
  });

  it("holds only unscheduled work: finished items live in the Done card, not under the inbox", () => {
    render(<PlanningInbox {...defaults} entries={[entry("Active")]} />);
    expect(screen.getByLabelText("1 unscheduled item")).toHaveTextContent("1");
    const icon = screen.getByRole("heading", { name: /Planning Inbox/ }).querySelector("svg");
    expect(icon).toHaveClass("lucide-inbox");
    expect(icon).toHaveAttribute("aria-hidden", "true");
    expect(icon).toHaveAttribute("width", String(ICON_SIZE.md));
    for (const filter of ["All", "Tasks", "Emails", "Contacts"]) {
      fireEvent.click(screen.getByRole("tab", { name: filter }));
      expect(screen.queryByRole("region", { name: "Completed" })).not.toBeInTheDocument();
    }
  });

  it("moves focus to the selected filter when completing a task removes the last inbox row", () => {
    const active = entry("Task"); active.row.control = <input aria-label="Complete Task" type="checkbox" checked={false} onChange={vi.fn()} />;
    const view = render(<PlanningInbox {...defaults} entries={[active]} />);
    fireEvent.click(screen.getByRole("tab", { name: "Tasks" }));
    const complete = screen.getByRole("checkbox", { name: "Complete Task" }); complete.focus(); fireEvent.click(complete);
    view.rerender(<PlanningInbox {...defaults} entries={[]} />);
    expect(screen.getByRole("tab", { name: "Tasks" })).toHaveFocus();
  });

  it("keeps checkbox focus on the next row when completing a task leaves others behind", () => {
    const first = entry("First"); first.row.control = <input aria-label="Complete First" type="checkbox" checked={false} onChange={vi.fn()} />;
    const second = entry("Second"); second.row.control = <input aria-label="Complete Second" type="checkbox" checked={false} onChange={vi.fn()} />;
    const view = render(<PlanningInbox {...defaults} entries={[first, second]} />);
    const complete = screen.getByRole("checkbox", { name: "Complete First" }); complete.focus(); fireEvent.click(complete);
    view.rerender(<PlanningInbox {...defaults} entries={[second]} />);
    expect(document.activeElement).not.toBe(document.body);
  });

  it("shows All as the union of the three categories, with a count for the selected filter", () => {
    render(<PlanningInbox {...defaults} entries={[entry("Task"), entry("Email", "emails"), entry("Contact", "contacts")]} />);
    expect(screen.getAllByRole("tab").map((tab) => tab.textContent)).toEqual(["All", "Tasks", "Emails", "Contacts"]);
    expect(screen.getAllByRole("article")).toHaveLength(3);
    expect(screen.getByLabelText("3 unscheduled items")).toHaveTextContent("3");
    for (const [filter, title] of [["Tasks", "Task"], ["Emails", "Email"], ["Contacts", "Contact"]]) {
      fireEvent.click(screen.getByRole("tab", { name: filter }));
      expect(screen.getAllByRole("article")).toHaveLength(1);
      expect(screen.getByRole("tabpanel")).toHaveAccessibleName(filter);
      expect(within(screen.getByRole("tabpanel")).getByText(title)).toBeInTheDocument();
      expect(screen.getByLabelText("1 unscheduled item")).toHaveTextContent("1");
    }
    expect(screen.queryByText(/^Meanwhile/)).not.toBeInTheDocument();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
  });

  it("deduplicates planned tasks, starred task sources, and contacts by durable identity", () => {
    const planned = entry("Planned", "tasks", ["plan:task-a", "task:a"]);
    expect(uniqueInboxEntries([planned, entry("Task source", "tasks", ["task:a"]), entry("Starred source", "tasks", ["email:t", "task:a"]), entry("Same email", "emails", ["email:t"]), entry("Contact", "contacts", ["contact:pat"]), entry("Contact source", "contacts", ["contact:pat"])])).toEqual([planned, expect.objectContaining({ title: "Contact" })]);
  });

  it("mixes the categories without changing task priority order", () => {
    const entries = [entry("First task"), entry("Second task"), entry("Third task"), entry("Email", "emails"), entry("Contact", "contacts")];
    expect(interleaveInboxCategories(entries).map((entry) => entry.title)).toEqual(["First task", "Email", "Contact", "Second task", "Third task"]);
    expect(interleaveInboxCategories([])).toEqual([]);
  });

  it("keeps scheduling and secondary actions accessible with native button activation", async () => {
    const item = entry("Review contract"); render(<PlanningInbox {...defaults} entries={[item]} />);
    const schedule = screen.getByRole("button", { name: "Schedule: Review contract" });
    expect(fireEvent.keyDown(schedule, { key: "Enter" })).toBe(true);
    expect(fireEvent.keyDown(schedule, { key: " " })).toBe(true);
    await act(async () => fireEvent.click(schedule)); expect(item.schedule).toHaveBeenCalledOnce();
    const menu = screen.getByRole("button", { name: "Actions for Review contract" });
    menu.focus(); fireEvent.keyDown(menu, { key: "ArrowDown" });
    expect(screen.getByRole("menuitem", { name: "Add to Today's Plan" })).toHaveFocus();
    fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    expect(menu).toHaveFocus();
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });

  it("prevents duplicate scheduling and moves focus to the next card when the booked card leaves", async () => {
    let finish!: () => void;
    const first = entry("First"); first.schedule = vi.fn(() => new Promise<void>((resolve) => { finish = resolve; }));
    const second = entry("Second");
    const view = render(<PlanningInbox {...defaults} entries={[first, second]} />);
    const button = screen.getByRole("button", { name: "Schedule: First" }); button.focus();
    fireEvent.click(button); fireEvent.click(button);
    expect(first.schedule).toHaveBeenCalledOnce(); expect(button).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Actions for First" }));
    expect(screen.getByRole("menuitem", { name: "Add to Today's Plan" })).toBeDisabled();
    fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });
    button.focus();
    await act(async () => finish());
    view.rerender(<PlanningInbox {...defaults} entries={[second]} />);
    expect(screen.getByRole("button", { name: "Second" })).toHaveFocus();
  });

  it("shows a failed scheduling action without removing the card or keeping its button disabled", async () => {
    const item = entry("Keep"); item.schedule = vi.fn(async () => { throw new Error("Booking failed"); });
    render(<PlanningInbox {...defaults} entries={[item]} />);
    fireEvent.click(screen.getByRole("button", { name: "Schedule: Keep" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Booking failed");
    expect(screen.getByRole("button", { name: "Schedule: Keep" })).toBeEnabled();
    expect(screen.getByRole("article")).toBeInTheDocument();
  });

  it("supports arrows, Home, End, wrapping, and a remembered choice without focusing a control on mount", () => {
    const before = document.createElement("button"); document.body.append(before); before.focus();
    render(<PlanningInbox {...defaults} entries={[]} />);
    expect(before).toHaveFocus();
    const all = screen.getByRole("tab", { name: "All" }); all.focus();
    fireEvent.keyDown(all, { key: "ArrowLeft" }); expect(screen.getByRole("tab", { name: "Contacts" })).toHaveFocus();
    fireEvent.keyDown(document.activeElement!, { key: "Home" }); expect(all).toHaveFocus();
    fireEvent.keyDown(all, { key: "ArrowRight" }); expect(screen.getByRole("tab", { name: "Tasks" })).toHaveFocus();
    fireEvent.keyDown(document.activeElement!, { key: "End" }); expect(screen.getByRole("tab", { name: "Contacts" })).toHaveFocus();
    expect(localStorage.getItem(INBOX_FILTER_KEY)).toBe("contacts");
    before.remove();
  });

  it("migrates prior Notes and Goals filters to Tasks without writing application data", () => {
    for (const previous of ["notes", "goals"]) {
      localStorage.clear(); localStorage.setItem("threestrands.today.planSource", previous);
      render(<PlanningInbox {...defaults} entries={[]} />);
      expect(screen.getByRole("tab", { name: "Tasks" })).toHaveAttribute("aria-selected", "true"); cleanup();
    }
  });

  it("pages one list without hiding a single remaining item or falsely claiming an exact count", () => {
    const entries = Array.from({ length: INBOX_PAGE_SIZE + 2 }, (_, i) => entry(`Task ${i}`));
    const view = render(<PlanningInbox {...defaults} entries={entries} />);
    expect(screen.getAllByRole("article")).toHaveLength(INBOX_PAGE_SIZE);
    fireEvent.click(screen.getByRole("button", { name: "Show More" }));
    expect(screen.getAllByRole("article")).toHaveLength(entries.length);
    expect(screen.queryByRole("button", { name: "Show More" })).not.toBeInTheDocument();
    view.rerender(<PlanningInbox {...defaults} entries={entries} hasMore={() => true} />);
    expect(screen.getByLabelText(`${entries.length} or more unscheduled items`)).toHaveTextContent(`${entries.length}+`);
    fireEvent.click(screen.getByRole("button", { name: "Show More" })); expect(defaults.onLoadMore).toHaveBeenCalledWith("all");
  });

  it("does not claim there is nothing to schedule when a page of settled suggestions has more pages", () => {
    render(<PlanningInbox {...defaults} entries={[]} hasMore={() => true} />);
    expect(screen.queryByText("Nothing to schedule here.")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Show More" })).toBeEnabled();
    expect(screen.getByLabelText("0 or more unscheduled items")).toHaveTextContent("0+");
  });

  it("preserves the filter and expanded list when fresh entries arrive", () => {
    const entries = Array.from({ length: INBOX_PAGE_SIZE + 2 }, (_, i) => entry(`Task ${i}`));
    const view = render(<PlanningInbox {...defaults} entries={entries} />);
    fireEvent.click(screen.getByRole("tab", { name: "Tasks" })); fireEvent.click(screen.getByRole("button", { name: "Show More" }));
    act(() => view.rerender(<PlanningInbox {...defaults} entries={[...entries, entry("Fresh")]} />));
    expect(screen.getByRole("tab", { name: "Tasks" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByText("Fresh")).toBeInTheDocument();
  });

  describe("adding a task inline", () => {
    const field = () => screen.getByRole("textbox", { name: "Add a task" });
    const type = (value: string) => fireEvent.change(field(), { target: { value } });

    it("is the last row, on All and Tasks only, and only when tasks can be created", () => {
      const view = render(<PlanningInbox {...defaults} entries={[entry("Task")]} onAddTask={vi.fn()} />);
      const after = (a: Node, b: Node) => Boolean(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING);
      expect(after(screen.getByText("Task"), field())).toBe(true);
      expect(screen.queryByRole("button", { name: "New Task" })).not.toBeInTheDocument();
      fireEvent.click(screen.getByRole("tab", { name: "Tasks" }));
      expect(field()).toBeInTheDocument();
      for (const filter of ["Emails", "Contacts"]) {
        fireEvent.click(screen.getByRole("tab", { name: filter }));
        expect(screen.queryByRole("textbox", { name: "Add a task" })).not.toBeInTheDocument();
      }
      view.rerender(<PlanningInbox {...defaults} entries={[]} />);
      fireEvent.click(screen.getByRole("tab", { name: "All" }));
      expect(screen.queryByRole("textbox", { name: "Add a task" })).not.toBeInTheDocument();
    });

    it("adds a trimmed title on Enter, clears the field, and keeps focus for the next task", async () => {
      const onAddTask = vi.fn().mockResolvedValue(undefined);
      render(<PlanningInbox {...defaults} entries={[]} onAddTask={onAddTask} />);
      field().focus();
      type("   ");
      fireEvent.keyDown(field(), { key: "Enter" });
      expect(onAddTask).not.toHaveBeenCalled();
      type("  Call Sam ");
      await act(async () => { fireEvent.keyDown(field(), { key: "Enter" }); });
      expect(onAddTask).toHaveBeenCalledExactlyOnceWith("Call Sam");
      expect(field()).toHaveValue("");
      expect(field()).toHaveFocus();
    });

    it("clears on Escape, and keeps the title with an alert when adding fails", async () => {
      const onAddTask = vi.fn().mockRejectedValue(new Error("Disk full"));
      render(<PlanningInbox {...defaults} entries={[]} onAddTask={onAddTask} />);
      type("Draft");
      await act(async () => { fireEvent.keyDown(field(), { key: "Enter" }); });
      expect(screen.getByRole("alert")).toHaveTextContent("Disk full");
      expect(field()).toHaveValue("Draft");
      fireEvent.keyDown(field(), { key: "Escape" });
      expect(field()).toHaveValue("");
      expect(screen.queryByRole("alert")).not.toBeInTheDocument();
      expect(field()).not.toHaveFocus();
    });

    it.each(["", "Draft"])("releases focus on Escape with title %j without passing Escape to navigation", (title) => {
      const onAddTask = vi.fn();
      const navigate = vi.fn();
      render(<div onKeyDown={navigate}><PlanningInbox {...defaults} entries={[]} onAddTask={onAddTask} /></div>);
      type(title);
      field().focus();
      expect(field()).toHaveFocus();
      fireEvent.keyDown(field(), { key: "Escape" });
      expect(document.body).toHaveFocus();
      expect(field()).toHaveValue("");
      expect(onAddTask).not.toHaveBeenCalled();
      expect(navigate).not.toHaveBeenCalled();
    });
  });
});
