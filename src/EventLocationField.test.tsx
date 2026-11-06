import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import { EventLocationField } from "./EventLocationField";

function Harness({ initial = "", onSubmit = vi.fn(), onEscape = vi.fn() }: { initial?: string; onSubmit?: () => void; onEscape?: () => void }) {
  const [value, setValue] = useState(initial);
  return (
    <form onSubmit={(event) => { event.preventDefault(); onSubmit(); }} onKeyDown={(event) => { if (event.key === "Escape") onEscape(); }}>
      <EventLocationField value={value} onChange={setValue} />
      <output aria-label="Saved location">{value}</output>
    </form>
  );
}

const places = ["Alloy 26, 100 S Commons, Pittsburgh, PA", "Alloy 26"];

describe("EventLocationField", () => {
  afterEach(() => { cleanup(); vi.restoreAllMocks(); });

  const type = async (text: string) => {
    fireEvent.change(screen.getByRole("combobox", { name: "Location" }), { target: { value: text } });
    return screen.findByRole("listbox", { name: "Places from your calendar" });
  };

  it("offers places from the calendar and chooses one with the keyboard instead of submitting", async () => {
    const search = vi.spyOn(mailClient, "searchEventLocations").mockResolvedValue(places);
    const onSubmit = vi.fn();
    render(<Harness onSubmit={onSubmit} />);
    await type("alloy");
    expect(search).toHaveBeenCalledWith("alloy");
    const input = screen.getByRole("combobox", { name: "Location" });
    expect(input).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("option", { name: places[0] })).toHaveAttribute("aria-selected", "true");

    fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(screen.getByRole("option", { name: places[1] })).toHaveAttribute("aria-selected", "true");
    expect(input).toHaveAttribute("aria-activedescendant", screen.getByRole("option", { name: places[1] }).id);
    fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(screen.getByRole("option", { name: places[0] })).toHaveAttribute("aria-selected", "true");
    fireEvent.keyDown(input, { key: "Enter" });

    expect(input).toHaveValue(places[0]);
    expect(onSubmit).not.toHaveBeenCalled();
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  });

  it("chooses a place with the pointer", async () => {
    vi.spyOn(mailClient, "searchEventLocations").mockResolvedValue(places);
    render(<Harness />);
    await type("alloy");
    fireEvent.mouseDown(screen.getByRole("option", { name: places[1] }));
    expect(screen.getByRole("combobox", { name: "Location" })).toHaveValue("Alloy 26");
  });

  it("closes the suggestions on Escape without closing the dialog, and keeps typed text", async () => {
    vi.spyOn(mailClient, "searchEventLocations").mockResolvedValue(places);
    const onEscape = vi.fn();
    render(<Harness onEscape={onEscape} />);
    await type("alloy");
    fireEvent.keyDown(screen.getByRole("combobox", { name: "Location" }), { key: "Escape" });
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    expect(onEscape).not.toHaveBeenCalled();
    expect(screen.getByRole("combobox", { name: "Location" })).toHaveValue("alloy");
    // With nothing open, Escape belongs to the dialog again.
    fireEvent.keyDown(screen.getByRole("combobox", { name: "Location" }), { key: "Escape" });
    expect(onEscape).toHaveBeenCalledTimes(1);
  });

  it("submits typed text as-is when no suggestion is open", async () => {
    vi.spyOn(mailClient, "searchEventLocations").mockResolvedValue([]);
    const onSubmit = vi.fn();
    render(<Harness onSubmit={onSubmit} />);
    const input = screen.getByRole("combobox", { name: "Location" });
    fireEvent.change(input, { target: { value: "Somewhere new" } });
    await waitFor(() => expect(mailClient.searchEventLocations).toHaveBeenCalled());
    fireEvent.submit(input);
    expect(onSubmit).toHaveBeenCalled();
    expect(screen.getByLabelText("Saved location")).toHaveTextContent("Somewhere new");
  });

  it("does not search for the location the dialog opened with", async () => {
    const search = vi.spyOn(mailClient, "searchEventLocations").mockResolvedValue(places);
    render(<Harness initial="Alloy 26" />);
    await new Promise((resolve) => setTimeout(resolve, 400));
    expect(search).not.toHaveBeenCalled();
    expect(screen.getByRole("combobox", { name: "Location" })).toHaveAttribute("aria-expanded", "false");
  });

  it("hides a suggestion that matches the text exactly", async () => {
    vi.spyOn(mailClient, "searchEventLocations").mockResolvedValue(places);
    render(<Harness />);
    await type("alloy 26");
    expect(screen.getAllByRole("option").map((option) => option.textContent)).toEqual([places[0]]);
  });
});
