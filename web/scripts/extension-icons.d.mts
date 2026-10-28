/** The light shell's `--bg`, behind the app icons (extension-icons.mjs). */
export const APP_BACKGROUND: [number, number, number];
/** The mark as a `size`×`size` PNG (extension-icons.mjs). */
export function drawMark(size: number, options?: { fill?: number; background?: [number, number, number] | null }): Buffer;
/** The extension's icons, written into `dir`. */
export function drawIcons(dir: string): void;
/** The installed web app's icons, by file name. */
export function appIcons(): Record<string, Buffer>;
