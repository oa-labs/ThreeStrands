import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DefaultAppsSettings } from "./DefaultAppsSettings";
import { mailClient } from "./data/client";

describe("DefaultAppsSettings", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("explains where defaults can be set when this copy cannot register", async () => {
    vi.spyOn(mailClient, "defaultAppStatus").mockResolvedValue({ supported: false, mail: false, calendar: false });
    render(<DefaultAppsSettings />);

    expect(await screen.findByText("Default apps can be set from the installed ThreeStrands app on macOS.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Make ThreeStrands the Default/ })).toBeNull();
  });

  it("shows which roles ThreeStrands already has and asks macOS for the others", async () => {
    vi.spyOn(mailClient, "defaultAppStatus").mockResolvedValue({ supported: true, mail: true, calendar: false });
    const make = vi.spyOn(mailClient, "makeDefaultApp").mockResolvedValue({ supported: true, mail: true, calendar: true });
    render(<DefaultAppsSettings />);

    expect(await screen.findByRole("status", { name: "ThreeStrands is the default for Email Links" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Make ThreeStrands the Default for Email Links" })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Make ThreeStrands the Default for Calendar Invitations" }));

    expect(make).toHaveBeenCalledWith("calendar");
    expect(await screen.findByRole("status", { name: "ThreeStrands is the default for Calendar Invitations" })).toBeInTheDocument();
  });

  it("shows a declined or failed change next to its role", async () => {
    vi.spyOn(mailClient, "defaultAppStatus").mockResolvedValue({ supported: true, mail: false, calendar: false });
    vi.spyOn(mailClient, "makeDefaultApp").mockRejectedValue(new Error("The operation couldn’t be completed."));
    render(<DefaultAppsSettings />);

    fireEvent.click(await screen.findByRole("button", { name: "Make ThreeStrands the Default for Email Links" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("The operation couldn’t be completed.");
    expect(screen.getByRole("button", { name: "Make ThreeStrands the Default for Email Links" })).toBeEnabled();
  });

  it("checks again when the window regains focus", async () => {
    const status = vi.spyOn(mailClient, "defaultAppStatus")
      .mockResolvedValueOnce({ supported: true, mail: false, calendar: false })
      .mockResolvedValue({ supported: true, mail: true, calendar: false });
    render(<DefaultAppsSettings />);
    await screen.findByRole("button", { name: "Make ThreeStrands the Default for Email Links" });

    fireEvent.focus(window);

    expect(await screen.findByRole("status", { name: "ThreeStrands is the default for Email Links" })).toBeInTheDocument();
    expect(status).toHaveBeenCalledTimes(2);
  });
});
