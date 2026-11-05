import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { mailClient } from "./data/client";

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(() => Promise.resolve()) }));
describe("Agents workspace wiring", () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => { cleanup(); vi.restoreAllMocks(); });
  it("opens from a conversation and the navigation shortcut without sending mail or creating personal tasks", async () => {
    const createTask = vi.spyOn(mailClient, "createTask");
    render(<App />);
    fireEvent.click(await screen.findByRole("option", { name: /Welcome to ThreeStrands/ }));
    fireEvent.click(await screen.findByRole("button", { name: "Delegate Work…" }));
    await screen.findByRole("form", { name: "New delegated task" });
    expect(screen.getByText("Linked to the selected conversation.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Inbox (1)" }));
    fireEvent.keyDown(window, { key: "6" });
    await screen.findByRole("region", { name: "Agents" });
    await waitFor(() => expect(createTask).not.toHaveBeenCalled());
  });
});
