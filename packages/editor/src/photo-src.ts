// How notes refer to photos (plan §5.1, ADR-003), and how the screen loads them
// through the app's `mirrorleaf-blob` protocol (plan §5.6).

/** URL scheme a note uses to reference a photo in the blob store. */
export const BLOB_SCHEME = 'mirrorleaf-blob';

const BLAKE3_HEX = /^[0-9a-f]{64}$/;
const PHOTO_SRC = /^mirrorleaf-blob:\/\/([0-9a-f]{64})\?thumb=([0-9a-f]{64})$/;

/** Builds the reference a note stores for a blob, from its BLAKE3 hash in lower-case hex. */
export function blobRef(hash: string): string {
  if (!BLAKE3_HEX.test(hash)) {
    throw new Error(`not a BLAKE3 hash: ${hash}`);
  }
  return `${BLOB_SCHEME}://${hash}`;
}

/** A photo's full image and thumbnail hashes. */
export interface PhotoHashes {
  full: string;
  thumbnail: string;
}

/** Reads `mirrorleaf-blob://<full>?thumb=<thumbnail>`, as the Rust core writes it. */
export function parsePhotoSrc(src: string): PhotoHashes | null {
  const match = PHOTO_SRC.exec(src);
  return match ? { full: match[1]!, thumbnail: match[2]! } : null;
}

/** The address of a photo through the app's protocol, given the protocol's base URL. */
export function photoUrl(base: string, src: string, version = 0): string {
  const photo = parsePhotoSrc(src);
  if (!photo) return '';
  const url = `${base.replace(/\/$/, '')}/${photo.full}/${photo.thumbnail}`;
  return version ? `${url}?v=${version}` : url;
}
