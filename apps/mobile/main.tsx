import { createRoot } from "react-dom/client";
import "@fontsource-variable/inter";
import "@fontsource-variable/fraunces";
import { applyAccent, readAccent } from "../../src/accent";
import { applyTheme } from "../../src/theme";
import { App } from "./App";
import "./styles.css";

const syncSystemTheme = () => applyTheme("system");
syncSystemTheme();
window.matchMedia("(prefers-color-scheme: light)").addEventListener("change", syncSystemTheme);
applyAccent(readAccent());

createRoot(document.getElementById("root")!).render(<App />);
