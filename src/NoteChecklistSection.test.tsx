import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { NoteChecklistSection } from "./NoteChecklistSection";
import type { NoteItem } from "./noteEntities";

const item = (title: string, checked = false, task = false): NoteItem => ({ id: title, title, checked, task, start: 0, markerStart: 0, markerEnd: 0, end: 0, persisted: true });

describe("note checklist preview", () => {
  afterEach(() => { cleanup(); localStorage.clear(); });

  it("previews unfinished items in note order and puts completed items behind their count", () => {
    const items = [item("Done first", true), item("First pending"), item("Done second", true, true), item("Second pending", false, true), item("Third pending"), item("Fourth pending"), item("Fifth pending"), item("Sixth pending"), item("Seventh pending")];
    const { rerender } = render(<NoteChecklistSection items={items} disabled={false} onChange={vi.fn()} />);
    expect(screen.getAllByRole("checkbox").map((box) => box.getAttribute("aria-label"))).toEqual(["Checklist: First pending", "Task: Second pending", "Checklist: Third pending", "Checklist: Fourth pending", "Checklist: Fifth pending"]);
    fireEvent.click(screen.getByRole("button", { name: "Show 2 more" }));
    expect(screen.getAllByRole("checkbox")).toHaveLength(7);
    const completed = screen.getByRole("button", { name: "Completed · 2" });
    expect(completed).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(completed);
    expect(screen.getAllByRole("checkbox").slice(-2).map((box) => box.getAttribute("aria-label"))).toEqual(["Checklist: Done first", "Task: Done second"]);
    expect(screen.getByRole("checkbox", { name: "Checklist: Done first" })).toBeChecked();
    rerender(<NoteChecklistSection items={[...items, item("Done third", true)]} disabled={false} onChange={vi.fn()} />);
    expect(screen.getByRole("button", { name: "Completed · 3" })).toHaveAttribute("aria-expanded", "true");
    expect(items.map((row) => row.title)).toEqual(["Done first", "First pending", "Done second", "Second pending", "Third pending", "Fourth pending", "Fifth pending", "Sixth pending", "Seventh pending"]);
  });

  it("shows a completed-only note as a collapsed count and lets an item be reopened", () => {
    const done = item("Finished", true, true);
    const onChange = vi.fn();
    render(<NoteChecklistSection items={[done]} disabled={false} onChange={onChange} />);
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Completed · 1" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "Task: Finished" }));
    expect(onChange).toHaveBeenCalledWith(done, { checked: false });
  });

  it("reveals full text and offers explicit conversion in a keyboard-accessible menu", () => {
    const long = item("Name and address followed by a long description that can wrap across many lines");
    const onChange = vi.fn();
    const { container, rerender } = render(<NoteChecklistSection items={[long]} disabled={false} onChange={onChange} />);
    const trigger = screen.getByRole("button", { name: `Actions for ${long.title}` });
    expect(screen.queryByRole("menuitem")).not.toBeInTheDocument();
    fireEvent.keyDown(trigger, { key: "ArrowDown" });
    const menu = screen.getByRole("menu");
    expect(within(menu).getByRole("menuitem", { name: "Show full text" })).toHaveFocus();
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Show full text" }));
    expect(container.querySelector(".note-item")).toHaveClass("note-item-expanded", "context-row-wrap");
    expect(trigger).toHaveFocus();
    expect(onChange).not.toHaveBeenCalled();
    fireEvent.click(trigger);
    fireEvent.click(screen.getByRole("menuitem", { name: "Show less text" }));
    expect(container.querySelector(".note-item")).not.toHaveClass("note-item-expanded");
    fireEvent.click(trigger);
    fireEvent.click(screen.getByRole("menuitem", { name: "Convert to Task" }));
    expect(onChange).toHaveBeenCalledWith(long, { task: true });
    rerender(<NoteChecklistSection items={[{ ...long, task: true }]} disabled={false} onChange={onChange} />);
    fireEvent.click(trigger);
    fireEvent.click(screen.getByRole("menuitem", { name: "Convert to Checklist" }));
    expect(onChange).toHaveBeenLastCalledWith({ ...long, task: true }, { task: false });
    fireEvent.click(trigger);
    fireEvent.keyDown(screen.getByRole("menuitem", { name: "Show full text" }), { key: "Escape" });
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });

  it("keeps full text available while mutations are disabled, and skips the disabled menu action", () => {
    const onChange = vi.fn();
    render(<NoteChecklistSection items={[item("Read only")]} disabled onChange={onChange} />);
    expect(screen.getByRole("checkbox")).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Actions for Read only" }));
    const fullText = screen.getByRole("menuitem", { name: "Show full text" });
    expect(screen.getByRole("menuitem", { name: "Convert to Task" })).toBeDisabled();
    fireEvent.keyDown(fullText, { key: "ArrowDown" });
    expect(fullText).toHaveFocus();
    fireEvent.click(fullText);
    expect(onChange).not.toHaveBeenCalled();
  });
});
