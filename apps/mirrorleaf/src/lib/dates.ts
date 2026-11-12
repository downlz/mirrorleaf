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

const dayMonth = new Intl.DateTimeFormat('en-GB', { day: 'numeric', month: 'long' });

/** When a device was last seen (E6): "just now", "12 minutes ago", "today at 09:05",
 *  "yesterday at 21:40", else "4 November". `seconds` is Unix time. */
export function lastSeen(seconds: number, now = Date.now()): string {
  const ms = seconds * 1000;
  const minutes = Math.floor((now - ms) / 60000);
  if (minutes < 2) return 'just now';
  if (minutes < 60) return `${minutes} minutes ago`;
  const days = Math.round((startOfDay(now) - startOfDay(ms)) / DAY);
  if (days <= 0) return `today at ${time.format(ms)}`;
  if (days === 1) return `yesterday at ${time.format(ms)}`;
  return dayMonth.format(ms);
}
