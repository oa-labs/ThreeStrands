import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

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
