import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TodayMailSignal } from "./TodayMailSignal";
import { todayItem } from "./test/todayMail";

const replies = (count: number) => Array.from({ length: count }, (_, index) => todayItem(`r${index}`));

function renderSignal(count: number, scope?: string) {
  render(<TodayMailSignal mail={{ threads: replies(count), total: count }} error={null} scope={scope} onOpenThread={vi.fn()} onOpenInbox={vi.fn()} />);
  return within(screen.getByRole("group", { name: "Mail" }));
}

describe("TodayMailSignal", () => {
  beforeEach(() => localStorage.clear());
  afterEach(cleanup);

  it.each([[1, "1 needs a reply"], [3, "3 need a reply"]])("says %i as %s, expanded and folded", (count, text) => {
    const signal = renderSignal(count);
    expect(signal.getByText(text)).toBeInTheDocument();
    fireEvent.click(signal.getByRole("button", { name: "Mail" }));
    expect(signal.getByText(text)).toBeInTheDocument();
  });

  it("names the account only when one is given", () => {
    const { unmount } = render(<TodayMailSignal mail={null} error={null} onOpenThread={vi.fn()} onOpenInbox={vi.fn()} />);
    expect(document.querySelector(".today-mail-signal-scope")).not.toBeInTheDocument();
    unmount();
    renderSignal(1, "me@example.com");
    expect(document.querySelector(".today-mail-signal-scope")).toHaveTextContent("me@example.com");
  });
});
