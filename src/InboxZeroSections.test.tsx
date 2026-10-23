import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import { InboxZeroSections } from "./InboxZeroSections";

// The agenda, open tasks, and Keep in touch moved to Today; their coverage is in TodayWorkspace.test.tsx.
describe("InboxZeroSections", () => {
  afterEach(() => { cleanup(); vi.restoreAllMocks(); });

  it("points to Today instead of repeating its sections", () => {
    const listTasks = vi.spyOn(mailClient, "listTasks");
    const listKeepInTouch = vi.spyOn(mailClient, "listKeepInTouch");
    const onOpenToday = vi.fn();
    render(<InboxZeroSections onOpenToday={onOpenToday} />);

    const section = screen.getByRole("region", { name: "What's next" });
    fireEvent.click(within(section).getByRole("button", { name: "Open Today" }));
    expect(onOpenToday).toHaveBeenCalled();
    expect(screen.queryByRole("region", { name: "Open tasks" })).not.toBeInTheDocument();
    expect(listTasks).not.toHaveBeenCalled();
    expect(listKeepInTouch).not.toHaveBeenCalled();
  });
});
