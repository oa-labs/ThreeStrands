import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { NoteItem } from "../../src/noteEntities";
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
    fireEvent.click(task);
    expect(onCheck).toHaveBeenCalledWith(round);
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
