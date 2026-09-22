import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { FrontierConflictEditor, type FrontierConflict } from "./FrontierConflictEditor";

const conflict: FrontierConflict = {
  entityType: "task",
  entityId: "task-1",
  field: "title",
  candidates: [
    { operationId: "op-a", deviceId: "device-aaaaaaaa", value: "From laptop" },
    { operationId: "op-b", deviceId: "device-bbbbbbbb", value: "From phone" },
    { operationId: "op-c", deviceId: "device-cccccccc", value: "From tablet" },
  ],
};

describe("FrontierConflictEditor", () => {
  afterEach(cleanup);

  it("renders one choice per frontier candidate, not a fixed binary choice", () => {
    render(<FrontierConflictEditor conflict={conflict} disabled={false} onResolve={vi.fn()} />);
    expect(screen.getAllByRole("radio")).toHaveLength(3);
    expect(screen.getByText(/From laptop/)).toBeInTheDocument();
    expect(screen.getByText(/From phone/)).toBeInTheDocument();
    expect(screen.getByText(/From tablet/)).toBeInTheDocument();
  });

  it("resolves with the selected candidate, defaulting to the first", () => {
    const onResolve = vi.fn();
    render(<FrontierConflictEditor conflict={conflict} disabled={false} onResolve={onResolve} />);

    fireEvent.click(screen.getByRole("button", { name: "Resolve conflict" }));
    expect(onResolve).toHaveBeenCalledWith(conflict.candidates[0]);

    onResolve.mockClear();
    const radios = screen.getAllByRole("radio");
    fireEvent.click(radios[2]);
    fireEvent.click(screen.getByRole("button", { name: "Resolve conflict" }));
    expect(onResolve).toHaveBeenCalledWith(conflict.candidates[2]);
  });

  it("disables the resolve action while disabled", () => {
    render(<FrontierConflictEditor conflict={conflict} disabled onResolve={vi.fn()} />);
    expect(screen.getByRole("button", { name: "Resolve conflict" })).toBeDisabled();
  });

  it("handles a conflict-free frontier of exactly one candidate", () => {
    const onResolve = vi.fn();
    const single: FrontierConflict = {
      ...conflict,
      candidates: [conflict.candidates[0]],
    };
    render(<FrontierConflictEditor conflict={single} disabled={false} onResolve={onResolve} />);
    expect(screen.getAllByRole("radio")).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: "Resolve conflict" }));
    expect(onResolve).toHaveBeenCalledWith(single.candidates[0]);
  });
});
