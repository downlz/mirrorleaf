// The screen's state, shared by every component. Direction D (owner, 4 Nov
// 2026): one stream of notes under a search field that also creates notes,
// folders as filter chips, and the note beside the stream. The same layout
// serves desktops and tablets; below 640 px the note opens as a sheet over
// the stream.

import type { NoteMeta } from '@mirrorleaf/editor';
import type { MenuItem } from '../components/Menu.svelte';
import {
  api,
  type BackupSettings,
  type EngineStatus,
  type FolderItem,
  type NoteItem,
  type PairingStatus,
} from './api';
import { counts, folderTree, hasTitle, visibleNotes, viewTitle, type View } from './notes';

export type Layout = 'wide' | 'medium' | 'narrow';
/** What Android's back gesture closes, newest first: overlays, and each
 *  folder stepped into (M12, option A). */
type Overlay = 'note' | 'settings' | 'welcome' | 'folder';
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
  /** Adding a device, on either side (E1). */
  pairing = $state<PairingStatus>({ state: 'idle' });
  /** On first run, the sheet over the welcome page, if one is open. */
  welcomeSheet = $state<'create' | 'join' | 'recover' | null>(null);
  /**
   * Setting up continues after the account exists (the kit, the permissions,
   * the devices after a recovery): the welcome page stays until it's done.
   */
  onboarding = $state(false);
  /** The app lock covers the screen (E4); null until the engine has said. */
  locked = $state<boolean | null>(null);
  /** Backups (E9): the last one, and the main screen's reminder. */
  backup = $state<BackupSettings | null>(null);
  /** The full-text search's results for `search`, note ID → snippet (M12, E5). */
  hits = $state<Map<string, string> | null>(null);

  layout = $derived(layoutFor(this.width));
  visible = $derived(visibleNotes(this.notes, this.view, this.search, this.hits, this.folders));
  counts = $derived(counts(this.notes, this.folders));
  title = $derived(viewTitle(this.view, this.folders));
  /** Enter in the search field would create a note: no note has that title. */
  canCreate = $derived(
    this.search.trim() !== '' && this.view.kind !== 'trash' && !hasTitle(this.notes, this.search),
  );
  selected = $derived(this.notes.find((n) => n.id === this.selectedId) ?? null);
  /** Primitive, so the editor reopens only when this actually changes. */
  selectedTrashed = $derived(this.selected?.trashed ?? false);
  isDesktop = $derived(['macos', 'linux', 'windows'].includes(this.platform));
  /** No account yet: the welcome page instead of the stream (direction A, owner, 9 Nov 2026). */
  needsAccount = $derived(this.status !== null && (!this.status.account || this.onboarding));
  /** What to call this device in sentences: "Add this phone to my account". */
  deviceWord = $derived(
    this.platform === 'android'
      ? this.layout === 'narrow'
        ? 'phone'
        : 'tablet'
      : this.platform === 'macos'
        ? 'Mac'
        : 'computer',
  );

  /** Set by the note view while an editor is open (not reactive). */
  editorHooks: {
    /** Sends the editor's waiting edits and closes it. */
    close?: () => Promise<void>;
    /** A photo's full image arrived. */
    photo?: (hash: string) => void;
    /** Puts the cursor in the open note. */
    focus?: () => void;
    /** Sends the open note's waiting edits now. */
    flush?: () => Promise<void>;
  } = {};
  /** Set by the stream: puts the cursor in the search field (not reactive). */
  focusSearch?: (selectAll?: boolean) => void;

  private messageTimer: ReturnType<typeof setTimeout> | undefined;
  private readyReported = false;

  async start() {
    this.platform = await api.platform();
    // The lock first, so no note shows before it.
    this.locked = await api.isLocked().catch(() => false);
    await Promise.all([this.refresh(), this.loadStatus()]);
    if (this.layout !== 'narrow') this.selectFirst();
    await this.openShared();
    // With a note beside the stream, the note view reports once its editor is open.
    if (this.layout === 'narrow' || !this.selectedId) this.reportReady('note list');
  }

  /**
   * The screen is ready to use: the engine logs the cold start once (plan §7.3,
   * "first note editable"). Reported after the next paint, so drawing counts.
   */
  reportReady(what: 'note list' | 'open note') {
    if (this.readyReported) return;
    this.readyReported = true;
    requestAnimationFrame(() => setTimeout(() => void api.startupDone(what).catch(() => {}), 0));
  }

  async refresh() {
    try {
      const list = await api.noteList();
      this.notes = list.notes;
      this.folders = list.folders;
      const view = this.view;
      if (view.kind === 'folder' && !list.folders.some((f) => f.id === view.id)) {
        // Deleted on another device: back out to the nearest folder still there.
        const live = this.folderTrail.filter((id) => list.folders.some((f) => f.id === id));
        this.leaveTo(live.at(-1) ?? null);
      }
    } catch (error) {
      this.fail(error);
    }
  }

  async loadPairing() {
    try {
      this.pairing = await api.pairingStatus();
    } catch (error) {
      this.fail(error);
    }
  }

  async loadStatus() {
    try {
      this.status = await api.status();
      this.fallback = await api.fallbackPrompt();
      this.backup = this.status.account ? await api.backupSettings() : null;
    } catch (error) {
      this.fail(error);
    }
  }

  /** The screen went out of sight (E4): the lock's time away starts. */
  screenLeft() {
    void api.screenLeft().catch(() => {});
  }

  /** The screen is back in sight, or the engine locked it: show the lock if due. */
  async checkLock() {
    const locked = await this.run(api.screenReturned);
    if (locked !== undefined) this.locked = locked;
  }

  /** Writes a backup where the person chooses (E9). */
  async backUpNow() {
    const done = await this.run(api.saveBackup);
    if (!done) return;
    this.notify('Backup saved.');
    await this.loadStatus();
  }

  /** Shows a short message for a few seconds. */
  notify(message: string) {
    this.message = message;
    clearTimeout(this.messageTimer);
    this.messageTimer = setTimeout(() => (this.message = null), MESSAGE_FOR_MS);
  }

  fail(error: unknown) {
    this.notify(error instanceof Error ? error.message : String(error));
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
    if (this.layout === 'narrow') this.closeOverlay('note');
    else this.selectFirst();
  }

  selectFirst() {
    this.selectedId = this.visible[0]?.id ?? null;
  }

  select(id: string) {
    this.selectedId = id;
    if (this.layout === 'narrow') this.showSheet();
  }

  // The note sheet and Settings each add a history entry, so Android's back
  // gesture (which goes back in the web view) closes the newest one (M6).
  // Closing them from the screen goes back through history too.
  private overlays: Overlay[] = [];

  private pushOverlay(kind: Overlay) {
    if (kind !== 'folder' && this.overlays.includes(kind)) return;
    this.overlays.push(kind);
    // How many overlays are open at this entry, so a jump back several
    // entries (a breadcrumb) closes the right number.
    history.pushState({ mirrorleaf: kind, open: this.overlays.length }, '');
  }

  /** The folders stepped into, outermost first (M12, option A). */
  private folderTrail: string[] = [];

  /**
   * Steps into a folder: the chips become its folders and the back gesture,
   * Esc or the breadcrumb come back out.
   */
  enterFolder(id: string) {
    if (this.view.kind !== 'folder') this.folderTrail = [];
    this.folderTrail.push(id);
    this.openView({ kind: 'folder', id });
    this.pushOverlay('folder');
  }

  /** Forgets the folders stepped into, leaving their history entries behind. */
  private dropTrail() {
    this.folderTrail = [];
    this.overlays = this.overlays.filter((k) => k !== 'folder');
  }

  /** Goes back out to `id`, a folder on the way in, or to All Notes with null. */
  leaveTo(id: string | null) {
    if (id !== null && !this.folderTrail.includes(id)) {
      // Not on the way in (the folders were moved meanwhile): go there directly.
      this.dropTrail();
      this.enterFolder(id);
      return;
    }
    const keep = id === null ? 0 : this.folderTrail.indexOf(id) + 1;
    const up = this.folderTrail.length - keep;
    let trailing = 0;
    for (let i = this.overlays.length - 1; i >= 0 && this.overlays[i] === 'folder'; i--) trailing++;
    if (up > 0 && trailing >= up) {
      // Through history, so the back gesture stays in step.
      history.go(-up);
      return;
    }
    const kept = this.folderTrail.slice(0, keep);
    this.dropTrail();
    this.folderTrail = kept;
    this.showTrailEnd();
  }

  /** Esc on the Mac: one folder up. */
  folderUp() {
    if (this.view.kind !== 'folder') return;
    const trail = this.folderTrail;
    this.leaveTo(trail.length > 1 ? (trail[trail.length - 2] ?? null) : null);
  }

  private showTrailEnd() {
    const at = this.folderTrail.at(-1);
    this.openView(at ? { kind: 'folder', id: at } : { kind: 'all' });
  }

  private showSheet() {
    this.pane = 'note';
    this.pushOverlay('note');
  }

  openSettings() {
    this.settings = true;
    this.pushOverlay('settings');
  }

  closeSettings() {
    this.closeOverlay('settings');
  }

  /** Closes the note sheet on a phone-sized screen. */
  back() {
    this.closeOverlay('note');
  }

  openWelcomeSheet(kind: 'create' | 'join' | 'recover') {
    this.welcomeSheet = kind;
    this.pushOverlay('welcome');
  }

  closeWelcomeSheet() {
    this.closeOverlay('welcome');
  }

  private closeOverlay(kind: Overlay) {
    if (this.overlays.at(-1) === kind) {
      history.back();
    } else {
      this.overlays = this.overlays.filter((k) => k !== kind);
      this.close(kind);
    }
  }

  private close(kind: Overlay) {
    if (kind === 'folder') {
      this.folderTrail.pop();
      this.showTrailEnd();
    } else if (kind === 'note') this.pane = 'list';
    else if (kind === 'settings') this.settings = false;
    else {
      this.welcomeSheet = null;
      // Closed midway through setting up (the back gesture): the account
      // exists, so the notes take over; a kit not saved yet comes back.
      if (this.onboarding) {
        this.onboarding = false;
        void this.loadStatus();
      }
    }
  }

  /** The back gesture, or a close that went through history (one entry or several). */
  onPopState() {
    const state = history.state as { open?: number } | null;
    const open = state?.open ?? 0;
    do {
      const kind = this.overlays.pop();
      if (kind) this.close(kind);
    } while (this.overlays.length > open);
  }

  /** Opens a note that was made outside the screen (shared from another app). */
  async openShared() {
    const id = await this.run(api.takePendingOpen);
    if (!id) return;
    await this.refresh();
    this.dropTrail();
    this.view = { kind: 'all' };
    this.search = '';
    this.select(id);
  }

  /**
   * Files dropped on the window outside the editor (M12, E5; the Mac's
   * stand-in for a share extension): photos and text files become a new
   * note, which then opens. The editor takes its own drops.
   */
  async dropFiles(files: File[]) {
    const isPhoto = (f: File) => f.type.startsWith('image/') || /\.(heic|heif|avif)$/i.test(f.name);
    const isText = (f: File) => f.type.startsWith('text/') || /\.(txt|md|markdown)$/i.test(f.name);
    const photos = files.filter(isPhoto);
    const texts = files.filter((f) => !isPhoto(f) && isText(f));
    if (photos.length + texts.length === 0) {
      this.notify('Drop photos or text files to make a note.');
      return;
    }
    await this.run(async () => {
      const staged: string[] = [];
      for (const photo of photos) {
        staged.push(await api.stageDropped(new Uint8Array(await photo.arrayBuffer())));
      }
      // At most 1 MB of each text file, as the Dock does.
      const text = (await Promise.all(texts.map((t) => t.slice(0, 1 << 20).text()))).join('\n');
      await api.dropIntoNote(text || null, staged);
    });
    // The engine asks the screen to open the new note (`open_note`).
  }

  /** Opens a note and puts the cursor in it. */
  openAndFocus(id: string) {
    if (this.selectedId === id && this.editorHooks.focus) {
      if (this.layout === 'narrow') this.showSheet();
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

  private searchSeq = 0;

  /** Runs the full-text search for what's in the search field (M12, E5). */
  async runSearch(query: string) {
    const seq = ++this.searchSeq;
    if (!query.trim()) {
      this.hits = null;
      return;
    }
    const found = await api.searchNotes(query).catch(() => null);
    // A later keystroke's search wins.
    if (seq !== this.searchSeq) return;
    this.hits = found ? new Map(found.map((hit) => [hit.id, hit.snippet])) : null;
  }

  /** Deletes a note for good (M12), after the person confirmed. */
  async deleteForGood(id: string) {
    const next = this.neighbour(id);
    await this.run(() => api.deleteNote(id));
    if (this.selectedId === id) this.selectedId = next;
    await this.refresh();
  }

  /** Deletes everything in Recently Deleted for good, after the person confirmed. */
  async emptyTrash() {
    const count = await this.run(api.emptyTrash);
    if (count === undefined) return;
    if (this.selectedTrashed) this.selectedId = null;
    this.notify(count === 1 ? 'Deleted 1 note for good.' : `Deleted ${count} notes for good.`);
    await this.refresh();
  }

  /** The confirmation for deleting a note for good, as a menu where the last one was. */
  confirmDeleteForGood(note: NoteItem, x = this.lastMenuAt.x, y = this.lastMenuAt.y) {
    this.openMenu(
      [
        {
          label: `Delete “${note.title || 'New Note'}” for good? It leaves all your devices and can't be brought back.`,
          section: true,
        },
        {
          label: 'Delete Permanently',
          danger: true,
          action: () => void this.deleteForGood(note.id),
        },
        { label: 'Cancel' },
      ],
      x,
      y,
    );
  }

  async setPinned(id: string, pinned: boolean) {
    await this.run(() => api.setPinned(id, pinned));
    await this.refresh();
  }

  async move(id: string, folder: string | null) {
    await this.run(() => api.moveNote(id, folder));
    await this.refresh();
  }

  async createFolder(name: string, parent: string | null = null) {
    const id = await this.run(() => api.createFolder(name, parent));
    await this.refresh();
    // Steps into the new folder, one level down from where it was made.
    if (id) this.enterFolder(id);
  }

  async renameFolder(id: string, name: string) {
    await this.run(() => api.renameFolder(id, name));
    await this.refresh();
  }

  async moveFolder(id: string, parent: string | null) {
    await this.run(() => api.moveFolder(id, parent));
    await this.refresh();
  }

  async deleteFolder(id: string) {
    await this.run(() => api.deleteFolder(id));
    if (this.view.kind === 'folder' && this.view.id === id) this.folderUp();
    await this.refresh();
  }

  /** Where the last menu opened, for a confirmation that follows it. */
  private lastMenuAt = { x: 0, y: 0 };

  openMenu(items: MenuItem[], x: number, y: number) {
    this.lastMenuAt = { x, y };
    this.menu = { items, x, y };
  }

  /** Opens a menu under a toolbar button. */
  openMenuUnder(items: MenuItem[], button: HTMLElement) {
    const rect = button.getBoundingClientRect();
    this.openMenu(items, rect.left, rect.bottom + 4);
  }

  /** What can be done to a note: from its row in the list or the note's ⋯ button. */
  noteActions(note: NoteItem): MenuItem[] {
    if (note.trashed) {
      return [
        { label: 'Restore', action: () => void this.restore(note.id) },
        { label: '', section: true },
        {
          label: 'Delete Permanently…',
          danger: true,
          action: () => this.confirmDeleteForGood(note),
        },
      ];
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
      ...folderTree(this.folders).map(({ folder, depth }) => ({
        label: folder.name,
        depth,
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
