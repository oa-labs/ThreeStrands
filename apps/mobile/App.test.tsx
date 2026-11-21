import { fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Note } from "../../src/domain";
import { App } from "./App";
import { mobile } from "./api";

const note: Note = {
  id: "library", title: "Library planning", body: "Bring drawings", dailyDate: null,
  createdAt: "2026-11-20T00:00:00Z", updatedAt: "2026-11-20T00:00:00Z", trashed: false, trashedAt: null,
};
const tabs = () => within(screen.getByRole("navigation", { name: "Workspaces" }));
const title = () => within(screen.getByRole("banner")).getByRole("heading");

beforeEach(() => {
  localStorage.clear();
  vi.spyOn(mobile, "draft").mockResolvedValue(null);
  vi.spyOn(mobile, "note").mockImplementation(async target => ({
    note: target.kind === "note" ? note : null, document: null, links: null, conflicts: [],
  }));
  vi.spyOn(mobile, "notes").mockImplementation(async (_view, query) => query
    ? [{ ...note, matchSnippet: "Project drawings" }]
    : [{ ...note, preview: "Bring drawings", visitedAt: null }]);
  vi.spyOn(mobile, "tasks").mockResolvedValue({ standalone: [], fromNotes: [] });
  vi.spyOn(mobile, "syncStatus").mockResolvedValue({
    enrolled: false, revoked: false, awaitingAdmissionFrom: null, lastSuccessAt: null, lastError: null,
  });
});
afterEach(() => {
  vi.restoreAllMocks();
  delete document.documentElement.dataset.accent;
});

describe("mobile workspace navigation", () => {
  it("restores a selected daily date, note detail, list scroll, search, and task filter", async () => {
    render(<App />);
    const today = title().textContent;
    fireEvent.click(screen.getByRole("button", { name: "Previous day" }));
    const selectedDate = title().textContent;
    expect(selectedDate).not.toEqual(today);
    expect(screen.getByRole("button", { name: "Back to Today" })).toBeVisible();

    fireEvent.click(tabs().getByRole("button", { name: "Notes" }));
    fireEvent.click(screen.getByRole("button", { name: "Starred" }));
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "drawings" } });
    const result = await screen.findByRole("button", { name: /Library planning/ });
    expect(result.querySelector("mark")).toHaveTextContent("drawings");
    expect(result).not.toHaveTextContent("**");
    const panel = screen.getByRole("main");
    panel.scrollTop = 160;
    fireEvent.scroll(panel);
    fireEvent.click(result);
    const heading = await screen.findByRole("heading", { name: note.title });
    expect(heading.closest("header")).toHaveClass("app-bar-note");

    fireEvent.click(tabs().getByRole("button", { name: "Tasks" }));
    fireEvent.click(screen.getByRole("button", { name: "Completed" }));
    await screen.findByText("No completed tasks.");
    fireEvent.click(tabs().getByRole("button", { name: "Daily" }));
    expect(title()).toHaveTextContent(selectedDate!);

    fireEvent.click(tabs().getByRole("button", { name: "Notes" }));
    await screen.findByRole("heading", { name: note.title });
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    expect(await screen.findByRole("searchbox")).toHaveValue("drawings");
    expect(screen.getByRole("button", { name: "Starred" })).toHaveAttribute("aria-pressed", "true");
    expect(panel.scrollTop).toBe(160);
    fireEvent.click(tabs().getByRole("button", { name: "Tasks" }));
    expect(screen.getByRole("button", { name: "Completed" })).toHaveAttribute("aria-pressed", "true");
  });

  it("opens Trash from Notes options and returns to the previous filter", async () => {
    render(<App />);
    fireEvent.click(tabs().getByRole("button", { name: "Notes" }));
    fireEvent.click(screen.getByRole("button", { name: "Starred" }));
    fireEvent.click(screen.getByLabelText("Notes options"));
    fireEvent.click(screen.getByRole("button", { name: "Trash" }));
    expect(title()).toHaveTextContent("Trash");
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    expect(await screen.findByRole("button", { name: "Starred" })).toHaveAttribute("aria-pressed", "true");
  });

  it("uses the shared desktop accents and lets the Daily backdrop be switched off", async () => {
    const { container } = render(<App />);
    expect(container.querySelector(".today-backdrop")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    fireEvent.change(screen.getByRole("combobox", { name: "Accent color" }), { target: { value: "blue" } });
    expect(document.documentElement).toHaveAttribute("data-accent", "blue");
    fireEvent.click(screen.getByRole("checkbox", { name: "Daily backdrop" }));
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    await screen.findByRole("button", { name: "Capture" });
    expect(container.querySelector(".today-backdrop")).toBeNull();
  });
});
