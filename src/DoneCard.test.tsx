import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { formatClockTime } from "./agendaEvents";
import { ICON_SIZE } from "./iconSizes";
import { DONE_COLLAPSED_ROWS, DONE_FILTER_KEY, doneSummary, DoneCard, sortDoneEntries, uniqueDoneEntries, type DoneEntry } from "./DoneCard";

const entry = (id: string, category: DoneEntry["category"], at: string | null, identities = [id]): DoneEntry => ({ id, identities, category, title: id, at, row: { onActivate: vi.fn() } });
const defaults = { title: "Done Today", today: true, errors: {}, loading: false };
const at = (hours: number, minutes = 0) => new Date(2026, 10, 17, hours, minutes).toISOString();
afterEach(() => { cleanup(); localStorage.clear(); });

describe("DoneCard", () => {
  it("lists every category newest first with each row's time, and rows without a time last", () => {
    render(<DoneCard {...defaults} entries={[entry("Planned", "tasks", null), entry("Early task", "tasks", at(8, 5)), entry("Sent mail", "emails", at(14, 14)), entry("New note", "notes", at(11, 30))]} />);
    const rows = within(screen.getByRole("tabpanel")).getAllByRole("button");
    expect(rows.map((row) => row.textContent)).toEqual([
      `Sent mail${formatClockTime(new Date(at(14, 14)))}`, `New note${formatClockTime(new Date(at(11, 30)))}`, `Early task${formatClockTime(new Date(at(8, 5)))}`, "Planned",
    ]);
  });

  it("names the heading, counts every row, and summarizes only the nonzero categories", () => {
    const view = render(<DoneCard {...defaults} entries={[entry("a", "tasks", at(9)), entry("b", "tasks", at(10)), entry("c", "tasks", at(11)), entry("d", "emails", at(12)), entry("e", "emails", at(13)), entry("m", "meetings", at(13, 30)), entry("f", "notes", at(14))]} />);
    expect(screen.getByRole("region", { name: "Done Today" })).toBeInTheDocument();
    const icon = screen.getByRole("heading", { name: /Done Today/ }).querySelector("svg");
    expect(icon).toHaveClass("lucide-circle-check");
    expect(icon).toHaveAttribute("aria-hidden", "true");
    expect(icon).toHaveAttribute("width", String(ICON_SIZE.md));
    expect(screen.getByLabelText("7 finished items")).toHaveTextContent("7");
    expect(screen.getByText("3 tasks · 2 emails · 1 meeting · 1 note")).toBeInTheDocument();
    view.rerender(<DoneCard {...defaults} entries={[entry("a", "notes", at(9))]} />);
    expect(screen.getByLabelText("1 finished item")).toBeInTheDocument();
    expect(screen.getByText("1 note")).toBeInTheDocument();
    expect(doneSummary([entry("m1", "meetings", at(9)), entry("m2", "meetings", at(10))])).toBe("2 meetings");
    expect(doneSummary([])).toBe("");
  });

  it("filters to Tasks, Emails, or Notes and remembers the choice under its own key", () => {
    const entries = [entry("A task", "tasks", at(9)), entry("An email", "emails", at(10)), entry("A meeting", "meetings", at(10, 30)), entry("A note", "notes", at(11))];
    const view = render(<DoneCard {...defaults} entries={entries} />);
    expect(screen.getAllByRole("tab").map((tab) => tab.textContent)).toEqual(["All", "Tasks", "Emails", "Meetings", "Notes"]);
    for (const [filter, title] of [["Tasks", "A task"], ["Emails", "An email"], ["Meetings", "A meeting"], ["Notes", "A note"]]) {
      fireEvent.click(screen.getByRole("tab", { name: filter }));
      expect(screen.getByRole("tabpanel")).toHaveAccessibleName(filter);
      expect(within(screen.getByRole("tabpanel")).getAllByRole("button")).toHaveLength(1);
      expect(within(screen.getByRole("tabpanel")).getByRole("button", { name: new RegExp(`^${title}`) })).toBeInTheDocument();
      expect(localStorage.getItem(DONE_FILTER_KEY)).toBe(filter.toLowerCase());
    }
    expect(localStorage.getItem("threestrands.today.inboxFilter")).toBeNull();
    view.unmount();
    render(<DoneCard {...defaults} entries={entries} />);
    expect(screen.getByRole("tab", { name: "Notes" })).toHaveAttribute("aria-selected", "true");
  });

  it("moves between the filters with arrow keys, Home, and End", () => {
    render(<DoneCard {...defaults} entries={[]} />);
    const all = screen.getByRole("tab", { name: "All" });
    all.focus();
    fireEvent.keyDown(all, { key: "ArrowRight" });
    expect(screen.getByRole("tab", { name: "Tasks" })).toHaveFocus();
    fireEvent.keyDown(screen.getByRole("tab", { name: "Tasks" }), { key: "End" });
    expect(screen.getByRole("tab", { name: "Notes" })).toHaveFocus();
    fireEvent.keyDown(screen.getByRole("tab", { name: "Notes" }), { key: "ArrowRight" });
    expect(screen.getByRole("tab", { name: "All" })).toHaveFocus();
  });

  it("collapses past five rows behind Show All, and collapses again when the filter changes", () => {
    const entries = Array.from({ length: DONE_COLLAPSED_ROWS + 2 }, (_, index) => entry(`Task ${index}`, "tasks", at(8 + index)));
    render(<DoneCard {...defaults} entries={entries} />);
    const rows = () => within(screen.getByRole("tabpanel")).getAllByRole("button", { name: /^Task / });
    expect(rows()).toHaveLength(DONE_COLLAPSED_ROWS);
    fireEvent.click(screen.getByRole("button", { name: "Show All" }));
    expect(rows()).toHaveLength(DONE_COLLAPSED_ROWS + 2);
    fireEvent.click(screen.getByRole("button", { name: "Show Fewer" }));
    expect(rows()).toHaveLength(DONE_COLLAPSED_ROWS);
    fireEvent.click(screen.getByRole("button", { name: "Show All" }));
    fireEvent.click(screen.getByRole("tab", { name: "Tasks" }));
    expect(rows()).toHaveLength(DONE_COLLAPSED_ROWS);
  });

  it("does not collapse exactly five rows", () => {
    render(<DoneCard {...defaults} entries={Array.from({ length: DONE_COLLAPSED_ROWS }, (_, index) => entry(`Task ${index}`, "tasks", at(8 + index)))} />);
    expect(screen.queryByRole("button", { name: "Show All" })).not.toBeInTheDocument();
  });

  it("keeps results in a keyboard-accessible scroll area and resets scrolling when switching filters", () => {
    const entries = [
      ...Array.from({ length: DONE_COLLAPSED_ROWS + 2 }, (_, index) => entry(`Task ${index}`, "tasks", at(8 + index))),
      { ...entry("A long email subject ".repeat(20), "emails", at(15)), row: { subtitle: "Recipients ".repeat(30), onActivate: vi.fn() } },
    ];
    const view = render(<DoneCard {...defaults} entries={entries} />);
    const panel = screen.getByRole("tabpanel");
    expect(panel).toHaveClass("today-done-items");
    expect(panel).toHaveAttribute("tabindex", "0");
    expect(panel).not.toContainElement(screen.getByRole("tablist"));
    fireEvent.click(screen.getByRole("button", { name: "Show All" }));
    expect(within(panel).getAllByRole("button", { name: /^Task / })).toHaveLength(DONE_COLLAPSED_ROWS + 2);
    for (const filter of ["Tasks", "Emails", "Notes", "All"]) {
      panel.scrollTop = 200;
      fireEvent.click(screen.getByRole("tab", { name: filter }));
      expect(screen.getByRole("tabpanel")).toBe(panel);
      expect(panel.scrollTop).toBe(0);
      expect(panel).toHaveClass("today-done-items");
    }
    view.rerender(<DoneCard {...defaults} entries={[]} errors={{ emails: "Offline" }} loading />);
    expect(screen.getByRole("tabpanel")).toBe(panel);
    expect(within(panel).getByRole("alert")).toHaveTextContent("Offline");
    expect(panel).toHaveAttribute("aria-busy", "true");
  });

  it("says nothing was finished yet today, nothing was recorded on a past day, and reading while loading", () => {
    const view = render(<DoneCard {...defaults} entries={[]} />);
    expect(screen.getByText("Nothing finished yet")).toBeInTheDocument();
    view.rerender(<DoneCard {...defaults} title="Done" today={false} entries={[]} />);
    expect(screen.getByText("Nothing recorded")).toBeInTheDocument();
    expect(screen.queryByText("Nothing finished yet")).not.toBeInTheDocument();
    view.rerender(<DoneCard {...defaults} entries={[]} loading />);
    expect(screen.queryByText("Nothing finished yet")).not.toBeInTheDocument();
    expect(screen.getByRole("tabpanel")).toHaveAttribute("aria-busy", "true");
  });

  it("shows each source's error on the filters it belongs to without hiding the other rows", () => {
    render(<DoneCard {...defaults} entries={[entry("A task", "tasks", at(9))]} errors={{ noteTasks: "Note tasks failed", emails: "Mail is offline", meetings: "Calendar is offline", notes: "Notes are locked" }} />);
    expect(screen.getAllByRole("alert").map((alert) => alert.textContent)).toEqual(["Note tasks: Note tasks failed", "Sent mail: Mail is offline", "Meetings: Calendar is offline", "Notes: Notes are locked"]);
    expect(screen.getByRole("button", { name: /^A task/ })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: "Emails" }));
    expect(screen.getAllByRole("alert").map((alert) => alert.textContent)).toEqual(["Sent mail: Mail is offline"]);
    fireEvent.click(screen.getByRole("tab", { name: "Meetings" }));
    expect(screen.getAllByRole("alert").map((alert) => alert.textContent)).toEqual(["Meetings: Calendar is offline"]);
    fireEvent.click(screen.getByRole("tab", { name: "Notes" }));
    expect(screen.getAllByRole("alert").map((alert) => alert.textContent)).toEqual(["Notes: Notes are locked"]);
    fireEvent.click(screen.getByRole("tab", { name: "Tasks" }));
    expect(screen.getAllByRole("alert").map((alert) => alert.textContent)).toEqual(["Note tasks: Note tasks failed"]);
  });

  it("shows finished rows muted with a check-state class and keeps a row's own control", () => {
    const done = entry("Reopenable", "tasks", at(9));
    const onChange = vi.fn();
    done.row.control = <input type="checkbox" checked aria-label="Mark Reopenable Not Done" onChange={onChange} />;
    render(<DoneCard {...defaults} entries={[done]} />);
    expect(screen.getByRole("button", { name: /^Reopenable/ }).closest(".context-task-done")).not.toBeNull();
    fireEvent.click(screen.getByRole("checkbox", { name: "Mark Reopenable Not Done" }));
    expect(onChange).toHaveBeenCalled();
  });

  it("puts focus on the selected filter when reopening a task removes its row", () => {
    const done = entry("Reopenable", "tasks", at(9));
    done.row.control = <input type="checkbox" checked aria-label="Mark Reopenable Not Done" onChange={vi.fn()} />;
    const view = render(<DoneCard {...defaults} entries={[done]} />);
    const box = screen.getByRole("checkbox", { name: "Mark Reopenable Not Done" });
    box.focus(); fireEvent.click(box);
    view.rerender(<DoneCard {...defaults} entries={[]} />);
    expect(screen.getByRole("tab", { name: "All" })).toHaveFocus();
  });

  it("merges rows that name the same task and orders by instant, not text", () => {
    const first = entry("Timed", "tasks", at(9), ["task:a", "plan:a"]);
    const second = entry("Planned copy", "tasks", null, ["plan:a"]);
    expect(uniqueDoneEntries([first, second])).toEqual([first]);
    expect(sortDoneEntries([entry("b", "tasks", "2026-11-17T09:00:00+01:00"), entry("a", "tasks", "2026-11-17T09:30:00Z")]).map((row) => row.id)).toEqual(["a", "b"]);
  });
});
