// The screen's state, shared by every component. Direction D (owner, 4 Nov
// 2026): one stream of notes under a search field that also creates notes,
// folders as filter chips, and the note beside the stream. The same layout
// serves desktops and tablets; below 640 px the note opens as a sheet over
// the stream.

import type { NoteMeta } from '@mirrorleaf/editor';
import type { MenuItem } from '../components/Menu.svelte';
import { api, type EngineStatus, type FolderItem, type NoteItem } from './api';
import { counts, hasTitle, visibleNotes, viewTitle, type View } from './notes';

export type Layout = 'wide' | 'medium' | 'narrow';
/** What a phone-sized screen shows: the stream, or a note as a sheet over it. */
export type Pane = 'list' | 'note';

/** At least this wide: a wider stream beside the note. */
export const WIDE = 960;
/** At least this wide: the stream and the note side by side. */
export const MEDIUM = 640;

export function layoutFor(width: number): Layout {
  return width >= WIDE ? 'wide' : width >= MEDIUM ? 'medium' : 'narrow';
}

const MESSAGE_FOR_MS = 6000;

class AppState {
  notes = $state<NoteItem[]>([]);
  folders = $state<FolderItem[]>([]);
  view = $state<View>({ kind: 'all' });
  selectedId = $state<string | null>(null);
  search = $state('');
  status = $state<EngineStatus | null>(null);
  /** The device to name in "Open Mirrorleaf on <name> to sync". */
  fallback = $state<string | null>(null);
  pane = $state<Pane>('list');
  width = $state(typeof window === 'undefined' ? WIDE : window.innerWidth);
  settings = $state(false);
  message = $state<string | null>(null);
  platform = $state('');
  /** A note just created, so the editor puts the cursor in it. */
  fresh = $state<string | null>(null);
  /** The open popover menu, if any. */
  menu = $state<{ items: MenuItem[]; x: number; y: number } | null>(null);

  layout = $derived(layoutFor(this.width));
  visible = $derived(visibleNotes(this.notes, this.view, this.search));
  counts = $derived(counts(this.notes));
  title = $derived(viewTitle(this.view, this.folders));
  /** Enter in the search field would create a note: no note has that title. */
  canCreate = $derived(
    this.search.trim() !== '' && this.view.kind !== 'trash' && !hasTitle(this.notes, this.search),
  );
  selected = $derived(this.notes.find((n) => n.id === this.selectedId) ?? null);
  /** Primitive, so the editor reopens only when this actually changes. */
  selectedTrashed = $derived(this.selected?.trashed ?? false);
  isDesktop = $derived(['macos', 'linux', 'windows'].includes(this.platform));

  /** Set by the note view while an editor is open (not reactive). */
  editorHooks: {
    /** Sends the editor's waiting edits and closes it. */
    close?: () => Promise<void>;
    /** A photo's full image arrived. */
    photo?: (hash: string) => void;
    /** Puts the cursor in the open note. */
    focus?: () => void;
  } = {};
  /** Set by the stream: puts the cursor in the search field (not reactive). */
  focusSearch?: (selectAll?: boolean) => void;

  private messageTimer: ReturnType<typeof setTimeout> | undefined;

  async start() {
    this.platform = await api.platform();
    await Promise.all([this.refresh(), this.loadStatus()]);
    if (this.layout !== 'narrow') this.selectFirst();
  }

  async refresh() {
    try {
      const list = await api.noteList();
      this.notes = list.notes;
      this.folders = list.folders;
      const view = this.view;
      if (view.kind === 'folder' && !list.folders.some((f) => f.id === view.id)) {
        this.view = { kind: 'all' };
      }
    } catch (error) {
      this.fail(error);
    }
  }

  async loadStatus() {
    try {
      this.status = await api.status();
      this.fallback = await api.fallbackPrompt();
    } catch (error) {
      this.fail(error);
    }
  }

  fail(error: unknown) {
    this.message = error instanceof Error ? error.message : String(error);
    clearTimeout(this.messageTimer);
    this.messageTimer = setTimeout(() => (this.message = null), MESSAGE_FOR_MS);
  }

  /** Runs an action, showing its error if it fails. */
  async run<T>(action: () => Promise<T>): Promise<T | undefined> {
    try {
      return await action();
    } catch (error) {
      this.fail(error);
      return undefined;
    }
  }

