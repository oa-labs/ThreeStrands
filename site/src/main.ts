import "@fontsource-variable/inter";
import "@fontsource-variable/fraunces/full.css";
import "@fontsource-variable/fraunces/full-italic.css";
import "./styles.css";
import { initKeyboardDemo } from "./keys";
import { initNav } from "./nav";
import { initStrands } from "./strands";
import { initTour } from "./tour";

const isMac = /Mac|iPhone|iPad/.test(navigator.platform) || navigator.userAgent.includes("Mac OS");

/** Shortcut hints say ⌘ on Apple platforms and Ctrl everywhere else, matching the app. */
function localizeModifierKeys() {
  if (isMac) return;
  for (const element of document.querySelectorAll<HTMLElement>("[data-mod-key]")) {
    element.textContent = "Ctrl K";
    const label = element.getAttribute("aria-label");
    if (label) element.setAttribute("aria-label", label.replace("⌘K", "Ctrl K"));
  }
  for (const element of document.querySelectorAll<HTMLElement>("[data-mod-enter]")) {
    element.textContent = "Ctrl ↩";
  }
}

function initHeroThemeSwitch() {
  const window_ = document.querySelector<HTMLElement>("[data-hero-window]");
  const group = document.querySelector<HTMLElement>("[data-theme-switch]");
  if (!window_ || !group) return;
  group.hidden = false;
  const buttons = [...group.querySelectorAll<HTMLButtonElement>("button[data-value]")];
  for (const button of buttons) {
    button.addEventListener("click", () => {
      const value = button.dataset.value!;
      window_.dataset.themeShot = value;
      for (const candidate of buttons) candidate.setAttribute("aria-pressed", String(candidate === button));
    });
  }
}

localizeModifierKeys();
initNav();
initHeroThemeSwitch();
initStrands();
initTour();
initKeyboardDemo(isMac);
