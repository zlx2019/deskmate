// Clipboard image reader that encodes raw RGBA data as PNG through canvas.
// Shared by the send-clipboard action and global hotkey, with images preferred.

import { readImage } from "@tauri-apps/plugin-clipboard-manager";

/** Clipboard screenshot payload containing PNG bytes and a suggested file name. */
export interface ClipboardImage {
  bytes: Uint8Array;
  name: string;
}

/** Pads a number to two digits. */
function pad(n: number): string {
  return String(n).padStart(2, "0");
}

/** Builds the receiver-side screenshot-YYYYMMDD-HHmmss.png file name. */
export function screenshotName(): string {
  const t = new Date();
  return `screenshot-${t.getFullYear()}${pad(t.getMonth() + 1)}${pad(t.getDate())}-${pad(t.getHours())}${pad(t.getMinutes())}${pad(t.getSeconds())}.png`;
}

/** Reads and encodes a clipboard image as PNG, returning null for non-images. */
export async function readClipboardImagePng(): Promise<ClipboardImage | null> {
  const img = await readImage().catch(() => null);
  if (!img) return null;
  try {
    const { width, height } = await img.size();
    const rgba = await img.rgba();
    if (width === 0 || height === 0 || rgba.length === 0) return null;
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    const ctx = canvas.getContext("2d");
    if (!ctx) return null;
    ctx.putImageData(new ImageData(new Uint8ClampedArray(rgba), width, height), 0, 0);
    const blob = await new Promise<Blob | null>((r) => canvas.toBlob(r, "image/png"));
    if (!blob) return null;
    const bytes = new Uint8Array(await blob.arrayBuffer());
    return { bytes, name: screenshotName() };
  } catch {
    return null;
  }
}
