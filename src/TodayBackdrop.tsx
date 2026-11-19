import { useId } from "react";
import { skyPhase, sunHeight } from "./backdropPreference";

/** The scene's own coordinate space, scaled to the header's width and height. */
const WIDTH = 1200;
const HEIGHT = 200;
/** Where the sun sits on the horizon at its lowest, and how far it climbs by noon. */
const HORIZON_Y = HEIGHT * 0.95;
const SUN_CLIMB = HEIGHT * 0.7;
const SUN_X = WIDTH * 0.86;
const SUN_RADIUS = 26;
/** The moon has its own square viewport, so the wide sky's vertical crop cannot cut it off. */
const MOON = { x: 32, y: 32, radius: 24 };
/** A few fixed stars at dusk, as shares of the scene's width and height; the same every evening. */
const DUSK_STARS = [[0.6, 0.55], [0.68, 0.28], [0.79, 0.16], [0.93, 0.62], [0.97, 0.22]] as const;
/** The late-night sky keeps those and adds more, still at fixed places. */
const NIGHT_STARS = [...DUSK_STARS, [0.52, 0.2], [0.56, 0.78], [0.64, 0.12], [0.73, 0.7], [0.76, 0.42], [0.83, 0.82], [0.9, 0.12], [0.95, 0.45], [0.99, 0.8], [0.46, 0.5], [0.42, 0.18], [0.7, 0.9]] as const;

/**
 * Today's optional sky art, inside the header band: between sunrise and
 * sunset a daytime sky whose sun stands higher toward noon; a twilight sky
 * with a moon and a few stars around it; and a dark late-night sky with more
 * stars and a brighter moon from 9 pm to 5 am.
 * Original art drawn in code from theme tokens, hidden from assistive
 * technology, with no motion beyond a soft fade when the view switches that
 * reduced motion turns off.
 */
export function TodayBackdrop({ now }: { now: Date }) {
  const moonCutoutId = useId();
  const phase = skyPhase(now);
  const stars = phase === "night" ? NIGHT_STARS : DUSK_STARS;
  return <div className="today-backdrop" data-phase={phase} aria-hidden="true">
    <svg viewBox={`0 0 ${WIDTH} ${HEIGHT}`} preserveAspectRatio="xMaxYMid slice" focusable="false">
      {phase === "day" ? <>
        <ellipse className="today-backdrop-cloud" cx={WIDTH * 0.62} cy={HEIGHT * 0.3} rx={90} ry={16} />
        <ellipse className="today-backdrop-cloud" cx={WIDTH * 0.68} cy={HEIGHT * 0.38} rx={60} ry={12} />
        <ellipse className="today-backdrop-cloud" cx={WIDTH * 0.95} cy={HEIGHT * 0.75} rx={70} ry={12} />
        <circle className="today-backdrop-sun" data-height={sunHeight(now).toFixed(2)} cx={SUN_X} cy={HORIZON_Y - SUN_CLIMB * sunHeight(now)} r={SUN_RADIUS} />
      </> : <>
        {stars.map(([x, y]) => <circle key={`${x}:${y}`} className="today-backdrop-star" cx={WIDTH * x} cy={HEIGHT * y} r={2} />)}
      </>}
    </svg>
    {phase !== "day" ? <svg className="today-backdrop-moon-scene" viewBox="0 0 64 64" preserveAspectRatio="xMidYMid meet" focusable="false">
      <defs>
        <clipPath id={moonCutoutId} clipPathUnits="userSpaceOnUse">
          {/* Subtract the offset circle from the viewport so the real sky shows through. */}
          <path clipRule="evenodd" d="M0 0H64V64H0Z M44 4a22 22 0 1 0 0 44a22 22 0 1 0 0-44Z" />
        </clipPath>
      </defs>
      <circle className="today-backdrop-moon" cx={MOON.x} cy={MOON.y} r={MOON.radius} clipPath={`url(#${moonCutoutId})`} />
    </svg> : null}
  </div>;
}
