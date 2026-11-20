import { act, renderHook } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { useCommandUndo } from "./useCommandExecution";
import { commands } from "./commands";

it("an addition to a note becomes the normal Undo target and keeps its Open Note action", async () => {
  const notice = vi.fn(), previousUndo = vi.fn(async () => undefined), captureUndo = vi.fn(async () => undefined);
  const action = { label: "Open Note", run: vi.fn() };
  const { result } = renderHook(() => useCommandUndo(notice));
  act(() => { result.current.rememberUndo(commands.find((c) => c.id === "thread.archive")!, { undoAction: previousUndo, undoKind: "archive" }); });
  act(() => { result.current.rememberUndo(commands.find((c) => c.id === "notes.add")!, { message: "Added to Prep", action, undoAction: captureUndo }); });
  expect(notice).toHaveBeenLastCalledWith(expect.objectContaining({ message: "Added to Prep", action, undo: expect.any(Function) }));
  expect(result.current.canUndoAction).toBe(true);
  expect(result.current.canUndoArchive).toBe(false);
  await act(async () => result.current.undoLastAction());
  expect(captureUndo).toHaveBeenCalledOnce();
  expect(previousUndo).not.toHaveBeenCalled();
});
