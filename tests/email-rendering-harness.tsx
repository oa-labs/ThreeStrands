import { createRoot } from "react-dom/client";
import { SafeMessage } from "../src/SafeMessage";
import { emailRenderingFixtures } from "../src/test/emailRenderingFixtures";
import "../src/styles.css";

const params = new URLSearchParams(window.location.search);
const theme = params.get("theme") === "light" ? "light" : "dark";
const fixture = params.get("fixture") === "transactional" ? emailRenderingFixtures.transactional : emailRenderingFixtures.notification;

document.documentElement.dataset.theme = theme;
document.body.style.margin = "0";
document.body.style.padding = "24px";
document.body.style.background = theme === "light" ? "#fff" : "#17171c";
document.body.style.color = theme === "light" ? "#24242c" : "#e8e8eb";

createRoot(document.getElementById("root")!).render(
  <div style={{ maxWidth: 760, margin: "0 auto" }}>
    <SafeMessage html={fixture} theme={theme} loadImages={false} />
  </div>,
);
