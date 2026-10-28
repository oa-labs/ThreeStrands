import "@testing-library/jest-dom/vitest";
import { cleanup, configure } from "@testing-library/react";
import { afterEach } from "vitest";

// The full App mounts lazily and does a lot on first render; on a busy CI
// runner its first screen can take longer than Testing Library's 1 s default
// wait. A longer wait only slows tests that are already failing.
configure({ asyncUtilTimeout: 5_000 });

// Vitest globals are off, so Testing Library cannot register its own
// automatic cleanup; without this, rendered trees and hooks (and their
// intervals) from one test stay mounted into the next. The app also reopens
// the last workspace it showed, so each test starts back in Mail, and reopens
// conversations where they were left, so each test starts at the latest message.
afterEach(() => {
  cleanup();
  localStorage.removeItem("threestrands.workspace.last");
  localStorage.removeItem("threestrands.workspace.sideColumn");
  localStorage.removeItem("threestrands.readingPositions");
});

Object.defineProperty(HTMLCanvasElement.prototype, "getContext", {
  configurable: true,
  value: () => null,
});
