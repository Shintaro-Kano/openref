/* ラベルの色（Paperpile の 25 スタイル = 背景 + 文字色 + 枠。New label の 5×5 グリッドの並び）。
   色値は Paperpile の配色に準拠。DB の tags.color には id を入れる
   （core::tags::LABEL_COLORS と id の並びを一致させる = cargo test `palette_ids_match_ui`）。
   solid = 濃い背景に淡い文字。それ以外（淡い背景に濃い文字・同系色の枠）はダークモードで暗い地に置き換える（app.css .lbl）。
   旧来の #rrggbb（以前の 8 色パレット等）は近いパレットのスタイルに読み替えて表示する（その色そのままの文字だと
   黄・橙・水色がライトモードで読めない）。 */

export interface LabelStyle {
  id: string;
  bg: string;
  fg: string;
  bd: string;
  solid: boolean;
}

export const PALETTE: LabelStyle[] = [
  { id: "gray", bg: "#e8e8e8", fg: "#575757", bd: "#cdcdcd", solid: false },
  { id: "sky", bg: "#d9e7fd", fg: "#1a5bf1", bd: "#1a5bf1", solid: false },
  { id: "periwinkle", bg: "#d8dbfd", fg: "#0201a4", bd: "#0201a4", solid: false },
  { id: "lilac", bg: "#dacdf7", fg: "#3a2086", bd: "#3a2086", solid: false },
  { id: "blush", bg: "#fce5f0", fg: "#6c4151", bd: "#a8828f", solid: false },
  { id: "rose", bg: "#fcdedd", fg: "#b40100", bd: "#b40100", solid: false },
  { id: "slate", bg: "#4c5d78", fg: "#e3ecf8", bd: "#4c5d78", solid: true },
  { id: "blue", bg: "#1a5efc", fg: "#e6f0fe", bd: "#1a5efc", solid: true },
  { id: "navy", bg: "#0000c4", fg: "#e3e7fe", bd: "#0000c4", solid: true },
  { id: "violet", bg: "#462298", fg: "#ece0fe", bd: "#462298", solid: true },
  { id: "plum", bg: "#764453", fg: "#fee6f3", bd: "#764453", solid: true },
  { id: "red", bg: "#c40000", fg: "#fee6e5", bd: "#c40000", solid: true },
  { id: "peach", bg: "#fdeddb", fg: "#e36100", bd: "#e36100", solid: false },
  { id: "apricot", bg: "#f7d4a8", fg: "#a35e00", bd: "#a35e00", solid: false },
  { id: "sand", bg: "#efe1a9", fg: "#9a7c00", bd: "#9a7c00", solid: false },
  { id: "cream", bg: "#fdfdcb", fg: "#535322", bd: "#535322", solid: false },
  { id: "lime", bg: "#f6fee9", fg: "#558a24", bd: "#558a24", solid: false },
  { id: "mint", bg: "#ecf3e7", fg: "#015428", bd: "#015428", solid: false },
  { id: "orange", bg: "#e66200", fg: "#fdf0e2", bd: "#e66200", solid: true },
  { id: "brown", bg: "#a85f00", fg: "#fae0bb", bd: "#a85f00", solid: true },
  { id: "mustard", bg: "#9f7e00", fg: "#f8e9bc", bd: "#9f7e00", solid: true },
  { id: "olive", bg: "#575728", fg: "#fefecf", bd: "#575728", solid: true },
  { id: "green", bg: "#578c25", fg: "#f7fdeb", bd: "#578c25", solid: true },
  { id: "forest", bg: "#00582a", fg: "#ecf5e7", bd: "#00582a", solid: true },
  { id: "steel", bg: "#d7e0ed", fg: "#4b586e", bd: "#4b586e", solid: false },
];

/** 新規ラベルの既定色（Paperpile と同じ灰） */
export const DEFAULT_LABEL_COLOR = "gray";

const byId = new Map(PALETTE.map((p) => [p.id, p]));

/** 以前の 8 色パレット（core の旧 PALETTE）→ 見た目の近いスタイル */
const LEGACY: Record<string, string> = {
  "#e53935": "rose",
  "#fb8c00": "peach",
  "#fdd835": "sand",
  "#43a047": "lime",
  "#00acc1": "mint",
  "#1e88e5": "sky",
  "#8e24aa": "lilac",
  "#6d4c41": "apricot",
};

function hsl(hex: string): [number, number] {
  const [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255);
  const mx = Math.max(r, g, b);
  const mn = Math.min(r, g, b);
  const d = mx - mn;
  if (d === 0) return [0, 0];
  const l = (mx + mn) / 2;
  const s = l > 0.5 ? d / (2 - mx - mn) : d / (mx + mn);
  const h = mx === r ? (g - b) / d + (g < b ? 6 : 0) : mx === g ? (b - r) / d + 2 : (r - g) / d + 4;
  return [h * 60, s];
}

/** 任意の #rrggbb → 色相の近い淡色スタイル（灰色に近ければ gray）。淡色だけを候補にして常に読める組にする */
function nearestId(hex: string): string {
  const [h, s] = hsl(hex);
  if (s < 0.15) return DEFAULT_LABEL_COLOR;
  let best = DEFAULT_LABEL_COLOR;
  let bestD = Infinity;
  for (const p of PALETTE) {
    if (p.solid) continue;
    const [h2, s2] = hsl(p.fg);
    if (s2 < 0.15) continue;
    const dh = Math.min(Math.abs(h - h2), 360 - Math.abs(h - h2)) / 180;
    const d = dh * 2 + Math.abs(s - s2) * 0.5;
    if (d < bestD) {
      bestD = d;
      best = p.id;
    }
  }
  return best;
}

/** tags.color（パレット id / 旧来の #rrggbb / null）→ 表示スタイル */
export function labelStyle(color: string | null | undefined): LabelStyle {
  if (color) {
    const p = byId.get(color);
    if (p) return p;
    if (/^#[0-9a-fA-F]{6}$/.test(color)) {
      return byId.get(LEGACY[color.toLowerCase()] ?? nearestId(color))!;
    }
  }
  return byId.get(DEFAULT_LABEL_COLOR)!;
}

/** バッジ要素に渡す CSS 変数（.lbl が --lb-bg / --lb-fg / --lb-bd を使う） */
export function labelVars(color: string | null | undefined): string {
  const s = labelStyle(color);
  return `--lb-bg:${s.bg};--lb-fg:${s.fg};--lb-bd:${s.bd}`;
}

export function isSolid(color: string | null | undefined): boolean {
  return labelStyle(color).solid;
}
