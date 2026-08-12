// Clipboard image reader/writer converting between raw RGBA and PNG through
// canvas. Shared by the send-clipboard action, global hotkey, and the message
// stream's copy-image action.

import { Image } from "@tauri-apps/api/image";
import { readImage, writeImage } from "@tauri-apps/plugin-clipboard-manager";

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

/** Copies an image Blob URL to the system clipboard by decoding it back to
 * RGBA, mirroring readClipboardImagePng without native decode features. */
export async function writeClipboardImage(url: string): Promise<void> {
  const blob = await fetch(url).then((r) => r.blob());
  const bitmap = await createImageBitmap(blob);
  try {
    const canvas = document.createElement("canvas");
    canvas.width = bitmap.width;
    canvas.height = bitmap.height;
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("canvas 2d context unavailable");
    ctx.drawImage(bitmap, 0, 0);
    const rgba = ctx.getImageData(0, 0, bitmap.width, bitmap.height).data;
    await writeImage(await Image.new(new Uint8Array(rgba.buffer), bitmap.width, bitmap.height));
  } finally {
    bitmap.close();
  }
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
