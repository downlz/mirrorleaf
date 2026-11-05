// The note editor (plan §5.6): TipTap 3 on a Yjs document whose updates go to
// the Rust engine, which owns persistence and sync. Both sides use Yjs' binary
// update format; the note's content is the `default` XML fragment, which the
// Rust core reads and writes as well.

import { Editor } from '@tiptap/core';
import Collaboration from '@tiptap/extension-collaboration';
import { TaskItem, TaskList } from '@tiptap/extension-list';
import { Placeholder } from '@tiptap/extensions';
import type { EditorView } from '@tiptap/pm/view';
import StarterKit from '@tiptap/starter-kit';
import { ySyncPluginKey } from '@tiptap/y-tiptap';
import * as Y from 'yjs';
import { formatState, type FormatState } from './formatting';
import { noteMeta, type NoteMeta } from './meta';
import { Photo, type PhotoAttrs } from './photo';
import { photoUrl } from './photo-src';
import { SlashCommand, slashItems } from './slash';

/** The Yjs field TipTap's collaboration extension and the Rust core share. */
export const CONTENT_FIELD = 'default';
/** Marks updates that came from the engine, so they aren't sent back to it. */
const FROM_ENGINE = 'mirrorleaf-engine';
/** Typing is sent to the engine at most this often, merged into one update. */
const SEND_EVERY_MS = 200;
/** The title and preview are worked out once typing pauses this long. */
const META_AFTER_MS = 500;

/** How the editor reaches the engine. The app uses `tauriBridge`; tests use fakes. */
export interface NoteBridge {
  /** Opens a note: resolves to its whole state, then calls `onRemote` with other devices' updates. */
  open(id: string, onRemote: (update: Uint8Array) => void): Promise<Uint8Array>;
  edit(id: string, update: Uint8Array): Promise<void>;
  close(): Promise<void>;
  /** Prepares a photo file in the engine, ready to place. */
  importPhoto(file: File): Promise<PhotoAttrs>;
  /**
   * Picks photos with the system's own picker and prepares them, where the
   * platform has one (Android); otherwise the editor shows a file dialog.
   */
  pickPhotos?(): Promise<PhotoAttrs[]>;
  /** The base address of the photo protocol. */
  photoBase(): string;
}

export interface OpenNoteOptions {
  id: string;
  element: HTMLElement;
  bridge: NoteBridge;
  /** False for a note in the trash. */
  editable?: boolean;
  /** The title or preview may have changed (local edits only). */
  onMeta?: (meta: NoteMeta) => void;
  /** The formatting where the cursor is, for the toolbar. */
  onFormat?: (state: FormatState) => void;
  onError?: (error: unknown) => void;
}

export interface NoteEditor {
  readonly id: string;
  readonly editor: Editor;
  pickPhotos(): void;
  insertPhotos(files: File[], at?: number): Promise<void>;
  /** Reloads a photo whose full image has just arrived. */
  refreshPhoto(fullHash: string): void;
  meta(): NoteMeta;
  /** Sends the edits still waiting to the engine now (the app is going to the background). */
  flush(): Promise<void>;
  /** Sends what's waiting, stops listening and frees the editor. */
  close(): Promise<void>;
}

const isImage = (file: File) => file.type.startsWith('image/') || /\.(heic|heif)$/i.test(file.name);

