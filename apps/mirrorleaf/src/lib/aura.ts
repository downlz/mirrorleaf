// The glow behind the screen (owner, 8 Nov 2026): a soft light like Gemini's
// home screen, whose colours follow the time of day on this device's clock:
// rose at dawn, sunlight by day, amber in the evening, moonlight at night.
// Dark mode uses deeper, dimmer versions of the same lights. This file picks
// the colours; the shapes are in styles.css (--aura-core, -side, -halo).

/** Red, green and blue (0–255), then opacity (0–1). */
export type Rgba = readonly [number, number, number, number];
/** Three lights: a bright core, a side light beside it and a wide halo. */
export type Glow = readonly [Rgba, Rgba, Rgba];
export type Scheme = 'light' | 'dark';

type Palette = Readonly<Record<Scheme, Glow>>;

const NIGHT: Palette = {
  light: [
    [190, 205, 250, 0.38],
    [212, 196, 245, 0.3],
    [175, 225, 220, 0.26],
  ],
  dark: [
    [90, 120, 230, 0.18],
    [130, 100, 220, 0.13],
    [60, 170, 160, 0.11],
  ],
};

const DAWN: Palette = {
  light: [
    [255, 200, 160, 0.42],
    [248, 190, 210, 0.38],
    [220, 205, 245, 0.3],
  ],
  dark: [
    [235, 160, 160, 0.15],
    [200, 130, 200, 0.14],
    [140, 140, 235, 0.11],
  ],
};

// The screenshot the owner shared: butter yellow, peach, a blush edge. On a
// dark screen, warm lights keep some blue in them, or they turn brown.
const DAY: Palette = {
  light: [
    [255, 214, 120, 0.5],
    [255, 190, 160, 0.42],
    [250, 205, 215, 0.3],
  ],
  dark: [
    [245, 190, 140, 0.16],
    [230, 140, 170, 0.14],
    [140, 150, 230, 0.08],
  ],
};

const DUSK: Palette = {
  light: [
    [255, 196, 120, 0.5],
    [250, 170, 150, 0.42],
    [240, 190, 215, 0.32],
  ],
  dark: [
    [240, 160, 120, 0.17],
    [225, 120, 150, 0.15],
    [160, 110, 210, 0.11],
  ],
};

const TWILIGHT: Palette = {
  light: [
    [205, 195, 245, 0.4],
    [235, 190, 220, 0.34],
    [185, 205, 245, 0.3],
  ],
  dark: [
    [140, 120, 230, 0.18],
    [190, 110, 180, 0.13],
    [90, 130, 220, 0.13],
  ],
};

/** Minutes after midnight, and the palette at that moment; in between, the
 *  two neighbours blend. Fixed hours: sunrise would need the user's location. */
const MOMENTS: readonly (readonly [number, Palette])[] = [
  [0, NIGHT],
  [5 * 60, NIGHT],
  [6 * 60 + 30, DAWN],
  [9 * 60, DAY],
  [16 * 60, DAY],
  [18 * 60, DUSK],
  [19 * 60 + 45, TWILIGHT],
  [21 * 60 + 30, NIGHT],
  [24 * 60, NIGHT],
];

const DAY_MINUTES = 24 * 60;

/** The glow at a time of day, in minutes after midnight. */
export function glowAt(minutes: number, scheme: Scheme): Glow {
  const m = ((minutes % DAY_MINUTES) + DAY_MINUTES) % DAY_MINUTES;
  let [from, a, to, b] = [0, NIGHT, DAY_MINUTES, NIGHT];
  for (const [at, palette] of MOMENTS) {
    if (at <= m) [from, a] = [at, palette];
    else {
      [to, b] = [at, palette];
      break;
    }
  }
  const t = (m - from) / (to - from);
  const eased = t * t * (3 - 2 * t);
  const mix = (x: Rgba, y: Rgba): Rgba => [
    Math.round(x[0] + (y[0] - x[0]) * eased),
    Math.round(x[1] + (y[1] - x[1]) * eased),
    Math.round(x[2] + (y[2] - x[2]) * eased),
    Math.round((x[3] + (y[3] - x[3]) * eased) * 1000) / 1000,
  ];
  const [p, q] = [a[scheme], b[scheme]];
  return [mix(p[0], q[0]), mix(p[1], q[1]), mix(p[2], q[2])];
}

/** How each light fades: the share of its opacity left at a distance from
 *  its centre (in % of its radius). Eased, so no edge shows. */
const FADE: readonly (readonly [number, number])[] = [
  [1, 0],
  [0.9, 24],
  [0.66, 48],
  [0.32, 70],
  [0.09, 88],
  [0, 100],
];

function light(shape: 'core' | 'side' | 'halo', [r, g, b, a]: Rgba): string {
  const stops = FADE.map(
    ([share, at]) => `rgba(${r}, ${g}, ${b}, ${Math.round(a * share * 1000) / 1000}) ${at}%`,
  );
  return `radial-gradient(var(--aura-${shape}), ${stops.join(', ')})`;
}

/** The glow as a CSS background image: three radial gradients, the core on top. */
export function auraImage([core, side, halo]: Glow): string {
  return [light('core', core), light('side', side), light('halo', halo)].join(', ');
}

/** Paints the glow into `--aura` now, then keeps it in step with the clock
 *  (checked each minute while the screen is visible) and the light or dark
 *  setting. It is only rewritten when it changes. */
export function startAura(root: HTMLElement = document.documentElement): void {
  const dark = window.matchMedia('(prefers-color-scheme: dark)');
  let shown = '';
  const paint = () => {
    if (document.visibilityState === 'hidden') return;
    const now = new Date();
    const image = auraImage(
      glowAt(now.getHours() * 60 + now.getMinutes(), dark.matches ? 'dark' : 'light'),
    );
    if (image === shown) return;
    shown = image;
    root.style.setProperty('--aura', image);
  };
  paint();
  setInterval(paint, 60_000);
  document.addEventListener('visibilitychange', paint);
  dark.addEventListener('change', paint);
}
