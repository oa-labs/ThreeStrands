import "./legal";
import { searchDocs, searchTerms, type SearchEntry } from "./docsSearch";

function highlighted(text: string, query: string): DocumentFragment {
  const fragment = document.createDocumentFragment();
  const terms = searchTerms(query).flat();
  if (!terms.length) { fragment.append(text); return fragment; }
  // Terms contain letters and numbers only, so cannot introduce regex syntax.
  const matcher = new RegExp(`(${[...new Set(terms)].join("|")})`, "giu");
  let cursor = 0;
  for (const match of text.matchAll(matcher)) {
    fragment.append(text.slice(cursor, match.index));
    const mark = document.createElement("mark");
    mark.textContent = match[0];
    fragment.append(mark);
    cursor = match.index + match[0].length;
  }
  fragment.append(text.slice(cursor));
  return fragment;
}

const input = document.querySelector<HTMLInputElement>("#docs-query");
const results = document.querySelector<HTMLUListElement>("#docs-results");
const status = document.querySelector<HTMLElement>("#docs-search-status");
const index = document.querySelector("#docs-search-index");
if (input && results && status && index) {
  const entries = JSON.parse(index.textContent ?? "[]") as SearchEntry[];
  const update = () => {
    const query = input.value.trim();
    results.replaceChildren();
    const matches = searchDocs(entries, query);
    results.hidden = !query;
    status.textContent = !query ? "" : matches.length ? `${matches.length} results. Use Tab or Down Arrow to browse.` : "No results. Try a feature name or symptom, or browse Troubleshooting and Get Help.";
    for (const match of matches) {
      const item = document.createElement("li");
      const link = document.createElement("a");
      link.href = match.href;
      link.append(highlighted(`${match.title} — ${match.section}`, query));
      const snippet = document.createElement("p");
      snippet.append(highlighted(match.snippet, query));
      item.append(link, snippet);
      results.append(item);
    }
    if (query && !matches.length) {
      const item = document.createElement("li");
      const link = document.createElement("a");
      link.href = "troubleshooting.html";
      link.textContent = "Browse Troubleshooting";
      item.append(link);
      results.append(item);
    }
  };
  input.addEventListener("input", update);
  const navigate = (event: KeyboardEvent) => {
    const links = [...results.querySelectorAll<HTMLAnchorElement>("a")];
    const active = links.indexOf(document.activeElement as HTMLAnchorElement);
    if (event.key === "ArrowDown") {
      event.preventDefault();
      links[Math.min(active + 1, links.length - 1)]?.focus();
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      if (active <= 0) input.focus(); else links[active - 1]?.focus();
    } else if (event.key === "Escape") {
      event.preventDefault();
      input.value = "";
      update();
      input.focus();
    } else if (event.key === "Enter" && event.target === input && links[0]) {
      event.preventDefault();
      links[0].click();
    }
  };
  input.addEventListener("keydown", navigate);
  results.addEventListener("keydown", navigate);
}

// Native details keep all navigation available when JavaScript is disabled.
const browse = document.querySelector<HTMLDetailsElement>(".docs-browse");
const narrow = window.matchMedia("(max-width: 900px)");
const setBrowse = () => { if (browse) browse.open = !narrow.matches; };
setBrowse();
narrow.addEventListener("change", setBrowse);

for (const heading of document.querySelectorAll<HTMLElement>(".docs-body h2[id], .docs-body h3[id]")) {
  const link = document.createElement("a");
  link.className = "docs-permalink";
  link.href = `#${heading.id}`;
  link.setAttribute("aria-label", `Link to ${heading.textContent}`);
  link.textContent = "#";
  const copy = document.createElement("button");
  copy.type = "button";
  copy.className = "docs-copy-link";
  copy.textContent = "Copy link";
  copy.setAttribute("aria-label", `Copy link to ${heading.textContent}`);
  copy.addEventListener("click", async () => {
    const url = new URL(link.href);
    try {
      await navigator.clipboard.writeText(url.href);
      copy.textContent = "Copied";
    } catch {
      // The ordinary permalink remains usable on insecure/local contexts.
      copy.textContent = "Use # link";
    }
  });
  heading.append(" ", link, " ", copy);
}

const dialog = document.createElement("dialog");
dialog.className = "docs-image-dialog";
dialog.setAttribute("aria-label", "Enlarged screenshot");
const close = document.createElement("button");
close.type = "button";
close.textContent = "Close screenshot";
close.addEventListener("click", () => dialog.close());
const enlarged = document.createElement("img");
dialog.append(close, enlarged);
document.body.append(dialog);
let imageTrigger: HTMLButtonElement | undefined;
dialog.addEventListener("close", () => imageTrigger?.focus());
for (const picture of document.querySelectorAll<HTMLPictureElement>(".docs-body picture")) {
  const original = picture.querySelector("img");
  if (!original) continue;
  const button = document.createElement("button");
  button.type = "button";
  button.className = "docs-enlarge";
  button.textContent = "Enlarge screenshot";
  button.setAttribute("aria-label", `Enlarge screenshot: ${original.alt}`);
  picture.after(button);
  button.addEventListener("click", () => {
    imageTrigger = button;
    enlarged.src = original.currentSrc || original.src;
    enlarged.alt = original.alt;
    dialog.showModal();
  });
}

