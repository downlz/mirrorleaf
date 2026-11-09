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
  /** When the account's recovery kit was made (Unix seconds); null if it has none (E2). */
  kit_made_at: number | null;
  /** A kit is waiting to be saved. */
  kit_pending: boolean;
  /** This device recovered the account with its kit: a new kit is due. */
  kit_used: boolean;
  /** Another device removed this one from the account (E3). */
  removed: boolean;
}

/** The recovery kit waiting to be saved (E2). */
export interface KitView {
  words: string[];
  /** The account's short form, "48a02495". */
  account: string;
  /** "9 November 2026". */
  made: string;
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

/** What the inviting device shows (E1). */
export interface Invitation {
  /** The QR code as an SVG document, dark on white. */
  qr_svg: string;
  /** The text the QR code holds, for a device without a camera to paste. */
  qr_text: string;
  /** "7-acoustic-planet-river", when the device has a ledger. */
  words: string | null;
  expires_ms: number;
}

/** Where adding a device has got to, on either side (E1). */
export type PairingStatus =
  | { state: 'idle' }
  | { state: 'inviting'; invitation: Invitation }
  | { state: 'request'; name: string; platform: string; number: string }
  | { state: 'connecting' }
  | { state: 'confirm'; number: string }
  | { state: 'joined'; name: string }
  | { state: 'ended'; reason: string };

/** Android's two sync permissions; always granted on the desktop. */
export interface Permissions {
  notifications: boolean;
  nearby: boolean;
}

export type EngineEvent =
  | { kind: 'notes_changed' }
  | { kind: 'photo_arrived'; hash: string }
  | { kind: 'status_changed' }
  | { kind: 'open_note'; id: string }
  | { kind: 'pairing_changed' };

export const api = {
  platform: () => invoke<string>('platform'),
  status: () => invoke<EngineStatus>('status'),
  createAccount: () => invoke<string>('create_account'),
  useLedger: (url: string) => invoke<string>('use_ledger', { url }),
  setDeviceName: (name: string) => invoke<void>('set_device_name', { name }),
  invite: () => invoke<Invitation>('invite'),
  answerPairing: (allow: boolean) => invoke<void>('answer_pairing', { allow }),
  cancelPairing: () => invoke<void>('cancel_pairing'),
  pairingStatus: () => invoke<PairingStatus>('pairing_status'),
  joinWithCode: (code: string, ledger: string | null) =>
    invoke<string>('join_with_code', { code, ledger }),
  /** Android's code scanner; null if cancelled. */
  scanCode: () => invoke<string | null>('scan_code'),
  permissions: () => invoke<Permissions>('permissions'),
  requestPermissions: (which: ('notifications' | 'nearby')[]) =>
    invoke<Permissions>('request_permissions', { which }),
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
  // The account's lifecycle (M10).
  pendingKit: () => invoke<KitView | null>('pending_kit'),
  /** Saves the kit where the person chooses; false if they cancelled. */
  saveKit: (kind: 'pdf' | 'text') => invoke<boolean>('save_kit', { kind }),
  finishKit: () => invoke<void>('finish_kit'),
  replaceKey: (remove: string[]) => invoke<void>('replace_key', { remove }),
  removeDevice: (id: string) => invoke<void>('remove_device', { id }),
  onlyHere: () => invoke<number>('only_here'),
  /** Signs out and erases this device; the app then starts afresh. */
  signOut: (force: boolean) => invoke<void>('sign_out', { force }),
  recover: (code: string, ledger: string | null) => invoke<string>('recover', { code, ledger }),
};

export function onEngineEvent(handler: (event: EngineEvent) => void): Promise<UnlistenFn> {
  return listen<EngineEvent>('engine', (event) => handler(event.payload));
}

/** Told before the window closes: send what's waiting, then call `api.closeWindow`. */
export function onWillClose(handler: () => void): Promise<UnlistenFn> {
  return listen('will-close', () => handler());
}
