import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import postcss from "postcss";
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

  it("keeps long structured values in a full-width, wrapping conflict card", () => {
    const longValue = { aiFeatures: Object.fromEntries(Array.from({ length: 20 }, (_, index) => [`feature${index}`, true])) };
    const preferencesConflict: FrontierConflict = {
      ...conflict,
      entityType: "preferences",
      field: "aiFeatures",
      candidates: [{ ...conflict.candidates[0], value: longValue }],
    };
    const { container } = render(<FrontierConflictEditor conflict={preferencesConflict} disabled={false} onResolve={vi.fn()} />);
    const card = container.querySelector(".frontier-conflict-editor");
    expect(card).toHaveTextContent(JSON.stringify(longValue));
    expect(screen.getByRole("radio").closest("label")?.querySelector("span")).toHaveTextContent(JSON.stringify(longValue));

    // jsdom does not calculate widths, so guard the CSS rules that prevent
    // fieldset intrinsic sizing and unbroken JSON from widening the panel.
    const css = postcss.parse(readFileSync(resolve(process.cwd(), "src/styles.css"), "utf8"));
    const declaration = (selector: string, property: string) => {
      let value: string | undefined;
      css.walkRules(selector, (rule) => {
        if (rule.selector === selector) rule.walkDecls(property, (entry) => { value = entry.value; });
      });
      return value;
    };
    expect(declaration(".frontier-conflict-editor", "flex-direction")).toBe("column");
    expect(declaration(".frontier-conflict-editor .settings-field", "min-width")).toBe("0");
    expect(declaration(".frontier-conflict-editor strong, .frontier-conflict-editor label span", "overflow-wrap")).toBe("anywhere");
    expect(declaration(".frontier-conflict-editor input[type=\"radio\"]", "width")).toBe("auto");
  });
});
