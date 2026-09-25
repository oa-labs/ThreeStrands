/** Crossfades the sticky tour window to whichever step sits in the middle of the viewport. */
export function initTour() {
  const list = document.querySelector<HTMLElement>("[data-tour-steps]");
  const frames = document.querySelector<HTMLElement>("[data-tour-frames]");
  const progress = document.querySelector<HTMLElement>("[data-tour-progress]");
  if (!list || !frames || !("IntersectionObserver" in window)) return;
  const steps = [...list.querySelectorAll<HTMLElement>("[data-step]")];
  const pictures = new Map(
    [...frames.querySelectorAll<HTMLElement>("[data-scene]")].map((picture) => [picture.dataset.scene!, picture]),
  );

  const activate = (step: HTMLElement) => {
    const scene = step.dataset.step!;
    for (const candidate of steps) candidate.toggleAttribute("data-active", candidate === step);
    for (const [name, picture] of pictures) picture.toggleAttribute("data-active", name === scene);
    progress?.style.setProperty("--progress", String((steps.indexOf(step) + 1) / steps.length));
  };

  activate(steps[0]!);
  list.dataset.ready = "";
  const observer = new IntersectionObserver(
    (entries) => {
      const entering = entries.filter((entry) => entry.isIntersecting);
      if (entering.length > 0) activate(entering.at(-1)!.target as HTMLElement);
    },
    { rootMargin: "-45% 0px -45% 0px" },
  );
  for (const step of steps) observer.observe(step);
}
