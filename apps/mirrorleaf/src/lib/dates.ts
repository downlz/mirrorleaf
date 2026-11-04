// Dates as the note list and the note show them, in British English.

const DAY = 24 * 60 * 60 * 1000;

const time = new Intl.DateTimeFormat('en-GB', { hour: '2-digit', minute: '2-digit' });
const weekday = new Intl.DateTimeFormat('en-GB', { weekday: 'long' });
const numeric = new Intl.DateTimeFormat('en-GB', {
  day: '2-digit',
  month: '2-digit',
  year: 'numeric',
});
const short = new Intl.DateTimeFormat('en-GB', { day: 'numeric', month: 'short', year: 'numeric' });

function startOfDay(ms: number): number {
  const date = new Date(ms);
  date.setHours(0, 0, 0, 0);
  return date.getTime();
}

/** "18:20" today, "Yesterday", a weekday within the last week, else "04/11/2026". */
export function listDate(ms: number, now = Date.now()): string {
  if (!ms) return '';
  const days = Math.round((startOfDay(now) - startOfDay(ms)) / DAY);
  if (days <= 0) return time.format(ms);
  if (days === 1) return 'Yesterday';
  if (days < 7) return weekday.format(ms);
  return numeric.format(ms);
}

/** "4 Nov 2026 · 18:20", above a note. */
export function noteDate(ms: number): string {
  if (!ms) return '';
  return `${short.format(ms)} · ${time.format(ms)}`;
}
