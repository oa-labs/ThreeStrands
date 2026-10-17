import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ModelCombobox } from "./ModelCombobox";
import type { AiModelInfo } from "./aiSettings";

const MODELS: AiModelInfo[] = [
  { id: "gpt-4o", label: null },
  { id: "gpt-4o-mini", label: null },
  { id: "anthropic/claude-3.5-sonnet", label: "Claude 3.5 Sonnet" },
];

afterEach(cleanup);

describe("ModelCombobox", () => {
  it("filters the list as the user types and selects an option", () => {
    const onCommit = vi.fn();
    render(<ModelCombobox value="" models={MODELS} ariaLabel="Reasoning model" onCommit={onCommit} />);
    const input = screen.getByRole("combobox", { name: "Reasoning model" });

    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: "mini" } });
    const options = screen.getAllByRole("option");
    expect(options).toHaveLength(1);
    expect(options[0]).toHaveTextContent("gpt-4o-mini");

    fireEvent.mouseDown(options[0]);
    expect(onCommit).toHaveBeenCalledWith("gpt-4o-mini");
  });

  it("matches on the human label as well as the id", () => {
    render(<ModelCombobox value="" models={MODELS} ariaLabel="Reasoning model" onCommit={vi.fn()} />);
    const input = screen.getByRole("combobox", { name: "Reasoning model" });
    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: "sonnet" } });
    expect(screen.getAllByRole("option")).toHaveLength(1);
    expect(screen.getByRole("option")).toHaveTextContent("anthropic/claude-3.5-sonnet");
  });

  it("commits a free-typed value the list does not contain on blur", () => {
    const onCommit = vi.fn();
    render(<ModelCombobox value="" models={MODELS} ariaLabel="Reasoning model" onCommit={onCommit} />);
    const input = screen.getByRole("combobox", { name: "Reasoning model" });
    fireEvent.change(input, { target: { value: "  my-local-model  " } });
    fireEvent.blur(input);
    expect(onCommit).toHaveBeenCalledWith("my-local-model");
  });

  it("keeps and commits typed text when the user clicks outside the open list", () => {
    // A real click outside fires pointerdown (which closes the list) before the
    // input's blur, so closing must not reset the draft that blur commits.
    const onCommit = vi.fn();
    render(<ModelCombobox value="gpt-4o" models={MODELS} ariaLabel="Reasoning model" onCommit={onCommit} />);
    const input = screen.getByRole("combobox", { name: "Reasoning model" });
    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: "my-local-model" } });
    expect(input).toHaveAttribute("aria-expanded", "true");

    fireEvent.pointerDown(document.body);
    expect(input).toHaveAttribute("aria-expanded", "false");
    expect(input).toHaveValue("my-local-model");

    fireEvent.blur(input);
    expect(onCommit).toHaveBeenCalledWith("my-local-model");
  });

  it("reverts the draft to the saved value on Escape without committing", () => {
    const onCommit = vi.fn();
    render(<ModelCombobox value="gpt-4o" models={MODELS} ariaLabel="Reasoning model" onCommit={onCommit} />);
    const input = screen.getByRole("combobox", { name: "Reasoning model" });
    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: "gpt-4o-m" } });
    fireEvent.keyDown(input, { key: "Escape" });
    expect(input).toHaveValue("gpt-4o");
    fireEvent.blur(input);
    expect(onCommit).not.toHaveBeenCalled();
  });

  it("follows a saved value changed from outside", () => {
    const { rerender } = render(
      <ModelCombobox value="gpt-4o" models={MODELS} ariaLabel="Reasoning model" onCommit={vi.fn()} />,
    );
    rerender(<ModelCombobox value="claude-sonnet-5" models={MODELS} ariaLabel="Reasoning model" onCommit={vi.fn()} />);
    expect(screen.getByRole("combobox", { name: "Reasoning model" })).toHaveValue("claude-sonnet-5");
  });

  it("turns off macOS inline predictions so the first Escape reaches the list", () => {
    render(<ModelCombobox value="" models={MODELS} ariaLabel="Reasoning model" onCommit={vi.fn()} />);
    const input = screen.getByRole("combobox", { name: "Reasoning model" });
    expect(input).toHaveAttribute("writingsuggestions", "false");
    expect(input).toHaveAttribute("autocorrect", "off");
    expect(input).toHaveAttribute("autocapitalize", "off");
    expect(input).toHaveAttribute("spellcheck", "false");
  });

  it("navigates with the arrow keys and selects with Enter", () => {
    const onCommit = vi.fn();
    render(<ModelCombobox value="" models={MODELS} ariaLabel="Reasoning model" onCommit={onCommit} />);
    const input = screen.getByRole("combobox", { name: "Reasoning model" });
    fireEvent.focus(input);
    fireEvent.keyDown(input, { key: "ArrowDown" }); // highlight first
    fireEvent.keyDown(input, { key: "ArrowDown" }); // highlight second
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onCommit).toHaveBeenCalledWith("gpt-4o-mini");
  });

  it("does not commit when the value is unchanged", () => {
    const onCommit = vi.fn();
    render(<ModelCombobox value="gpt-4o" models={MODELS} ariaLabel="Reasoning model" onCommit={onCommit} />);
    const input = screen.getByRole("combobox", { name: "Reasoning model" });
    fireEvent.focus(input);
    fireEvent.blur(input);
    expect(onCommit).not.toHaveBeenCalled();
  });
});