export async function openNote(options: OpenNoteOptions): Promise<NoteEditor> {
  const { id, element, bridge } = options;
  const onError = options.onError ?? ((error) => console.error(error));
  const doc = new Y.Doc();

  // Updates may arrive before the state does; Yjs holds any whose
  // predecessors are missing until they arrive.
  const state = await bridge.open(id, (update) => Y.applyUpdate(doc, update, FROM_ENGINE));
  Y.applyUpdate(doc, state, FROM_ENGINE);

  // Local updates, merged and sent in order.
  let queue: Uint8Array[] = [];
  let sendTimer: ReturnType<typeof setTimeout> | undefined;
  let sending: Promise<void> = Promise.resolve();
  const send = (): Promise<void> => {
    clearTimeout(sendTimer);
    sendTimer = undefined;
    if (queue.length > 0) {
      const merged = queue.length === 1 ? queue[0]! : Y.mergeUpdates(queue);
      queue = [];
      sending = sending.then(() => bridge.edit(id, merged)).catch(onError);
    }
    return sending;
  };
  doc.on('update', (update: Uint8Array, origin: unknown) => {
    if (origin === FROM_ENGINE) return;
    queue.push(update);
    sendTimer ??= setTimeout(send, SEND_EVERY_MS);
  });

  let metaTimer: ReturnType<typeof setTimeout> | undefined;
  let metaDue = false;
  const reportMeta = () => {
    clearTimeout(metaTimer);
    if (metaDue && !editor.isDestroyed) options.onMeta?.(noteMeta(editor.state.doc));
    metaDue = false;
  };

  const base = bridge.photoBase();
  /** Places a photo at `at`, at the cursor, or at the end with `'end'`. */
  const place = (photo: PhotoAttrs, at?: number | 'end') => {
    if (editor.isDestroyed) return;
    const chain = editor.chain().focus();
    if (at === 'end') chain.setTextSelection(editor.state.doc.content.size);
    else if (at !== undefined) chain.setTextSelection(at);
    chain.insertContent({ type: 'image', attrs: photo }).run();
    // A photo at the end gets a line after it to carry on typing.
    if (editor.state.doc.lastChild?.type.name === 'image') {
      editor.commands.insertContentAt(editor.state.doc.content.size, { type: 'paragraph' });
    }
  };
  const insertPhotos = async (files: File[], at?: number | 'end') => {
    for (const file of files.filter(isImage)) {
      try {
        place(await bridge.importPhoto(file), at);
        at = undefined;
      } catch (error) {
        onError(error);
      }
    }
  };
  const pickPhotos = () => {
    // Without a cursor in the note (it was never tapped), photos go at the
    // end rather than above the title.
    const where = editor.isFocused ? undefined : 'end';
    if (bridge.pickPhotos) {
      bridge
        .pickPhotos()
        .then((photos) => photos.forEach((photo) => place(photo, where)))
        .catch(onError);
      return;
    }
    const input = document.createElement('input');
    input.type = 'file';
    input.accept = 'image/*,.heic,.heif';
    input.multiple = true;
    input.addEventListener('change', () => void insertPhotos(Array.from(input.files ?? []), where));
    input.click();
  };
  const droppedImages = (files: FileList | undefined | null) =>
    Array.from(files ?? []).filter(isImage);

  const editor: Editor = new Editor({
    element,
    editable: options.editable ?? true,
    extensions: [
      StarterKit.configure({
        // Collaboration brings its own undo, which only undoes this device's edits.
        undoRedo: false,
        // It would add a paragraph on every device that opens a note ending in
        // a photo; photos get their line when they are placed instead.
        trailingNode: false,
        // A click on a link would navigate the app's own window away.
        link: { openOnClick: false, autolink: true },
      }),
      TaskList,
      TaskItem.configure({ nested: true }),
      Placeholder.configure({
        placeholder: ({ pos }) => (pos === 0 ? 'Title' : 'Type / for blocks and photos'),
      }),
      Collaboration.configure({ document: doc, field: CONTENT_FIELD }),
      Photo.configure({ url: (src) => photoUrl(base, src) }),
      SlashCommand.configure({ items: () => slashItems(pickPhotos) }),
    ],
    editorProps: {
      attributes: { class: 'note-body', spellcheck: 'true' },
      handleDrop: (view: EditorView, event: DragEvent, _slice, moved: boolean) => {
        const images = droppedImages(event.dataTransfer?.files);
        if (moved || images.length === 0) return false;
        event.preventDefault();
        const at = view.posAtCoords({ left: event.clientX, top: event.clientY })?.pos;
        void insertPhotos(images, at);
        return true;
      },
      handlePaste: (_view: EditorView, event: ClipboardEvent) => {
        const images = droppedImages(event.clipboardData?.files);
        if (images.length === 0) return false;
        void insertPhotos(images);
        return true;
      },
    },
    onUpdate: ({ transaction }) => {
      // Changes from other devices carry their own title and preview.
      if (transaction.getMeta(ySyncPluginKey)?.isChangeOrigin) return;
      metaDue = true;
      clearTimeout(metaTimer);
      metaTimer = setTimeout(reportMeta, META_AFTER_MS);
    },
    onTransaction: ({ editor }) => options.onFormat?.(formatState(editor)),
  });

  // When the on-screen keyboard opens, the view shrinks; keep the cursor in sight.
  const viewport = typeof window === 'undefined' ? undefined : window.visualViewport;
  const keepCursorInView = () => {
    if (!editor.isDestroyed && editor.isFocused) editor.commands.scrollIntoView();
  };
  viewport?.addEventListener('resize', keepCursorInView);

  return {
    id,
    editor,
    pickPhotos,
    insertPhotos,
    refreshPhoto(fullHash: string) {
      for (const img of element.querySelectorAll<HTMLImageElement>(
        `img[data-full="${fullHash}"]`,
      )) {
        img.src = photoUrl(base, img.dataset.src ?? '', Date.now());
      }
    },
    meta: () => noteMeta(editor.state.doc),
    flush() {
      reportMeta();
      return send();
    },
    async close() {
      viewport?.removeEventListener('resize', keepCursorInView);
      reportMeta();
      await send();
      editor.destroy();
      doc.destroy();
      await bridge.close();
    },
  };
}
