import { useState } from "react";
import { createRoot } from "react-dom/client";
import { SafeMessage } from "../src/SafeMessage";
import { emailRenderingFixtures } from "../src/test/emailRenderingFixtures";
import { threadTextIndex } from "../src/threadTextIndex";
import "../src/styles.css";

const params = new URLSearchParams(window.location.search);
const theme = params.get("theme") === "light" ? "light" : "dark";
const fixtureName = params.get("fixture");
if (!fixtureName || !Object.hasOwn(emailRenderingFixtures, fixtureName)) {
  throw new Error(`Unknown email rendering fixture: ${fixtureName ?? "(missing)"}`);
}
const fixture = emailRenderingFixtures[fixtureName as keyof typeof emailRenderingFixtures];
// `prior` names a fixture to index as the conversation's earlier message.
const priorName = params.get("prior");
if (priorName && !Object.hasOwn(emailRenderingFixtures, priorName)) {
  throw new Error(`Unknown prior message fixture: ${priorName}`);
}
const priorThreadText = priorName
  ? threadTextIndex([{ bodyText: "", bodyHtml: emailRenderingFixtures[priorName as keyof typeof emailRenderingFixtures] }]).before(1)
  : undefined;

document.documentElement.dataset.theme = theme;
document.body.style.margin = "0";
document.body.style.minWidth = "0";
document.body.style.overflow = "auto";
document.body.style.padding = "24px";
document.body.style.background = theme === "light" ? "#fff" : "#17171c";
document.body.style.color = theme === "light" ? "#24242c" : "#e8e8eb";


function RenderingHarness() {
  const [minimum, setMinimum] = useState(Number(params.get("minimumFontSize") ?? 0));
  return (
    <div style={{ width: "100%", maxWidth: 760, margin: "0 auto" }}>
      {params.has("minimumFontSize") ? <select aria-label="Minimum email font size" value={minimum} onChange={(event) => setMinimum(Number(event.target.value))}>
        <option value={0}>Off</option><option value={18}>18 px</option><option value={22}>22 px</option>
      </select> : null}
      <SafeMessage html={params.has("plain") ? "" : fixture} text="Plain text message" theme={theme} loadImages={false} emailMinimumFontSize={minimum} priorThreadText={priorThreadText} />
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<RenderingHarness />);
