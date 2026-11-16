// OpenNote: ページへのリンクのプレビュー（しばらく乗せると浮き窓）。状態と時計はここ、描くのは WikiPreview（App に 1 つ = ShadowPage の
// `contain: layout paint` の外）。乗せる側 = 本文のリンク（ShadowPage）・右ペインのバックリンク（WikiInfo）・検索の結果の行（WikiResults）・
// チャットの回答（ChatMd = ShadowPage）。浮き窓の中身は同じ ShadowPage（中のリンクは同じ規則で開く・中では次のプレビューを出さない）。
// 出る = 乗せてから openDelay（離れたら取り消す）・閉じる = 離れてから closeDelay（その間に浮き窓へ移れば保つ）/ Esc / 外側の押下 / スクロール 等（WikiPreview）。
// 待ち時間は自前の値（実機の手触りで変えてよい）。設定は作らない

import type { WikiNav, WikiPageData } from "./types";
import { linkMenu } from "./linkmenu.svelte";
import { navigate, opensInNewTab, readPage, wk } from "./wiki.svelte";
import { modalOpen } from "../state.svelte";

export interface PreviewReq {
  /** 描画の root 相対の md ページ。null = まだ無いページ（missing に書かれた名前） */
  path: string | null;
  /** `[[page#見出し]]` の見出し（本文）。開いたらそこへスクロール */
  heading: string | null;
  /** まだ無いページの名前（書かれたまま = `[[名前]]`） */
  missing?: string;
  /** 描画の root（ワークスペース） */
  root: string;
  refs?: string[];
  /** 浮き窓の中のリンク・題名のクリックの行き先（乗せた側の遷移と同じ） */
  onNavigate: (nav: WikiNav, newTab?: boolean) => unknown;
  readPage: (path: string) => Promise<WikiPageData>;
}

export interface Preview extends PreviewReq {
  /** 乗せた要素の矩形（viewport）= 浮き窓をその下（入らなければ上）に置く */
  rect: { left: number; top: number; right: number; bottom: number };
  /** 横の起点（乗せたときのポインタの x・無ければ矩形の左） */
  x: number;
  anchor: Element;
  /** 開くたびに進む（読みの取り違えを防ぐ） */
  seq: number;
}

/** 待ち時間（ms）。試験が縮める */
export const previewCfg = { openDelay: 450, closeDelay: 250 };

export const preview = $state({ cur: null as Preview | null });

let seq = 0;
let openTimer: ReturnType<typeof setTimeout> | undefined;
let closeTimer: ReturnType<typeof setTimeout> | undefined;
let pending: Element | null = null;

/** 要素に乗った。同じ要素の浮き窓が出ていれば保つ（閉じる時計を止める）・別の要素なら openDelay の後に出し直す
    （前の要素の浮き窓は離れたときの時計のまま閉じる = 次が出るまで古い物を残さない） */
export function hoverStart(anchor: Element, req: PreviewReq, x?: number) {
  if (preview.cur?.anchor === anchor) return cancelClose();
  if (pending === anchor) return;
  clearTimeout(openTimer);
  pending = anchor;
  openTimer = setTimeout(() => {
    pending = null;
    // 右クリックのメニュー・アプリのモーダル（引用の窓 等）が出ている・要素が消えた（描き直し・遷移）/ 見えない（集中モードで隠した右ペイン等）なら出さない
    if (linkMenu.cur || modalOpen() || !anchorVisible(anchor)) return;
    const r = anchor.getBoundingClientRect();
    preview.cur = { ...req, rect: { left: r.left, top: r.top, right: r.right, bottom: r.bottom }, x: x ?? r.left, anchor, seq: ++seq };
  }, previewCfg.openDelay);
}

/** 要素から離れた。出す前なら取り消す・出ていれば closeDelay の後に閉じる（その間に浮き窓へ移れば cancelClose） */
export function hoverEnd(anchor: Element) {
  if (pending === anchor) {
    clearTimeout(openTimer);
    pending = null;
  }
  if (preview.cur) scheduleClose();
}

/** 離れた後の時計で閉じる（今の浮き窓だけ = 移った先の要素の出す前の時計は止めない） */
export function scheduleClose() {
  clearTimeout(closeTimer);
  closeTimer = setTimeout(() => (preview.cur = null), previewCfg.closeDelay);
}

export function cancelClose() {
  clearTimeout(closeTimer);
}

/** 乗せた要素が見えているか。DOM から外れた・`display: none` / `visibility: hidden` の祖先を持つ（shadow root の host を辿る）・
    箱を持たない（getClientRects が空に当たる）なら見えない。checkVisibility があればそれ（WebKit = 平らな木で判定・visibility も見る）、
    無ければ（jsdom 等）計算済みのスタイルを祖先へたどる */
export function anchorVisible(el: Element): boolean {
  if (!el.isConnected) return false;
  const cv = (el as Element & { checkVisibility?: (o?: object) => boolean }).checkVisibility;
  // visibilityProperty の旧名 checkVisibilityCSS も渡す（WebKit の版が新しい名前を解さないと visibility: hidden を見ない）
  if (typeof cv === "function") return cv.call(el, { visibilityProperty: true, checkVisibilityCSS: true });
  for (let n: Node | null = el; n; n = n.parentNode ?? (n instanceof ShadowRoot ? n.host : null)) {
    if (!(n instanceof Element)) continue;
    const st = getComputedStyle(n);
    if (st.display === "none" || st.visibility === "hidden") return false;
  }
  return true;
}

/** 閉じる（出す前の時計も止める） */
export function closePreview() {
  clearTimeout(openTimer);
  clearTimeout(closeTimer);
  pending = null;
  preview.cur = null;
}

/** 画面のワークスペースのページ（右ペインのバックリンク・検索の結果の行）の要求。md 以外（自分のタブで開く物）・root が未定なら null */
export function workspaceReq(path: string): PreviewReq | null {
  const root = wk.status?.root;
  if (!root || opensInNewTab(path)) return null;
  return { path, heading: null, root, refs: wk.refDirs, onNavigate: navigate, readPage: (p) => readPage(p) };
}

/** 一覧の行（ShadowPage の外の素の要素）に乗った / 離れた。ポインタの x を起点に */
export function hoverRow(e: MouseEvent, path: string) {
  const el = e.currentTarget as Element;
  if (e.type === "mouseleave") return hoverEnd(el);
  if (e.buttons) return;
  const req = workspaceReq(path);
  if (req) hoverStart(el, req, e.clientX);
}
