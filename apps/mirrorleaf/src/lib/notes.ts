// What the note list shows for a view: plain functions, so they can be tested.

import type { FolderItem, NoteItem } from './api';

export type View =
  { kind: 'all' } | { kind: 'pinned' } | { kind: 'folder'; id: string } | { kind: 'trash' };

/**
 * The notes a view shows, matching the search: pinned first, then newest first.
 * A note matches if its title or preview contains the query, or the engine's
 * full-text search found it (`hits`, by note ID, M12).
 */
export function visibleNotes(
  notes: NoteItem[],
  view: View,
  search: string,
  hits: ReadonlyMap<string, string> | null = null,
  folders: FolderItem[] = [],
): NoteItem[] {
  const query = search.trim().toLowerCase();
  // A folder shows its own notes and those of the folders inside it (M12).
  const inView =
    view.kind === 'folder' ? new Set([view.id, ...folderDescendants(folders, view.id)]) : null;
  return notes
    .filter((note) => {
      if (view.kind === 'trash') return note.trashed;
      if (note.trashed) return false;
      if (view.kind === 'pinned') return note.pinned;
      return view.kind === 'all' || (note.folder !== null && (inView?.has(note.folder) ?? false));
    })
    .filter(
      (note) =>
        !query ||
        note.title.toLowerCase().includes(query) ||
        note.preview.toLowerCase().includes(query) ||
        (hits?.has(note.id) ?? false),
    )
    .sort(
      (a, b) =>
        Number(b.pinned && view.kind !== 'trash') - Number(a.pinned && view.kind !== 'trash') ||
        b.updated_ms - a.updated_ms ||
        a.id.localeCompare(b.id),
    );
}

/** How many notes each folder holds, counting the folders inside it, and all notes and the trash. */
export function counts(
  notes: NoteItem[],
  folders: FolderItem[] = [],
): {
  all: number;
  trash: number;
  [folder: string]: number;
} {
  const result = { all: 0, trash: 0 } as { all: number; trash: number; [folder: string]: number };
  const parent = new Map(folders.map((f) => [f.id, f.parent]));
  for (const note of notes) {
    if (note.trashed) {
      result.trash += 1;
      continue;
    }
    result.all += 1;
    // The note's folder and each folder above it.
    const seen = new Set<string>();
    for (let at = note.folder; at && !seen.has(at); at = parent.get(at) ?? null) {
      seen.add(at);
      result[at] = (result[at] ?? 0) + 1;
    }
  }
  return result;
}

/** Every folder inside `id`, at any depth. */
export function folderDescendants(folders: FolderItem[], id: string): string[] {
  const found: string[] = [];
  const queue = [id];
  while (queue.length) {
    const at = queue.pop();
    for (const child of folders) {
      if (child.parent === at && child.id !== id && !found.includes(child.id)) {
        found.push(child.id);
        queue.push(child.id);
      }
    }
  }
  return found;
}

/** The folders from the top level down to `id`, inclusive. */
export function folderPath(folders: FolderItem[], id: string): FolderItem[] {
  const byId = new Map(folders.map((f) => [f.id, f]));
  const path: FolderItem[] = [];
  const seen = new Set<string>();
  for (
    let at = byId.get(id);
    at && !seen.has(at.id);
    at = at.parent ? byId.get(at.parent) : undefined
  ) {
    seen.add(at.id);
    path.unshift(at);
  }
  return path;
}

/** Where a note lives, as the list labels it: its folders below `base` ("Clients › Acme"). */
export function placeLabel(folders: FolderItem[], folder: string, base: string | null): string {
  const path = folderPath(folders, folder);
  const from = base ? path.findIndex((f) => f.id === base) + 1 : 0;
  return path
    .slice(from)
    .map((f) => f.name)
    .join(' › ');
}

/** Every folder, each followed by the folders inside it, with its depth: for "Move to". */
export function folderTree(folders: FolderItem[]): { folder: FolderItem; depth: number }[] {
  const out: { folder: FolderItem; depth: number }[] = [];
  const known = new Set(folders.map((f) => f.id));
  const walk = (parent: string | null, depth: number) => {
    for (const folder of folders) {
      const at = folder.parent && known.has(folder.parent) ? folder.parent : null;
      if (at === parent && !out.some((o) => o.folder.id === folder.id)) {
        out.push({ folder, depth });
        walk(folder.id, depth + 1);
      }
    }
  };
  walk(null, 0);
  return out;
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

/**
 * A search snippet split into plain text and matches. The engine marks matches
 * with Start of Text (\u0002) and End of Text (\u0003).
 */
export function snippetParts(snippet: string): { text: string; match: boolean }[] {
  const START = String.fromCharCode(2);
  const END = String.fromCharCode(3);
  const parts: { text: string; match: boolean }[] = [];
  let text = '';
  let match = false;
  for (const ch of snippet) {
    if (ch !== START && ch !== END) {
      text += ch;
      continue;
    }
    if (text) parts.push({ text, match });
    text = '';
    match = ch === START;
  }
  if (text) parts.push({ text, match });
  return parts;
}
