// The editor's bridge to the Rust engine through Tauri commands (plan §5.6).
// Yjs updates and photo files travel as raw bytes, not JSON.

import { Channel, convertFileSrc, invoke } from '@tauri-apps/api/core';
import type { NoteBridge } from './editor';
import type { PhotoAttrs } from './photo';
import { BLOB_SCHEME } from './photo-src';

/** The header naming the note an `edit_note` update belongs to. */
export const NOTE_HEADER = 'x-mirrorleaf-note';

export const tauriBridge: NoteBridge = {
  async open(id, onRemote) {
    const updates = new Channel<ArrayBuffer>();
    updates.onmessage = (buffer) => onRemote(new Uint8Array(buffer));
    const state = await invoke<ArrayBuffer>('open_note', { id, updates });
    return new Uint8Array(state);
  },
  edit: (id, update) => invoke('edit_note', update, { headers: { [NOTE_HEADER]: id } }),
  close: () => invoke('close_note'),
  async importPhoto(file) {
    const bytes = new Uint8Array(await file.arrayBuffer());
    return invoke<PhotoAttrs>('import_photo', bytes);
  },
  photoBase: () => convertFileSrc('', BLOB_SCHEME),
};
