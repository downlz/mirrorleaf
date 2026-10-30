// The TipTap editor and its Tauri bridge arrive in M5 (implementation plan §5.6).

/** URL scheme a note uses to reference a photo in the blob store (plan §5.1). */
export const BLOB_SCHEME = 'mirrorleaf-blob';

const BLAKE3_HEX = /^[0-9a-f]{64}$/;

/** Builds the reference a note stores for a photo, from its BLAKE3 hash in lower-case hex. */
export function blobRef(hash: string): string {
  if (!BLAKE3_HEX.test(hash)) {
    throw new Error(`not a BLAKE3 hash: ${hash}`);
  }
  return `${BLOB_SCHEME}://${hash}`;
}
