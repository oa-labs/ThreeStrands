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

// A tiny opaque PNG stands in for the native image proxy, so rendered images
// size from their declared attributes rather than placeholder styling.
const FIXTURE_IMAGE = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";
// Height-only fixtures need real intrinsic ratios: 40×40 and 80×40 source
// images displayed at 20px high, rather than widths inferred from attributes.
const HIGH_DENSITY_ICON = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAoCAYAAACM/rhtAAAACXBIWXMAAAPoAAAD6AG1e1JrAAAAYUlEQVRYhe2UoREAQRDCaBZPCXR+38B7VkTgmUxATt/laF3AFAwEO/fMjCQQ7Nw1c9SBYE9G6wKmYCDYuWdmJIFg566Zow4EezJaFzAFA8HOPTMjCQQ7d80cdSDYuW9/+QCA/o3TkqJslQAAAABJRU5ErkJggg==";
const HIGH_DENSITY_WIDE_ICON = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAFAAAAAoCAYAAABpYH0BAAAACXBIWXMAAAPoAAAD6AG1e1JrAAAAlUlEQVRoge2WsQkDQRDEvlnnKuE6l3vwg4cDBduAkNh5Phy78zODJ3jnlUABJIAuK8pAAmgGcu8SKGECaAayT7GECaBrm3oiBNC1Uc0YAuhN15AmgGYg+xRLmAC6tqknQgBdG9WMIYDedA1pAmgGsk+xhAmga5t6IgTQtVHNGALoTdeQJoBmIPsUS5gAurapJ8L/AX4BU6kbtJ6s/gIAAAAASUVORK5CYII=";
const resolveFixtureImage = async (url: string) => {
  if (url === "https://example.com/height-only.png") return HIGH_DENSITY_ICON;
  if (url === "https://example.com/height-only-wide.png") return HIGH_DENSITY_WIDE_ICON;
  return FIXTURE_IMAGE;
};

function RenderingHarness() {
  const [zoom, setZoom] = useState(Number(params.get("zoom") ?? 100));
  const [minimum, setMinimum] = useState(Number(params.get("minimumFontSize") ?? 0));
  const [tone, setTone] = useState<"default" | "current" | "muted">("default");
  return (
    <div style={{ width: "100%", maxWidth: 760, margin: "0 auto" }}>
      {params.has("zoom") ? <select aria-label="Email zoom" value={zoom} onChange={(event) => setZoom(Number(event.target.value))}>
        <option value={50}>50%</option><option value={100}>100%</option><option value={125}>125%</option><option value={150}>150%</option><option value={200}>200%</option>
      </select> : null}
      {params.has("minimumFontSize") ? <select aria-label="Minimum email font size" value={minimum} onChange={(event) => setMinimum(Number(event.target.value))}>
        <option value={0}>Off</option><option value={18}>18 px</option><option value={22}>22 px</option>
      </select> : null}
      {params.has("tone") ? <select aria-label="Message tone" value={tone} onChange={(event) => setTone(event.target.value as typeof tone)}>
        <option value="default">Unread</option><option value="muted">Read</option><option value="current">Latest</option>
      </select> : null}
      <SafeMessage html={params.has("plain") ? "" : fixture} text="Plain text message" theme={theme} loadImages={params.has("images")} resolveImage={resolveFixtureImage} emailZoom={zoom} emailMinimumFontSize={minimum} priorThreadText={priorThreadText} tone={tone} />
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<RenderingHarness />);
