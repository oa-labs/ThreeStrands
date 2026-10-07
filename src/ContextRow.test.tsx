import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ContextRow } from "./ContextSections";
import { formatHistoryDate } from "./contactContext";

/** jsdom does no layout, so a line's measured widths are set by hand. */
function setWidths(element: Element, scrollWidth: number, clientWidth: number) {
  Object.defineProperty(element, "scrollWidth", { configurable: true, value: scrollWidth });
  Object.defineProperty(element, "clientWidth", { configurable: true, value: clientWidth });
}

describe("ContextRow", () => {
  it("shows a line's full text on hover only while it is cut off", () => {
    const { container } = render(<ContextRow title="Re: User randomly losing location access" date="Oct 7" detail="Awesome, thank you so much!" onActivate={() => {}} />);
    const row = container.querySelector(".context-row")!;
    const title = container.querySelector(".context-row-title")!;
    const detail = container.querySelector(".context-row-detail")!;
    setWidths(title, 320, 180);
    setWidths(detail, 150, 180);
    fireEvent.mouseEnter(row);
    expect(title).toHaveAttribute("title", "Re: User randomly losing location access");
    // A line that fits would only repeat itself.
    expect(detail).not.toHaveAttribute("title");

    // The panel can widen; the next hover drops the tooltip once the text fits.
    setWidths(title, 180, 180);
    fireEvent.mouseEnter(row);
    expect(title).not.toHaveAttribute("title");
  });

  it("notices text cut off inside a line, such as a filename's base or a meeting's attendees", () => {
    const { container } = render(<ContextRow title={<><span className="context-file-base">Screenshot 2026-09-16 at 11.49.10 AM</span>.png</>} date="Sep 16" detail="57 KB" onActivate={() => {}} />);
    const title = container.querySelector(".context-row-title")!;
    setWidths(title, 200, 200);
    setWidths(title.querySelector(".context-file-base")!, 260, 150);
    fireEvent.mouseEnter(container.querySelector(".context-row")!);
    expect(title).toHaveAttribute("title", "Screenshot 2026-09-16 at 11.49.10 AM.png");
  });

  it("keeps the glyph column and puts the date on the title line, with or without a glyph", () => {
    for (const element of [
      <ContextRow key="bare" title="You" date="Oct 7" detail="Progress on the Area Directors" />,
      <ContextRow key="control" control={<input type="checkbox" aria-label="Mark done" />} title="Email the sync errors" date="Due Tomorrow" />,
    ]) {
      const { container, unmount } = render(element);
      expect(container.querySelector(".context-row-glyph")).not.toBeNull();
      expect(container.querySelector(".context-row-date")!.parentElement).toHaveClass("context-row-line");
      unmount();
    }
  });
  it("shows a dated row's date as a month-and-day tile, still named for assistive technology", () => {
    const now = new Date();
    const thisYear = new Date(now.getFullYear(), 0, 15, 12).toISOString();
    const { container, unmount } = render(<ContextRow title="Budget review" dateTile={thisYear} detail="Numbers attached" onActivate={() => {}} />);
    const tile = container.querySelector(".context-row-glyph .context-calendar-date-tile")!;
    expect(tile.querySelector(".context-calendar-date-tile-day")).toHaveTextContent(/^15$/);
    expect(tile.querySelector(".context-calendar-date-tile-month")!.textContent).not.toBe("");
    expect(tile).toHaveAttribute("title");
    const date = container.querySelector(".context-row-line .context-row-date")!;
    expect(date.querySelector(".sr-only")).toHaveTextContent(formatHistoryDate(thisYear));
    // Within the current year the tile says it all, so nothing else shows on the title line.
    expect(date.querySelector("[aria-hidden]")).toBeNull();
    unmount();

    const lastYear = new Date(now.getFullYear() - 1, 2, 4, 12).toISOString();
    const { container: older } = render(<ContextRow title="Old thread" dateTile={lastYear} />);
    expect(older.querySelector(".context-row-date [aria-hidden]")).toHaveTextContent(String(now.getFullYear() - 1));
  });
});
