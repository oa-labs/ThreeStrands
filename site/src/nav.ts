/** Gives the fixed nav a backdrop once the page scrolls under it. */
export function initNav() {
  const nav = document.querySelector<HTMLElement>("[data-nav]");
  if (!nav) return;
  const update = () => nav.toggleAttribute("data-scrolled", window.scrollY > 24);
  update();
  window.addEventListener("scroll", update, { passive: true });
}
