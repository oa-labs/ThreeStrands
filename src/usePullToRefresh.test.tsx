import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useRef } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { usePullToRefresh } from "./usePullToRefresh";
import { PullToRefreshStatus } from "./PullToRefreshStatus";

function Surface({ refresh }: { refresh: () => Promise<void> }) {
  const target = useRef<HTMLDivElement>(null);
  const pull = usePullToRefresh(target, refresh);
  return <div aria-label="Scroll pane" style={{ overflowY: "auto" }}>
    <div ref={target} role="region" aria-label="Agenda" aria-busy={pull.refreshing}><PullToRefreshStatus {...pull} /></div>
  </div>;
}

afterEach(() => { cleanup(); vi.restoreAllMocks(); });

describe("pull refresh feedback", () => {
  it("keeps progress visible until completion and exposes failures without clearing the content", async () => {
    let reject!: (reason: Error) => void;
    vi.spyOn(console, "warn").mockImplementation(() => {});
    const refresh = vi.fn(() => new Promise<void>((_resolve, fail) => { reject = fail; }));
    render(<Surface refresh={refresh} />);
    const region = screen.getByRole("region", { name: "Agenda" });
    fireEvent.wheel(region, { deltaY: -100 });
    expect(screen.getByRole("status")).toHaveTextContent("Refreshing…");
    expect(region).toHaveAttribute("aria-busy", "true");
    fireEvent.wheel(region, { deltaY: -100 });
    expect(refresh).toHaveBeenCalledTimes(1);
    await act(async () => { reject(new Error("Offline")); });
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
    expect(region).toHaveAttribute("aria-busy", "false");
    expect(screen.getByRole("alert")).toHaveTextContent("Refresh failed: Offline");
  });

  it("uses an agenda's containing pane scroll position", async () => {
    const refresh = vi.fn(async () => {});
    render(<Surface refresh={refresh} />);
    const region = screen.getByRole("region", { name: "Agenda" });
    region.parentElement!.scrollTop = 100;
    fireEvent.wheel(region, { deltaY: -100 });
    await act(async () => {});
    expect(refresh).not.toHaveBeenCalled();
  });

  it("does not carry old account progress or completion into the new account", async () => {
    let finish!: () => void;
    const first = vi.fn(() => new Promise<void>((resolve) => { finish = resolve; }));
    const second = vi.fn(async () => {});
    const { rerender } = render(<Surface refresh={first} />);
    fireEvent.wheel(screen.getByRole("region"), { deltaY: -100 });
    rerender(<Surface refresh={second} />);
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
    await act(async () => { finish(); fireEvent.wheel(screen.getByRole("region"), { deltaY: -100 }); });
    expect(second).toHaveBeenCalledTimes(1);
  });
});
