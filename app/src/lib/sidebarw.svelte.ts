// 左の列（LIBRARY のサイドバー・NOTE の木・SHELF の本棚の木）の幅。境界のつまみ（SidebarGrip）で変える。
// 幅はモードごとに覚える（lib = LIBRARY・wiki = NOTE・shelf = SHELF。ノートを広げても蔵書の一覧が狭くならない）=
// 設定 ui_sidebar_w = {"lib": n, "wiki": n, "shelf": n}（set_ui_pref で書ける）。
// 前後の版の読み書き: 前の版の読みは lib / wiki だけを見て知らない鍵（shelf）を無視する = 新しい値を読んでも壊れない。
// 前の版が書くと {lib, wiki} だけになる（shelf が消える）= 新しい版はそれを読んで shelf を既定（230）にする = 壊れない（本棚の幅だけ戻る）。
// 本の読み手（ShelfViewer）は左の列を持たない（全面の読み手）= 対象外。
// 表示の幅は CSS が窓に合わせて詰める（app.css の main.sbw の --sb-col = 中央の最小幅と右ペインを残す）= 覚えた値は書き換えない。
// 壊れた JSON・範囲の外の値は既定（230）/ 範囲の端として読む（落ちない）。読む前につまみで変えた幅は、読んだ値で上書きしない。
// 設定を読めるまでは書かない（書くのは両モードの丸ごと = 読む前に書くと、触っていないモードの保存値を既定で消す）。読めなかった間の変更は、
// 次に読めたとき（次の変更の書き込みの前に読み直す）に 1 回書く

import { api } from "./api";
import { t } from "./i18n.svelte";
import { toast } from "./state.svelte";
import { errText } from "./err";

export type SidebarMode = "lib" | "wiki" | "shelf";
const MODES = ["lib", "wiki", "shelf"] as const;

export const SIDEBAR_KEY = "ui_sidebar_w";
export const SIDEBAR_DEFAULT = 230;
export const SIDEBAR_MIN = 180;
/** 覚える値の上限（窓に依らない端。表示はさらに窓で詰める） */
export const SIDEBAR_MAX = 1200;
/** 中央の列の最小幅（app.css の minmax(360px, 1fr) と同じ）と右ペインの幅（380px）。
    ここの 4 つ（SIDEBAR_DEFAULT・SIDEBAR_MIN・CENTER_MIN・RIGHT_W）は CSS の数字と対 = app.css の main の格子（740 = 360 + 380）・
    WikiPage の集中モードの .wk-sidebar / .wk-info の幅・SidebarGrip の --sb-col の既定。sidebarw.test の ① が照合する。
    チャットの柱の既定の幅（chat.ts の DOCK_W = 380）は別の量（柱は可変・ui_chat_dock）= ここに入れない */
export const CENTER_MIN = 360;
export const RIGHT_W = 380;

export const sidebarW = $state({ lib: SIDEBAR_DEFAULT, wiki: SIDEBAR_DEFAULT, shelf: SIDEBAR_DEFAULT });

/** 幅を範囲に詰める（max = 窓から決まる上限・無ければ SIDEBAR_MAX）。数でなければ既定 */
export function clampSidebarW(w: number, max = SIDEBAR_MAX): number {
  if (!Number.isFinite(w)) return SIDEBAR_DEFAULT;
  return Math.round(Math.max(SIDEBAR_MIN, Math.min(w, Math.max(SIDEBAR_MIN, max), SIDEBAR_MAX)));
}

/** ドラッグの上限 = 中央の最小幅と右ペイン（出しているとき）を残せる幅 */
export function dragMax(mainWidth: number, right: boolean): number {
  return mainWidth - CENTER_MIN - (right ? RIGHT_W : 0);
}

/** 設定の値 → 幅（形の違う物は既定） */
export function parseSidebarW(raw: string | null | undefined): Record<SidebarMode, number> {
  const out = { lib: SIDEBAR_DEFAULT, wiki: SIDEBAR_DEFAULT, shelf: SIDEBAR_DEFAULT };
  if (!raw) return out;
  let v: unknown;
  try {
    v = JSON.parse(raw);
  } catch {
    return out;
  }
  if (!v || typeof v !== "object" || Array.isArray(v)) return out;
  for (const m of MODES) {
    const n = (v as Record<string, unknown>)[m];
    if (typeof n === "number" && Number.isFinite(n)) out[m] = clampSidebarW(n);
  }
  return out;
}

const touched = new Set<SidebarMode>();
let loading: Promise<boolean> | null = null;
/** 読めなかった間に変えた幅がある（読めたら 1 回書く） */
let dirty = false;

/** 設定を 1 回だけ読む（起動時・書く前）。読めたら true。読めなければ false（既定のまま・知らせない = 表示の好み・次に呼ばれたら読み直す）。
    読む前につまみで変えたモード（touched）は、読んだ値で上書きしない */
export function loadSidebarW(): Promise<boolean> {
  loading ??= api.getUiPref(SIDEBAR_KEY).then(
    (raw) => {
      const v = parseSidebarW(raw);
      for (const m of MODES) if (!touched.has(m)) sidebarW[m] = v[m];
      if (dirty) void write();
      return true;
    },
    (e) => {
      loading = null;
      console.warn("ui_sidebar_w:", e);
      return false;
    }
  );
  return loading;
}

async function write() {
  dirty = false;
  try {
    await api.setUiPref(SIDEBAR_KEY, JSON.stringify({ lib: sidebarW.lib, wiki: sidebarW.wiki, shelf: sidebarW.shelf }));
  } catch (e) {
    toast(`✗ ${t("表示設定を保存できません")}: ${errText(e, 160)}`);
  }
}

/** 幅を確定して書く（同じ幅なら書かない・設定を読めるまでは書かずに覚えておく） */
export function setSidebarW(mode: SidebarMode, w: number, max?: number) {
  touched.add(mode);
  const nw = clampSidebarW(w, max);
  if (nw === sidebarW[mode]) return;
  sidebarW[mode] = nw;
  dirty = true;
  void loadSidebarW().then((ok) => {
    if (ok && dirty) void write();
  });
}
