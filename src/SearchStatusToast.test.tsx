import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SearchStatusToast } from "./SearchStatusToast";
import { SERVER_SEARCH_FAILED_COPY, SERVER_SEARCHING_COPY } from "./searchStatusCopy";

afterEach(cleanup);

describe("SearchStatusToast", () => {
  it("shows when the server search is still running", () => {
    render(<SearchStatusToast remoteSearchState="searching" status={null} onDismiss={vi.fn()} />);
    expect(screen.getByRole("status")).toHaveTextContent(SERVER_SEARCHING_COPY);
  });

  it("reports when server search cannot be reached", () => {
    render(<SearchStatusToast remoteSearchState="error" status={null} onDismiss={vi.fn()} />);
    expect(screen.getByText(SERVER_SEARCH_FAILED_COPY)).toBeVisible();
  });

  it("shows the search outcome with a screen-reader status role", () => {
    render(<SearchStatusToast remoteSearchState="idle" status="Some folders took too long to search. Showing what was found." onDismiss={vi.fn()} />);
    const region = screen.getByText("Some folders took too long to search. Showing what was found.").closest("[role=status]");
    expect(region).not.toBeNull();
    expect(region).toHaveAttribute("aria-live", "polite");
  });

  it("keeps the spinner up rather than an earlier outcome while a search runs", () => {
    render(<SearchStatusToast remoteSearchState="searching" status="Earlier outcome" onDismiss={vi.fn()} />);
    expect(screen.queryByText("Earlier outcome")).not.toBeInTheDocument();
  });

  it("shows nothing for a search with nothing to say", () => {
    const { container } = render(<SearchStatusToast remoteSearchState="idle" status={null} onDismiss={vi.fn()} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("is dismissible", () => {
    const onDismiss = vi.fn();
    render(<SearchStatusToast remoteSearchState="idle" status="Some folders took too long to search." onDismiss={onDismiss} />);
    fireEvent.click(within(screen.getByRole("status")).getByRole("button", { name: "Dismiss" }));
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });
});
