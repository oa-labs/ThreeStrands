import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

// Vitest globals are off, so Testing Library cannot register its own
// automatic cleanup; without this, rendered trees and hooks (and their
// intervals) from one test stay mounted into the next.
afterEach(() => {
  cleanup();
});

Object.defineProperty(HTMLCanvasElement.prototype, "getContext", {
  configurable: true,
  value: () => null,
});
