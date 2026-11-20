import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { NoteItem } from "../../src/noteEntities";
import { meetingLine } from "../../src/noteMeetings";
import { NotePreview, renderPreview } from "./NotePreview";

const byteOffset = (text: string, index: number) => new TextEncoder().encode(text.slice(0, index)).length;

function item(body: string, marker: string, id: string, title: string, task: boolean): NoteItem {
  const index = body.indexOf(marker);
  const markerStart = byteOffset(body, index);
  return { id, title, task, checked: marker.includes("x"), persisted: task,
    start: markerStart - 2, markerStart, markerEnd: markerStart + marker.length, end: markerStart + marker.length };
}

describe("mobile Markdown preview", () => {
  it("uses byte offsets to bind square checklists and round tasks to different source items", () => {
    const body = "Café ☕\n\n- [ ] Pack lunch\n- (x) Call the library";
    const square = item(body, "[ ]", "square-id", "Pack lunch", false);
    const round = item(body, "(x)", "round-id", "Call the library", true);
    const onCheck = vi.fn();
    render(<NotePreview body={body} items={[square, round]} disabled={false} onCheck={onCheck} onWiki={vi.fn()} onUrl={vi.fn()} />);
    const checklist = screen.getByRole("checkbox", { name: "Pack lunch", checked: false });
    const task = screen.getByRole("checkbox", { name: "Call the library", checked: true });
    expect(checklist).toHaveClass("square");
    expect(task).toHaveClass("round");
    expect(checklist.closest("li")).toHaveClass("note-check-item");
    expect(checklist.nextElementSibling).toHaveTextContent("Pack lunch");
    expect(task.nextElementSibling).toHaveTextContent("Call the library");
    fireEvent.click(task);
    expect(onCheck).toHaveBeenCalledWith(round);
  });

  it("puts generated meeting titles before metadata and retains nested notes", () => {
    const event = { version: 1, kind: "event", id: "event", account: "calendar-account", calendar: "calendar" } as const;
    const contact = { version: 1, kind: "contact", id: "riley@example.test" } as const;
    const planned = meetingLine({
      event, time: "9:00 am", name: "Library planning", linkNote: true,
      attendees: [{ label: "Riley", reference: contact }],
    }, "Library planning");
    const allDay = meetingLine({ event, time: "All day", name: "Community fair", linkNote: false, attendees: [] }, null);
    const body = [
      "## Meetings", "", planned, "  - Bring drawings", allDay, "- Ordinary meeting notes",
      "", "## Elsewhere", "", planned,
    ].join("\n");
    const onWiki = vi.fn();
    const { container } = render(<NotePreview daily body={body} items={[]} disabled={false}
      onCheck={vi.fn()} onWiki={onWiki} onUrl={vi.fn()} />);
    const [linked, plain] = container.querySelectorAll(".meeting-item");
    expect(linked.firstElementChild).toHaveTextContent("Library planning");
    expect(linked.children[1]).toHaveTextContent("9:00 am · Riley");
    expect(linked.querySelector("ul")).toHaveTextContent("Bring drawings");
    expect(plain.firstElementChild).toHaveTextContent("Community fair");
    expect(plain.children[1]).toHaveTextContent("All day");
    expect(screen.getByText("Ordinary meeting notes").closest("li")).not.toHaveClass("meeting-item");
    fireEvent.click(screen.getAllByRole("button", { name: "Library planning" })[0]);
    expect(onWiki).toHaveBeenCalledWith("Library planning");
  });

  it("preserves nested checklist items and ordinary bullets", () => {
    const body = "- A paragraph\n  - [ ] Water the seedlings before sunrise\n    - Bring the watering can\n  - Write a summary";
    const square = item(body, "[ ]", "square-id", "Water the seedlings before sunrise", false);
    render(<NotePreview body={body} items={[square]} disabled={false} onCheck={vi.fn()} onWiki={vi.fn()} onUrl={vi.fn()} />);
    const check = screen.getByRole("checkbox", { name: square.title });
    expect(check.nextElementSibling).toHaveTextContent("Bring the watering can");
    expect(screen.getByText("Write a summary").closest("li")).not.toHaveClass("note-check-item");
  });

  it("renders safe note links and never loads remote images or accepts authored action markup", () => {
    const html = renderPreview([
      '[[Library|Open Library]]',
      '[Read](https://example.com/article)',
      '[Unsafe](javascript:alert%281%29)',
      '![Photo](https://example.com/track.png)',
      '![Receipt](attachment:test)',
      '<button data-note-item="fake" onclick="alert(1)">Danger</button>',
    ].join("\n\n"), [], false);
    const result = document.createElement("div");
    result.innerHTML = html;
    expect(result.querySelector("[data-wiki]")).toHaveAttribute("data-wiki", "Library");
    expect(result.querySelectorAll("[data-external]")).toHaveLength(1);
    expect(result.querySelector("img,[data-note-item],[onclick],a")).toBeNull();
    expect(result).toHaveTextContent("Receipt");
  });
});
