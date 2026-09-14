import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { installCrashReporter } from "./crashReporting";
import "./styles.css";
import { applyTheme, readTheme } from "./theme";
import { applyFontFamily, readFontFamily } from "./settings";

applyTheme(readTheme());
applyFontFamily(readFontFamily());

installCrashReporter();

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
