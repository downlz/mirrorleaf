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
  /** When the ledger last heard from it (Unix seconds). */
  last_seen: number | null;
  /** It runs a version this one can't sync with (E6). */
  needs_update: boolean;
  /** It announces itself on the same network as this device (M12). */
  on_this_network: boolean;
}

/** Android only (M12, E8): battery restrictions, the maker, and seeding. */
export interface Battery {
  unrestricted: boolean;
  /** Build.MANUFACTURER, lower case: "samsung", "xiaomi", "motorola"… */
  maker: string;
  seeding: boolean;
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
  /** A device that runs a version this one can't sync with, by name (E6). */
  update_needed: string | null;
  /** Changes no other device is known to hold yet (E6). */
  only_here: number;
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
  /** The folder holding it; null at the top level (M12). */
  parent: string | null;
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

/** Backups (E9): a setting of this device, plain by default. */
export interface BackupSettings {
  encrypt: boolean;
  /** "9 November 2026", if there has been a backup. */
  last_backup: string | null;
  /** The main screen's reminder, when one is due. */
  due: string | null;
}

/** What a backup file says about itself, before restoring it. */
export interface BackupInfo {
  made: string;
  encrypted: boolean;
  /** Encrypted with an earlier account key: restoring needs that kit's words. */
  needs_kit: boolean;
}

export interface PickedBackup {
  path: string;
  info: BackupInfo;
}

/** What a backup or a restore moved. */
export interface BackupDone {
  changes: number;
  photos: number;
}

/** The app lock (E4): off by default; the device's own unlock. */
export interface LockSettings {
  enabled: boolean;
  /** Minutes away before it locks: 0, 1, 5 or 15. */
  after_minutes: number;
  /** The device has an unlock the lock can use (a screen lock, a login password). */
  available: boolean;
}

export type EngineEvent =
  | { kind: 'notes_changed' }
  | { kind: 'photo_arrived'; hash: string }
  | { kind: 'status_changed' }
  | { kind: 'open_note'; id: string }
  | { kind: 'pairing_changed' }
  | { kind: 'lock_changed' };

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
  createFolder: (name: string, parent: string | null = null) =>
    invoke<string>('create_folder', { name, parent }),
  moveFolder: (id: string, parent: string | null) => invoke<void>('move_folder', { id, parent }),
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
  // Backups (M11, E9).
  backupSettings: () => invoke<BackupSettings>('backup_settings'),
  setBackupEncryption: (on: boolean) => invoke<void>('set_backup_encryption', { on }),
  snoozeBackup: () => invoke<void>('snooze_backup'),
  /** Writes a backup where the person chooses; null if they cancelled. */
  saveBackup: () => invoke<BackupDone | null>('save_backup'),
  /** Lets the person pick a backup to restore; null if they cancelled. */
  pickBackup: () => invoke<PickedBackup | null>('pick_backup'),
  restoreBackup: (path: string, words: string | null) =>
    invoke<BackupDone>('restore_backup', { path, words }),
  forgetBackup: (path: string) => invoke<void>('forget_backup', { path }),
  // The app lock (M11, E4).
  lockSettings: () => invoke<LockSettings>('lock_settings'),
  /** Asks the device's own unlock first; false if the person cancelled. */
  setLock: (enabled: boolean, afterMinutes: number) =>
    invoke<boolean>('set_lock', { enabled, afterMinutes }),
  isLocked: () => invoke<boolean>('is_locked'),
  screenLeft: () => invoke<void>('screen_left'),
  screenReturned: () => invoke<boolean>('screen_returned'),
  /** The device's own unlock; true once unlocked. */
  unlock: () => invoke<boolean>('unlock'),
  // Finding devices on the same network, the phone's battery (M12).
  localDiscovery: () => invoke<boolean>('local_discovery'),
  setLocalDiscovery: (on: boolean) => invoke<void>('set_local_discovery', { on }),
  /** Android's battery state; null on a desktop. */
  battery: () => invoke<Battery | null>('battery'),
  openAppSettings: () => invoke<void>('open_app_settings'),
  setSeeding: (on: boolean) => invoke<void>('set_seeding', { on }),
  // Everyday features (M12, E5).
  /** Notes matching every word in their title or text; snippets mark matches with \u0002 and \u0003. */
  searchNotes: (query: string) =>
    invoke<{ id: string; snippet: string }[]>('search_notes', { query }),
  /** Deletes a note for good, on every device. */
  deleteNote: (id: string) => invoke<void>('delete_note', { id }),
  /** Deletes every note in the trash for good; returns how many. */
  emptyTrash: () => invoke<number>('empty_trash'),
  // Files dropped on the window outside the editor (M12): photos are staged
  // one by one (raw bytes), then the note is made.
  stageDropped: (bytes: Uint8Array) => invoke<string>('stage_dropped', bytes),
  dropIntoNote: (text: string | null, photos: string[]) =>
    invoke<string>('drop_into_note', { text, photos }),
};

export function onEngineEvent(handler: (event: EngineEvent) => void): Promise<UnlistenFn> {
  return listen<EngineEvent>('engine', (event) => handler(event.payload));
}

/** Told before the window closes: send what's waiting, then call `api.closeWindow`. */
export function onWillClose(handler: () => void): Promise<UnlistenFn> {
  return listen('will-close', () => handler());
}
