import { isDaytime, sunHeight } from "./backdropPreference";

/** The scene's own coordinate space, scaled to the header's width and height. */
const WIDTH = 1200;
const HEIGHT = 200;
/** Where the sun sits on the horizon at its lowest, and how far it climbs by noon. */
const HORIZON_Y = HEIGHT * 0.95;
const SUN_CLIMB = HEIGHT * 0.7;
const SUN_X = WIDTH * 0.86;
const SUN_RADIUS = 26;
const MOON = { x: WIDTH * 0.86, y: HEIGHT * 0.34, radius: 24 };
/** A few fixed stars, as shares of the scene's width and height; the same every evening. */
const STARS = [[0.52, 0.2], [0.6, 0.55], [0.68, 0.28], [0.73, 0.7], [0.79, 0.16], [0.93, 0.62], [0.97, 0.22]] as const;

/**
 * Today's optional backdrop, behind the header band only: between sunrise and
 * sunset a daytime sky whose sun stands higher toward noon, otherwise a dusk
 * sky with a moon and a few stars.
 * Original art drawn in code from theme tokens, hidden from assistive
 * technology, with no motion beyond a soft fade when the view switches that
 * reduced motion turns off.
 */
export function TodayBackdrop({ now }: { now: Date }) {
  const view = isDaytime(now) ? "day" : "evening";
  return <div className="today-backdrop" data-view={view} aria-hidden="true">
    <svg viewBox={`0 0 ${WIDTH} ${HEIGHT}`} preserveAspectRatio="xMaxYMid slice" focusable="false">
      {view === "day" ? <>
        <ellipse className="today-backdrop-cloud" cx={WIDTH * 0.62} cy={HEIGHT * 0.3} rx={90} ry={16} />
        <ellipse className="today-backdrop-cloud" cx={WIDTH * 0.68} cy={HEIGHT * 0.38} rx={60} ry={12} />
        <ellipse className="today-backdrop-cloud" cx={WIDTH * 0.95} cy={HEIGHT * 0.75} rx={70} ry={12} />
        <circle className="today-backdrop-sun" data-height={sunHeight(now).toFixed(2)} cx={SUN_X} cy={HORIZON_Y - SUN_CLIMB * sunHeight(now)} r={SUN_RADIUS} />
      </> : <>
        {STARS.map(([x, y]) => <circle key={`${x}:${y}`} className="today-backdrop-star" cx={WIDTH * x} cy={HEIGHT * y} r={2} />)}
        <circle className="today-backdrop-moon" cx={MOON.x} cy={MOON.y} r={MOON.radius} />
        <circle className="today-backdrop-moon-shade" cx={MOON.x + 12} cy={MOON.y - 6} r={MOON.radius - 2} />
      </>}
    </svg>
  </div>;
}
