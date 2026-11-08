import { describe, expect, it } from 'vitest';
import { auraImage, glowAt } from './aura';

const at = (h: number, m = 0) => h * 60 + m;

describe('glowAt', () => {
  it('is sunlight through the day and moonlight through the night', () => {
    expect(glowAt(at(12), 'light')[0]).toEqual([255, 214, 120, 0.5]);
    expect(glowAt(at(2), 'light')[0]).toEqual([190, 205, 250, 0.38]);
    expect(glowAt(at(2), 'dark')[0]).toEqual([90, 120, 230, 0.18]);
  });

  it('blends between moments', () => {
    const [r, , b] = glowAt(at(17), 'light')[0];
    expect(r).toBe(255);
    expect(b).toBe(120);
    const [, g] = glowAt(at(17), 'light')[0];
    expect(g).toBeGreaterThan(196);
    expect(g).toBeLessThan(214);
  });

  it('meets itself at midnight', () => {
    expect(glowAt(at(24), 'light')).toEqual(glowAt(0, 'light'));
    expect(glowAt(-1, 'dark')).toEqual(glowAt(at(23, 59), 'dark'));
  });

  it('changes by at most a little from one minute to the next', () => {
    for (const scheme of ['light', 'dark'] as const) {
      for (let m = 0; m < at(24); m += 1) {
        const next = glowAt(m + 1, scheme).flat();
        glowAt(m, scheme)
          .flat()
          .forEach((v, k) => {
            // Each light is red, green, blue, then opacity.
            const most = k % 4 === 3 ? 0.01 : 3;
            expect(Math.abs(v - (next[k] ?? NaN))).toBeLessThanOrEqual(most);
          });
      }
    }
  });
});

describe('auraImage', () => {
  it('draws three fading lights in the shapes from the stylesheet', () => {
    const image = auraImage(glowAt(at(12), 'light'));
    expect(image.match(/radial-gradient\(/g)).toHaveLength(3);
    expect(image).toContain('radial-gradient(var(--aura-core), rgba(255, 214, 120, 0.5) 0%');
    expect(image).toContain('rgba(255, 214, 120, 0) 100%');
  });
});
