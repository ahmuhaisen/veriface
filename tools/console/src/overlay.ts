// Draws result geometry over the input image. All API geometry is in INPUT-image coordinates.
import type { Box, DocumentInfo, FaceInfo } from "./api";
import { h } from "./dom";

export interface OverlayData {
  face?: FaceInfo | null;
  document?: DocumentInfo | null;
  portraitSourceBox?: Box | null;
}

const COLORS = {
  face: "#22c55e",
  landmarks: "#f43f5e",
  document: "#3b82f6",
  portrait: "#f59e0b",
};
const LANDMARK_NAMES = ["R eye", "L eye", "nose", "R mouth", "L mouth"]; // subject's own right/left

export function overlay(imageUrl: string, data: OverlayData): HTMLElement {
  const canvas = h("canvas.overlay");
  const legend = h(
    "div.legend",
    null,
    key(COLORS.face, "face box"),
    key(COLORS.landmarks, "landmarks5 (1-5)"),
    key(COLORS.document, "document corners"),
    key(COLORS.portrait, "portrait source box"),
  );
  const wrap = h("div.overlay-wrap", null, canvas, legend);
  const img = new Image();
  img.onload = () => {
    canvas.width = img.naturalWidth;
    canvas.height = img.naturalHeight;
    const ctx = canvas.getContext("2d")!;
    ctx.drawImage(img, 0, 0);
    const lw = Math.max(2, Math.round(Math.max(img.naturalWidth, img.naturalHeight) / 400));
    ctx.lineWidth = lw;
    ctx.font = `${lw * 6}px sans-serif`;

    if (data.document?.corners?.length) {
      ctx.strokeStyle = COLORS.document;
      ctx.beginPath();
      data.document.corners.forEach(([x, y], i) => (i ? ctx.lineTo(x, y) : ctx.moveTo(x, y)));
      ctx.closePath();
      ctx.stroke();
      const [x0, y0] = data.document.corners[0]!;
      ctx.fillStyle = COLORS.document;
      ctx.fillText("TL", x0 + lw * 2, y0 + lw * 8);
    }
    if (data.portraitSourceBox) {
      const b = data.portraitSourceBox;
      ctx.strokeStyle = COLORS.portrait;
      ctx.setLineDash([lw * 4, lw * 3]);
      ctx.strokeRect(b.x, b.y, b.width, b.height);
      ctx.setLineDash([]);
    }
    if (data.face) {
      const b = data.face.box;
      ctx.strokeStyle = COLORS.face;
      ctx.strokeRect(b.x, b.y, b.width, b.height);
      ctx.fillStyle = COLORS.landmarks;
      data.face.landmarks5.forEach(([x, y], i) => {
        ctx.beginPath();
        ctx.arc(x, y, lw * 2.5, 0, Math.PI * 2);
        ctx.fill();
        ctx.fillText(String(i + 1), x + lw * 3, y - lw * 3);
      });
    }
  };
  img.src = imageUrl;
  canvas.title = data.face ? `landmarks5: ${LANDMARK_NAMES.map((n, i) => `${i + 1}=${n}`).join(", ")}` : "";
  return wrap;
}

function key(color: string, text: string): HTMLElement {
  return h("span.key", null, h("i", { style: { background: color } }), text);
}
