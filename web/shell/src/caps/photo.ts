// A photo the person took or picked through WebKit (caps/webkit.ts), as
// the contract's `Device.Photo`: redrawn as a JPEG at most `PHOTO_MAX_SIDE`
// pixels on a side, as the app's `CameraPort` downscales (`image_picker`'s
// `maxWidth`/`maxHeight` 1600, quality 85).
import { CapError } from "./errors";

export const PHOTO_MAX_SIDE = 1600;
const QUALITY = 0.85;

/** `bytes` as base64. */
export function toBase64(bytes: Uint8Array): string {
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s);
}

/** The bytes of `b`, read with a `FileReader`. */
export function bytesOf(b: Blob): Promise<Uint8Array> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(new Uint8Array(r.result as ArrayBuffer));
    r.onerror = () => reject(new CapError("unavailable", "the file could not be read"));
    r.readAsArrayBuffer(b);
  });
}

export type ShellPhoto = { mime: string; data: string; width: number; height: number; bytes: number };

/** `f` decoded (in its EXIF orientation), scaled to fit `PHOTO_MAX_SIDE`
 * and encoded as JPEG; `unavailable` when WebKit cannot decode it. */
export async function photoOf(f: Blob): Promise<ShellPhoto> {
  let bitmap: ImageBitmap;
  try {
    bitmap = await createImageBitmap(f, { imageOrientation: "from-image" });
  } catch {
    throw new CapError("unavailable", "the photo could not be read");
  }
  try {
    const scale = Math.min(1, PHOTO_MAX_SIDE / Math.max(bitmap.width, bitmap.height));
    const width = Math.max(1, Math.round(bitmap.width * scale));
    const height = Math.max(1, Math.round(bitmap.height * scale));
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new CapError("unavailable", "the photo could not be drawn");
    ctx.drawImage(bitmap, 0, 0, width, height);
    const blob = await new Promise<Blob | null>(resolve => canvas.toBlob(resolve, "image/jpeg", QUALITY));
    if (!blob) throw new CapError("unavailable", "the photo could not be encoded");
    const bytes = await bytesOf(blob);
    return { mime: "image/jpeg", data: toBase64(bytes), width, height, bytes: bytes.length };
  } finally {
    bitmap.close();
  }
}
