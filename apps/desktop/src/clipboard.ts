// 剪贴板图片读取: RGBA 原始数据经 canvas 编码为 PNG(零额外依赖)
// 供"发送剪贴板"按钮与全局快捷键共用 —— 剪贴板里有截图时优先发图

import { readImage } from "@tauri-apps/plugin-clipboard-manager";

/** 剪贴板截图载荷: PNG 字节 + 建议文件名 */
export interface ClipboardImage {
  bytes: number[];
  name: string;
}

/** 两位补零 */
function pad(n: number): string {
  return String(n).padStart(2, "0");
}

/** 截图文件名: screenshot-YYYYMMDD-HHmmss.png(接收端以此落盘) */
export function screenshotName(): string {
  const t = new Date();
  return `screenshot-${t.getFullYear()}${pad(t.getMonth() + 1)}${pad(t.getDate())}-${pad(t.getHours())}${pad(t.getMinutes())}${pad(t.getSeconds())}.png`;
}

/** 读取剪贴板图片并编码为 PNG; 剪贴板不是图片时返回 null */
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
    const bytes = Array.from(new Uint8Array(await blob.arrayBuffer()));
    return { bytes, name: screenshotName() };
  } catch {
    return null;
  }
}
