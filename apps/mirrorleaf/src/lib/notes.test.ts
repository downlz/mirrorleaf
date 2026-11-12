import { describe, expect, it } from 'vitest';
import type { NoteItem } from './api';
import {
  counts,
  displayTitle,
  folderPath,
  folderTree,
  placeLabel,
  hasTitle,
  highlight,
  snippetParts,
  visibleNotes,
  viewTitle,
} from './notes';

const note = (id: string, extra: Partial<NoteItem> = {}): NoteItem => ({
  id,
  title: id,
  preview: '',
  updated_ms: 0,
  folder: null,
  pinned: false,
  trashed: false,
  ...extra,
});

const notes = [
  note('old', { updated_ms: 1 }),
  note('new', { updated_ms: 3, folder: 'f1', preview: 'Milk and eggs' }),
  note('pinned', { updated_ms: 2, pinned: true }),
  note('gone', { updated_ms: 4, trashed: true, pinned: true }),
];
const ids = (list: NoteItem[]) => list.map((n) => n.id);

describe('the note list', () => {
  it('shows pinned notes first, then the newest', () => {
    expect(ids(visibleNotes(notes, { kind: 'all' }, ''))).toEqual(['pinned', 'new', 'old']);
  });

  it('shows a folder, the trash and search results', () => {
    expect(ids(visibleNotes(notes, { kind: 'folder', id: 'f1' }, ''))).toEqual(['new']);
    expect(ids(visibleNotes(notes, { kind: 'trash' }, ''))).toEqual(['gone']);
    expect(ids(visibleNotes(notes, { kind: 'all' }, '  EGGS '))).toEqual(['new']);
    expect(ids(visibleNotes(notes, { kind: 'pinned' }, ''))).toEqual(['pinned']);
  });

  it('adds notes the full-text search found, and splits its snippets', () => {
    const hits = new Map([['old', 'bought \u0002garlic\u0003 and…']]);
    expect(ids(visibleNotes(notes, { kind: 'all' }, 'garl', hits))).toEqual(['old']);
    expect(ids(visibleNotes(notes, { kind: 'trash' }, 'garl', hits))).toEqual([]);
    expect(snippetParts('bought \u0002garlic\u0003 and…')).toEqual([
      { text: 'bought ', match: false },
      { text: 'garlic', match: true },
      { text: ' and…', match: false },
    ]);
  });

  it('shows a folder with the folders inside it, and labels where notes live', () => {
    const folders = [
      { id: 'w', name: 'Work', parent: null },
      { id: 'c', name: 'Clients', parent: 'w' },
      { id: 'a', name: 'Acme', parent: 'c' },
      { id: 'r', name: 'Recipes', parent: null },
    ];
    const list = [
      note('plan', { folder: 'w', updated_ms: 3 }),
      note('renewal', { folder: 'a', updated_ms: 2 }),
      note('soup', { folder: 'r', updated_ms: 1 }),
    ];
    expect(ids(visibleNotes(list, { kind: 'folder', id: 'w' }, '', null, folders))).toEqual([
      'plan',
      'renewal',
    ]);
    expect(ids(visibleNotes(list, { kind: 'folder', id: 'c' }, '', null, folders))).toEqual([
      'renewal',
    ]);
    const n = counts(list, folders);
    expect([n.w, n.c, n.a, n.r]).toEqual([2, 1, 1, 1]);
    expect(folderPath(folders, 'a').map((f) => f.name)).toEqual(['Work', 'Clients', 'Acme']);
    expect(placeLabel(folders, 'a', null)).toBe('Work › Clients › Acme');
    expect(placeLabel(folders, 'a', 'w')).toBe('Clients › Acme');
    expect(folderTree(folders).map((t) => `${t.depth}${t.folder.name}`)).toEqual([
      '0Work',
      '1Clients',
      '2Acme',
      '0Recipes',
    ]);
  });

  it('knows when a typed title is new, and marks matches', () => {
    expect(hasTitle(notes, ' NEW ')).toBe(true);
    expect(hasTitle(notes, 'Packing list')).toBe(false);
    expect(highlight('Pack walking boots, pack socks', 'pack')).toEqual([
      { text: 'Pack', match: true },
      { text: ' walking boots, ', match: false },
      { text: 'pack', match: true },
      { text: ' socks', match: false },
    ]);
    expect(highlight('Milk', '')).toEqual([{ text: 'Milk', match: false }]);
  });

  it('counts notes per folder', () => {
    expect(counts(notes)).toEqual({ all: 3, trash: 1, f1: 1 });
  });

  it('names views and untitled notes', () => {
    expect(
      viewTitle({ kind: 'folder', id: 'f1' }, [{ id: 'f1', name: 'Work', parent: null }]),
    ).toBe('Work');
    expect(viewTitle({ kind: 'trash' }, [])).toBe('Recently Deleted');
    expect(displayTitle({ title: '  ' })).toBe('New Note');
  });
});
