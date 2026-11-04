import { describe, expect, it } from 'vitest';
import { blobRef, parsePhotoSrc, photoUrl } from './photo-src';

const FULL = 'af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262';
const THUMB = '0'.repeat(64);
const SRC = `mirrorleaf-blob://${FULL}?thumb=${THUMB}`;

describe('blobRef', () => {
  it('builds a mirrorleaf-blob reference from a BLAKE3 hash', () => {
    expect(blobRef(FULL)).toBe(`mirrorleaf-blob://${FULL}`);
  });

  it.each([
    ['too short', FULL.slice(1)],
    ['upper-case hex', FULL.toUpperCase()],
    ['not hex', 'z'.repeat(64)],
    ['empty', ''],
  ])('rejects a hash that is %s', (_case, hash) => {
    expect(() => blobRef(hash)).toThrow('not a BLAKE3 hash');
  });
});

describe('photo addresses', () => {
  it('reads the reference the Rust core writes', () => {
    expect(parsePhotoSrc(SRC)).toEqual({ full: FULL, thumbnail: THUMB });
    expect(parsePhotoSrc(`${SRC}x`)).toBeNull();
    expect(parsePhotoSrc('https://example.com/a.jpg')).toBeNull();
  });

  it('turns a reference into the protocol address on each platform', () => {
    expect(photoUrl('mirrorleaf-blob://localhost/', SRC)).toBe(
      `mirrorleaf-blob://localhost/${FULL}/${THUMB}`,
    );
    expect(photoUrl('http://mirrorleaf-blob.localhost/', SRC, 7)).toBe(
      `http://mirrorleaf-blob.localhost/${FULL}/${THUMB}?v=7`,
    );
    expect(photoUrl('mirrorleaf-blob://localhost/', 'nonsense')).toBe('');
  });
});
