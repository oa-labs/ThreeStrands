import { createRoot } from "react-dom/client";
import { SafeMessage } from "../src/SafeMessage";
import { emailRenderingFixtures } from "../src/test/emailRenderingFixtures";
import "../src/styles.css";

const params = new URLSearchParams(window.location.search);
const theme = params.get("theme") === "light" ? "light" : "dark";
const fixtureName = params.get("fixture");
if (!fixtureName || !Object.hasOwn(emailRenderingFixtures, fixtureName)) {
  throw new Error(`Unknown email rendering fixture: ${fixtureName ?? "(missing)"}`);
}
const fixture = emailRenderingFixtures[fixtureName as keyof typeof emailRenderingFixtures];

document.documentElement.dataset.theme = theme;
document.body.style.margin = "0";
document.body.style.minWidth = "0";
document.body.style.overflow = "auto";
document.body.style.padding = "24px";
document.body.style.background = theme === "light" ? "#fff" : "#17171c";
document.body.style.color = theme === "light" ? "#24242c" : "#e8e8eb";

createRoot(document.getElementById("root")!).render(
  <div style={{ width: "100%", maxWidth: 760, margin: "0 auto" }}>
    <SafeMessage html={fixture} theme={theme} loadImages={false} />
  </div>,
);