  openView(view: View) {
    this.view = view;
    this.search = '';
    if (this.layout === 'narrow') this.pane = 'list';
    else this.selectFirst();
  }

  selectFirst() {
    this.selectedId = this.visible[0]?.id ?? null;
  }

  select(id: string) {
    this.selectedId = id;
    if (this.layout === 'narrow') this.pane = 'note';
  }

  /** Closes the note sheet on a phone-sized screen. */
  back() {
    this.pane = 'list';
  }

  /** Opens a note and puts the cursor in it. */
  openAndFocus(id: string) {
    if (this.selectedId === id && this.editorHooks.focus) {
      if (this.layout === 'narrow') this.pane = 'note';
      this.editorHooks.focus();
      return;
    }
    this.fresh = id;
    this.select(id);
  }

  /** A new note, starting with `title` if given, in the folder being shown. */
  async newNote(title: string | null = null) {
    if (this.view.kind === 'trash' || this.view.kind === 'pinned') this.view = { kind: 'all' };
    const folder = this.view.kind === 'folder' ? this.view.id : null;
    const id = await this.run(() => api.createNote(folder, title));
    if (!id) return;
    this.search = '';
    await this.refresh();
    this.fresh = id;
    this.select(id);
  }

  /** Enter in the search field with no note of that title (direction D). */
  createFromSearch() {
    const title = this.search.trim();
    if (title) void this.newNote(title);
  }

  /** The note after `id` in the list, else the one before it. */
  private neighbour(id: string): string | null {
    const list = this.visible;
    const at = list.findIndex((n) => n.id === id);
    return list[at + 1]?.id ?? list[at - 1]?.id ?? null;
  }

  async trash(id: string) {
    const next = this.neighbour(id);
    await this.run(() => api.trashNote(id));
    if (this.selectedId === id) this.selectedId = next;
    await this.refresh();
  }

  async restore(id: string) {
    await this.run(() => api.restoreNote(id));
    await this.refresh();
  }

  async setPinned(id: string, pinned: boolean) {
    await this.run(() => api.setPinned(id, pinned));
    await this.refresh();
  }

  async move(id: string, folder: string | null) {
    await this.run(() => api.moveNote(id, folder));
    await this.refresh();
  }

  async createFolder(name: string) {
    const id = await this.run(() => api.createFolder(name));
    await this.refresh();
    if (id) this.openView({ kind: 'folder', id });
  }

  async renameFolder(id: string, name: string) {
    await this.run(() => api.renameFolder(id, name));
    await this.refresh();
  }

  async deleteFolder(id: string) {
    await this.run(() => api.deleteFolder(id));
    if (this.view.kind === 'folder' && this.view.id === id) this.openView({ kind: 'all' });
    await this.refresh();
  }

  openMenu(items: MenuItem[], x: number, y: number) {
    this.menu = { items, x, y };
  }

  /** Opens a menu under a toolbar button. */
  openMenuUnder(items: MenuItem[], button: HTMLElement) {
    const rect = button.getBoundingClientRect();
    this.menu = { items, x: rect.left, y: rect.bottom + 4 };
  }

  /** What can be done to a note: from its row in the list or the note's ⋯ button. */
  noteActions(note: NoteItem): MenuItem[] {
    if (note.trashed) {
      return [{ label: 'Restore', action: () => void this.restore(note.id) }];
    }
    return [
      {
        label: note.pinned ? 'Unpin Note' : 'Pin Note',
        action: () => void this.setPinned(note.id, !note.pinned),
      },
      { label: 'Move to', section: true },
      {
        label: 'No Folder',
        checked: note.folder === null,
        action: () => void this.move(note.id, null),
      },
      ...this.folders.map((folder) => ({
        label: folder.name,
        checked: note.folder === folder.id,
        action: () => void this.move(note.id, folder.id),
      })),
      { label: '', section: true },
      { label: 'Delete', danger: true, action: () => void this.trash(note.id) },
    ];
  }

  /** The editor saw the title or preview change: show it now, and store it. */
  noteEdited(id: string, meta: NoteMeta) {
    const note = this.notes.find((n) => n.id === id);
    if (note) {
      note.title = meta.title;
      note.preview = meta.preview;
      note.updated_ms = Date.now();
    }
    void this.run(() => api.setNoteMeta(id, meta.title, meta.preview));
  }
}

export const app = new AppState();
