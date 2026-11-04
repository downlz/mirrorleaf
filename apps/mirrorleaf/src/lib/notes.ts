// What the note list shows for a view: plain functions, so they can be tested.

import type { FolderItem, NoteItem } from './api';

export type View =
  { kind: 'all' } | { kind: 'pinned' } | { kind: 'folder'; id: string } | { kind: 'trash' };

/** The notes a view shows, matching the search: pinned first, then newest first. */
export function visibleNotes(notes: NoteItem[], view: View, search: string): NoteItem[] {
  const query = search.trim().toLowerCase();
  return notes
    .filter((note) => {
      if (view.kind === 'trash') return note.trashed;
      if (note.trashed) return false;
      if (view.kind === 'pinned') return note.pinned;
      return view.kind === 'all' || note.folder === view.id;
    })
    .filter(
      (note) =>
        !query ||
        note.title.toLowerCase().includes(query) ||
        note.preview.toLowerCase().includes(query),
    )
    .sort(
      (a, b) =>
        Number(b.pinned && view.kind !== 'trash') - Number(a.pinned && view.kind !== 'trash') ||
        b.updated_ms - a.updated_ms ||
        a.id.localeCompare(b.id),
    );
}

/** How many notes each folder holds, and all notes and the trash. */
export function counts(notes: NoteItem[]): {
  all: number;
  trash: number;
  [folder: string]: number;
} {
  const result = { all: 0, trash: 0 } as { all: number; trash: number; [folder: string]: number };
  for (const note of notes) {
    if (note.trashed) {
      result.trash += 1;
      continue;
    }
    result.all += 1;
    if (note.folder) result[note.folder] = (result[note.folder] ?? 0) + 1;
  }
  return result;
}

export function viewTitle(view: View, folders: FolderItem[]): string {
  if (view.kind === 'all') return 'All Notes';
  if (view.kind === 'trash') return 'Recently Deleted';
  if (view.kind === 'pinned') return 'Pinned';
  return folders.find((f) => f.id === view.id)?.name ?? 'Folder';
}

/** Whether a note with exactly this title (ignoring case) is in the list. */
export function hasTitle(notes: NoteItem[], title: string): boolean {
  const wanted = title.trim().toLowerCase();
  return notes.some((note) => note.title.trim().toLowerCase() === wanted);
}

/** Text split around the search query's matches, for highlighting. */
export function highlight(text: string, query: string): { text: string; match: boolean }[] {
  const q = query.trim().toLowerCase();
  if (!q) return [{ text, match: false }];
  const parts: { text: string; match: boolean }[] = [];
  const lower = text.toLowerCase();
  let from = 0;
  for (let at = lower.indexOf(q); at !== -1; at = lower.indexOf(q, from)) {
    if (at > from) parts.push({ text: text.slice(from, at), match: false });
    parts.push({ text: text.slice(at, at + q.length), match: true });
    from = at + q.length;
  }
  if (from < text.length) parts.push({ text: text.slice(from), match: false });
  return parts;
}

/** A note's title as the list shows it. */
export function displayTitle(note: Pick<NoteItem, 'title'>): string {
  return note.title.trim() || 'New Note';
}
