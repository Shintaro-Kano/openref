// OpenNote: 画像を大きく見る（読み表示で埋め込みの画像を押すと全画面の覆い）。状態はここ、描くのは WikiLightbox（App に 1 つ）。
// 開くのは描画部品（ShadowPage）: 押した画像と、同じページの画像の並び（リンクで包んだ画像は外す = リンクが勝つ）。
// 画像ごとの nav = その画像のファイルを新しいタブ（MediaView）で開く遷移（描画の root の外・data: 等は null = ボタンを出さない）

import type { WikiNav } from "./types";
import { assetPath, relInRoot } from "./render/asset";

export interface LightboxImage {
  src: string;
  alt: string;
  nav: WikiNav | null;
}

export interface Lightbox {
  images: LightboxImage[];
  index: number;
  /** 画像を新しいタブで開く遷移（押した描画部品の onNavigate） */
  onNavigate: (nav: WikiNav, newTab?: boolean) => unknown;
}

export const lightbox = $state({ cur: null as Lightbox | null });

export function openLightbox(l: Lightbox) {
  lightbox.cur = l;
}

export function closeLightbox() {
  lightbox.cur = null;
}

/** 前後の画像へ（端では止まる） */
export function stepLightbox(d: 1 | -1) {
  const c = lightbox.cur;
  if (!c) return;
  const i = c.index + d;
  if (i >= 0 && i < c.images.length) c.index = i;
}

/** img の asset URL → その画像のファイルを開く遷移（描画の root 相対のページ = navigate が自分のタブ = MediaView で開く）。root の外・asset でなければ null */
export function imageNav(src: string, root: string): WikiNav | null {
  const abs = assetPath(src);
  const rel = abs ? relInRoot(root, abs) : null;
  return rel ? { kind: "page", path: rel } : null;
}

/** 覆いに並べる画像（文書順・src のある img・リンクで包んだ物は外す） */
export function pageImages(root: ParentNode): HTMLImageElement[] {
  return Array.from(root.querySelectorAll<HTMLImageElement>("img[src]")).filter((i) => !i.closest("a"));
}
