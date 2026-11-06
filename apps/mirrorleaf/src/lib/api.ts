// The engine's commands, typed (apps/mirrorleaf/src-tauri/src/commands.rs).

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export interface DeviceSummary {
  id: string;
  name: string;
  this_device: boolean;
  changes_held: number;
  online: boolean | null;
  behind: number | null;
  connection: string | null;
}

export interface EngineStatus {
  device_id: string;
  device_name: string;
  account: string | null;
  devices: DeviceSummary[];
  notes: number;
  changes: number;
  ledger_url: string | null;
  has_push_token: boolean;
  notifications: boolean;
  live: boolean;
}

export interface NoteItem {
  id: string;
  title: string;
  preview: string;
  updated_ms: number;
  folder: string | null;
  pinned: boolean;
  trashed: boolean;
}

export interface FolderItem {
  id: string;
  name: string;
}

export interface NoteList {
  notes: NoteItem[];
  folders: FolderItem[];
}

export type EngineEvent =
  | { kind: 'notes_changed' }
  | { kind: 'photo_arrived'; hash: string }
  | { kind: 'status_changed' }
  | { kind: 'open_note'; id: string };

export const api = {
  platform: () => invoke<string>('platform'),
  status: () => invoke<EngineStatus>('status'),
  createAccount: () => invoke<string>('create_account'),
  ticket: () => invoke<string>('ticket'),
  join: (ticket: string) => invoke<string>('join', { ticket }),
  useLedger: (url: string) => invoke<string>('use_ledger', { url }),
  fallbackPrompt: () => invoke<string | null>('fallback_prompt'),
  noteList: () => invoke<NoteList>('note_list'),
  createNote: (folder: string | null, title: string | null = null) =>
    invoke<string>('create_note', { folder, title }),
  trashNote: (id: string) => invoke<void>('trash_note', { id }),
  restoreNote: (id: string) => invoke<void>('restore_note', { id }),
  setPinned: (id: string, pinned: boolean) => invoke<void>('set_pinned', { id, pinned }),
  moveNote: (id: string, folder: string | null) => invoke<void>('move_note', { id, folder }),
  createFolder: (name: string) => invoke<string>('create_folder', { name }),
  renameFolder: (id: string, name: string) => invoke<void>('rename_folder', { id, name }),
  deleteFolder: (id: string) => invoke<void>('delete_folder', { id }),
  setNoteMeta: (id: string, title: string, preview: string) =>
    invoke<void>('set_note_meta', { id, title, preview }),
  flushEdits: () => invoke<void>('flush_edits'),
  closeWindow: () => invoke<void>('close_window'),
  takePendingOpen: () => invoke<string | null>('take_pending_open'),
  startupDone: (what: string) => invoke<void>('startup_done', { what }),
  autostart: () => invoke<boolean>('autostart'),
  setAutostart: (enabled: boolean) => invoke<void>('set_autostart', { enabled }),
};

export function onEngineEvent(handler: (event: EngineEvent) => void): Promise<UnlistenFn> {
  return listen<EngineEvent>('engine', (event) => handler(event.payload));
}

/** Told before the window closes: send what's waiting, then call `api.closeWindow`. */
export function onWillClose(handler: () => void): Promise<UnlistenFn> {
  return listen('will-close', () => handler());
}
