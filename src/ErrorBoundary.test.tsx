import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { reportCaughtError } from "./crashReporting";
import { ErrorBoundary } from "./ErrorBoundary";

vi.mock("./crashReporting", () => ({ reportCaughtError: vi.fn() }));

let broken = true;

function Conversation() {
  if (broken) throw new TypeError("message body is undefined");
  return <p>Quarterly planning</p>;
}

describe("ErrorBoundary", () => {
  beforeEach(() => {
    broken = true;
    vi.mocked(reportCaughtError).mockClear();
    // React logs every caught render error to the console; keep test output readable.
    vi.spyOn(console, "error").mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  it("renders a healthy area unchanged, with no element of its own", () => {
    broken = false;
    const { container } = render(<ErrorBoundary area="this conversation"><Conversation /></ErrorBoundary>);

    expect(container.innerHTML).toBe("<p>Quarterly planning</p>");
  });

  it("replaces only the failed area, leaving the rest of the window working", () => {
    render(
      <main>
        <nav>Mailboxes</nav>
        <ErrorBoundary area="this conversation"><Conversation /></ErrorBoundary>
      </main>,
    );

    expect(screen.getByRole("alert")).toHaveTextContent("Something went wrong showing this conversation.");
    expect(screen.getByText("Mailboxes")).toBeInTheDocument();
  });

  it("reports the caught error with the area and component stack", () => {
    render(<ErrorBoundary area="the inbox"><Conversation /></ErrorBoundary>);

    expect(reportCaughtError).toHaveBeenCalledTimes(1);
    const [context, error, componentStack] = vi.mocked(reportCaughtError).mock.calls[0]!;
    expect(context).toBe("Showing the inbox failed");
    expect(error).toBeInstanceOf(TypeError);
    expect(componentStack).toContain("Conversation");
  });

  it("renders the area again on Try Again", () => {
    render(<ErrorBoundary area="this conversation"><Conversation /></ErrorBoundary>);
    broken = false;
    fireEvent.click(screen.getByRole("button", { name: "Try Again" }));

    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByText("Quarterly planning")).toBeInTheDocument();
  });

  it("shows the fallback again when a retry fails the same way", () => {
    render(<ErrorBoundary area="this conversation"><Conversation /></ErrorBoundary>);
    fireEvent.click(screen.getByRole("button", { name: "Try Again" }));

    expect(screen.getByRole("alert")).toBeInTheDocument();
    expect(reportCaughtError).toHaveBeenCalledTimes(2);
  });

  it("recovers on its own when the reset key changes, such as choosing another thread", () => {
    const { rerender } = render(<ErrorBoundary area="this conversation" resetKey="thread-1"><Conversation /></ErrorBoundary>);
    broken = false;
    rerender(<ErrorBoundary area="this conversation" resetKey="thread-1"><Conversation /></ErrorBoundary>);
    expect(screen.getByRole("alert")).toBeInTheDocument();

    rerender(<ErrorBoundary area="this conversation" resetKey="thread-2"><Conversation /></ErrorBoundary>);
    expect(screen.getByText("Quarterly planning")).toBeInTheDocument();
  });

  it("offers Close only for areas that can be dismissed", () => {
    const onDismiss = vi.fn();
    const { unmount } = render(<ErrorBoundary area="the inbox"><Conversation /></ErrorBoundary>);
    expect(screen.queryByRole("button", { name: "Close" })).not.toBeInTheDocument();
    unmount();

    render(<ErrorBoundary area="Settings" variant="floating" onDismiss={onDismiss}><Conversation /></ErrorBoundary>);
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });

  it("offers a reload instead of a retry when the whole window failed", () => {
    render(<ErrorBoundary area="ThreeStrands" variant="app"><Conversation /></ErrorBoundary>);

    expect(screen.getByRole("alert")).toHaveClass("pane-error", "app");
    expect(screen.getByRole("button", { name: "Reload ThreeStrands" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Try Again" })).not.toBeInTheDocument();
  });

  it("styles each variant from the shared fallback recipe", () => {
    const css = readFileSync(resolve(process.cwd(), "src/styles.css"), "utf8");
    for (const variant of ["side", "floating", "app"]) expect(css, variant).toContain(`.pane-error.${variant} {`);
  });
});

describe("app error boundaries", () => {
  const app = readFileSync(resolve(process.cwd(), "src/App.tsx"), "utf8");
  const main = readFileSync(resolve(process.cwd(), "src/main.tsx"), "utf8");

  /** The source between a boundary's opening tag for `area` and its closing tag. */
  function boundaryAround(area: string): string {
    const start = app.indexOf(`<ErrorBoundary area="${area}"`);
    expect(start, area).toBeGreaterThan(-1);
    return app.slice(start, app.indexOf("</ErrorBoundary>", start));
  }

  it("gives every main area its own boundary so one failure cannot blank the window", () => {
    expect(boundaryAround("the inbox")).toContain("<ThreadList");
    expect(boundaryAround("this conversation")).toContain("renderComposer");
    expect(boundaryAround("the context panel")).toContain("<ContextPanel");
    expect(boundaryAround("this draft's context")).toContain("<ComposeContext");
    expect(boundaryAround("Tasks")).toContain("<TaskSidebar");
    expect(boundaryAround("Files")).toContain("<FilesWorkspace");
    expect(boundaryAround("Contacts")).toContain("<ContactsWorkspace");
    expect(boundaryAround("Settings")).toContain("<Settings");
    expect(app).toMatch(/<ErrorBoundary area="the calendar" variant="side">\s*<Suspense fallback=\{null\}>\s*<CalendarWeekView/);
    expect(app).toMatch(/<ErrorBoundary area="the calendar" variant="side">\s*<CalendarSidebar/);
  });

  it("keeps a last-resort boundary around the whole app", () => {
    expect(main).toMatch(/<ErrorBoundary area="ThreeStrands" variant="app">\s*<App \/>\s*<\/ErrorBoundary>/);
  });
});
