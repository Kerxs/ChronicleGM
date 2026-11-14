// 图片缩放：上传前在界面里把图片缩到指定长边，减小世界文件与模型请求的体积。
export interface Resized {
  dataUrl: string;
  width: number;
  height: number;
}

export const IMAGE_TYPES = ["image/png", "image/jpeg", "image/webp", "image/gif"];

function load(file: Blob): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const url = URL.createObjectURL(file);
    const img = new Image();
    img.onload = () => {
      URL.revokeObjectURL(url);
      resolve(img);
    };
    img.onerror = () => {
      URL.revokeObjectURL(url);
      reject(new Error("无法读取这张图片"));
    };
    img.src = url;
  });
}

/**
 * 缩放到长边不超过 `maxSide`。PNG 保持 PNG（保留透明），其他格式输出 JPEG。
 * 已经足够小、且是支持的格式时直接用原文件。
 */
export async function resizeImage(file: Blob, maxSide: number, quality = 0.88, forceJpeg = false): Promise<Resized> {
  if (!IMAGE_TYPES.includes(file.type)) throw new Error("不支持的图片格式（支持 PNG、JPEG、WebP、GIF）");
  const img = await load(file);
  const scale = Math.min(1, maxSide / Math.max(img.naturalWidth, img.naturalHeight));
  const width = Math.max(1, Math.round(img.naturalWidth * scale));
  const height = Math.max(1, Math.round(img.naturalHeight * scale));
  const keepPng = file.type === "image/png" && !forceJpeg;
  if (scale === 1 && !forceJpeg && file.size < 3 * 1024 * 1024) {
    return { dataUrl: await blobToDataUrl(file), width, height };
  }
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const g = canvas.getContext("2d");
  if (!g) throw new Error("无法处理图片");
  if (!keepPng) {
    g.fillStyle = "#ffffff";
    g.fillRect(0, 0, width, height);
  }
  g.imageSmoothingQuality = "high";
  g.drawImage(img, 0, 0, width, height);
  return { dataUrl: canvas.toDataURL(keepPng ? "image/png" : "image/jpeg", quality), width, height };
}

export function blobToDataUrl(b: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(String(r.result));
    r.onerror = () => reject(new Error("无法读取文件"));
    r.readAsDataURL(b);
  });
}

/** 从拖放或粘贴事件里取出图片文件。 */
export function imageFiles(list: FileList | DataTransferItemList | null | undefined): File[] {
  if (!list) return [];
  const out: File[] = [];
  for (const it of Array.from(list as ArrayLike<File | DataTransferItem>)) {
    const f = it instanceof File ? it : it.kind === "file" ? it.getAsFile() : null;
    if (f && IMAGE_TYPES.includes(f.type)) out.push(f);
  }
  return out;
}
