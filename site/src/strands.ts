/**
 * Browsers with CSS scroll-driven animations draw the strands in CSS alone.
 * Everywhere else, reveal each drawing once it scrolls into view.
 */
export function initStrands() {
  if (CSS.supports("animation-timeline: view()") || !("IntersectionObserver" in window)) return;
  const observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        entry.target.classList.add("is-drawn");
        observer.unobserve(entry.target);
      }
    },
    { threshold: 0.3 },
  );
  for (const scope of document.querySelectorAll(".bridge, .finale-mark")) observer.observe(scope);
}
