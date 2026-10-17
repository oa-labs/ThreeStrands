import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { ErrorBoundary } from "./ErrorBoundary";
import { installCrashReporter } from "./crashReporting";
import "./styles.css";
import { applyTheme, readTheme } from "./theme";
import { applyFontFamily, readFontFamily } from "./settings";

applyTheme(readTheme());
applyFontFamily(readFontFamily());

installCrashReporter();

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    {/* Last resort: each pane has its own boundary, so this catches only a failure in the app shell itself. */}
    <ErrorBoundary area="ThreeStrands" variant="app">
      <App />
    </ErrorBoundary>
  </StrictMode>,
);
