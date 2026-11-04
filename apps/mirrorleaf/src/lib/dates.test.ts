import { describe, expect, it } from 'vitest';
import { listDate, noteDate } from './dates';

const at = (iso: string) => new Date(iso).getTime();
const NOW = at('2026-11-04T18:20:00');

describe('dates', () => {
  it('shows the time today and the day nearby', () => {
    expect(listDate(at('2026-11-04T09:05:00'), NOW)).toBe('09:05');
    expect(listDate(at('2026-11-03T23:59:00'), NOW)).toBe('Yesterday');
    expect(listDate(at('2026-11-01T12:00:00'), NOW)).toBe('Sunday');
    expect(listDate(at('2026-10-20T12:00:00'), NOW)).toBe('20/10/2026');
    expect(listDate(0, NOW)).toBe('');
  });

  it('writes the full date above a note', () => {
    expect(noteDate(NOW)).toBe('4 Nov 2026 · 18:20');
  });
});
