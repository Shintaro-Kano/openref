/* PDF viewer の純関数ヘルパ（DOM / Svelte / pdfjs 非依存 → node で単体検証可能: `node src/lib/viewer/util.test.mjs`）。
   ズーム刻み・回転角・幅フィット・文書内検索の index 計算・ページ索引・読み順・ブロック分割・柱検出をここに集約する。 */

export type ZoomMode = "auto" | "page" | "width" | "custom";

export const ZOOM_MIN = 50; // percent
export const ZOOM_MAX = 300; // percent
export const ZOOM_STEP = 25; // percent
export const AUTO_MAX = 1.25; // 自動モードの実効 ratio 上限（幅フィットのクリップ）
export const PAGE_PAD = 16; // .pages の片側 padding（app.css と一致させる）

/** 空白畳み込み + lowercase。drawAnnotations の既存 norm と完全に同一の正規化。 */
export function normText(s: string): string {
  // lowercase は buildFlat と同じく UTF-16 コード単位ごとに適用する。文字列全体の
  // toLowerCase() は語末シグマ規則（"ΟΣ"→"ος"）など文脈依存変換を行い、per-unit で
  // 構築する buildFlat の geom.flat と食い違ってハイライトが消える/ずれるため。
  return s
    .replace(/\s+/g, " ")
    .split("")
    .map((ch) => ch.toLowerCase())
    .join("");
}

/** パーセントを [ZOOM_MIN, ZOOM_MAX] にクランプ */
export function clampZoomPct(pct: number): number {
  return Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, Math.round(pct)));
}

/** 現在の実効 ratio から 25% グリッドへスナップし 1 段ズーム。返り値は新しい % 値。
    dir = 1（拡大）/ -1（縮小）。fit 由来の半端な倍率でも次/前の 25% 目盛りへ寄せる。 */
export function stepZoomPct(currentRatio: number, dir: 1 | -1): number {
  const cur = currentRatio * 100;
  const eps = 1e-6;
  let next: number;
  if (dir > 0) {
    next = Math.floor(cur / ZOOM_STEP + eps) * ZOOM_STEP + ZOOM_STEP;
  } else {
    next = Math.ceil(cur / ZOOM_STEP - eps) * ZOOM_STEP - ZOOM_STEP;
  }
  return clampZoomPct(next);
}

/** 0/90/180/270 へ正規化（負値・360 超も吸収） */
export function normRotation(deg: number): number {
  return (((deg % 360) + 360) % 360);
}

/** page 固有 rotate と userRotation を合成した絶対 rotation。
    pdf.js の getViewport({ rotation }) は絶対値で page.rotate を上書きするため、ここで合成する。 */
export function absRotation(pageRotate: number, userRotation: number): number {
  return normRotation(pageRotate + userRotation);
}

export interface FitInput {
  mode: ZoomMode;
  containerW: number; // scroller clientWidth
  containerH: number; // scroller clientHeight
  pageW: number; // 基準（scale=1・現 rotation）のページ幅
  pageH: number; // 同・高さ
  pad?: number; // 片側 padding（既定 PAGE_PAD）
  customPct?: number; // custom モード時の %
}

/** モードに応じた実効 scale ratio を返す。custom 以外は container 実寸から算出。 */
export function fitScale(inp: FitInput): number {
  if (inp.mode === "custom") return clampZoomPct(inp.customPct ?? 100) / 100;
  const pad = inp.pad ?? PAGE_PAD;
  const availW = Math.max(1, inp.containerW - pad * 2);
  const availH = Math.max(1, inp.containerH - pad * 2);
  if (inp.pageW <= 0 || inp.pageH <= 0) return 1;
  const wScale = availW / inp.pageW;
  if (inp.mode === "width") return wScale;
  if (inp.mode === "page") return Math.min(wScale, availH / inp.pageH);
  // auto: 幅フィットを AUTO_MAX で上限クリップ（小さいページを過拡大しない）
  return Math.min(wScale, AUTO_MAX);
}

export interface Match {
  page: number;
  occ: number; // そのページ内での 0-based 出現順（非重複）
}

/** 各ページの正規化済み full に対し normQuery の全出現を列挙。
    ページ昇順・ページ内は出現順。占有は非重複（i + query.length で前進）。
    highlight 側も同じ刻みでページ内 occ 番目を数えれば index が一致する。 */
export function buildMatches(
  pages: { page: number; full: string }[],
  normQuery: string
): Match[] {
  const out: Match[] = [];
  if (!normQuery) return out;
  const step = Math.max(1, normQuery.length);
  for (const p of pages) {
    let from = 0;
    let occ = 0;
    for (;;) {
      const i = p.full.indexOf(normQuery, from);
      if (i < 0) break;
      out.push({ page: p.page, occ });
      occ++;
      from = i + step;
    }
  }
  return out;
}

/** 正規化テキスト full 内の q の全出現（非重複・buildMatches と同じ前進刻み）の開始 index 列。
    スニペット生成用（nthMatchIndex を occ ごとに呼ぶと occ² になるため 1 走査で出す）。 */
export function allMatchIndexes(full: string, q: string): number[] {
  const out: number[] = [];
  if (!q) return out;
  const step = Math.max(1, q.length);
  let from = 0;
  for (;;) {
    const i = full.indexOf(q, from);
    if (i < 0) break;
    out.push(i);
    from = i + step;
  }
  return out;
}

/** 検索結果 1 件（サイドバー一覧用）。before/hit/after はページ元テキストからの前後文脈。 */
export interface SearchHit {
  page: number;
  occ: number;
  before: string;
  hit: string;
  after: string;
}

/** ヒット（正規化 full 上の [fullIdx, fullIdx+qLen)）の前後文脈スニペットを元テキスト raw から組む。
    posMap = buildFlat(raw).posMap（full index → raw index）。itemStarts = 各 item の raw 上の開始 offset
    （昇順）。item 境界で前の item 末尾が空白・ハイフンでなく次の先頭が英数字なら空白を 1 個補う
    （pdf.js の item 連結は行末空白を持たないことがあり "clinicalinformation" のように繋がるため。
    照合用の full/raw には触らず表示だけ補う）。改行は空白に畳む。 */
export function buildSnippet(
  raw: string,
  posMap: number[],
  itemStarts: number[],
  fullIdx: number,
  qLen: number,
  ctxBefore = 40,
  ctxAfter = 60
): { before: string; hit: string; after: string } {
  const rs = posMap[Math.min(fullIdx, posMap.length - 1)] ?? 0;
  const re = posMap[Math.min(fullIdx + qLen, posMap.length - 1)] ?? rs;
  const ws = Math.max(0, rs - ctxBefore);
  const we = Math.min(raw.length, re + ctxAfter);
  const sec = { before: "", hit: "", after: "" };
  let lastCh = "";
  const put = (pos: number, text: string) => {
    for (let i = 0; i < text.length; i++) {
      const p = pos + i;
      const ch = /\s/.test(text[i]) ? " " : text[i];
      if (p < rs) sec.before += ch;
      else if (p < re) sec.hit += ch;
      else sec.after += ch;
      lastCh = ch;
    }
  };
  // 窓 [ws, we) に重なる item を順に（itemStarts は昇順・最後の item の終端 = raw.length）
  let k = 0;
  while (k + 1 < itemStarts.length && itemStarts[k + 1] <= ws) k++;
  for (; k < itemStarts.length; k++) {
    const s0 = itemStarts[k];
    const e0 = k + 1 < itemStarts.length ? itemStarts[k + 1] : raw.length;
    if (s0 >= we) break;
    const s = Math.max(ws, s0);
    const e = Math.min(we, e0);
    if (e <= s) continue;
    const piece = raw.slice(s, e);
    if (s === s0 && s > ws && lastCh && !/[\s-]/.test(lastCh) && /[A-Za-z0-9\u00C0-\u024F]/.test(piece[0])) {
      // 境界の空白補い。ヒットの内側（item 跨ぎで一致した "patient"+"s"）には入れない（一致語を変えない）。
      // ヒット先頭に接する境界は before 側、末尾に接する境界は after 側へ
      if (s <= rs) sec.before += " ";
      else if (s >= re) sec.after += " ";
      lastCh = " ";
    }
    put(s, piece);
  }
  // 語の途中で切れた前後の端を落とし、省略記号を付ける
  let before = sec.before.replace(/\s+/g, " ");
  let after = sec.after.replace(/\s+/g, " ");
  if (ws > 0) before = "… " + before.replace(/^\S*\s/, "");
  if (we < raw.length) after = after.replace(/\s\S*$/, "") + " …";
  return { before, hit: sec.hit.replace(/\s+/g, " "), after };
}

/** 正規化テキスト flat 内で q の occ 番目（0-based・非重複）の開始 index を返す。無ければ -1。
    buildMatches と同じ前進刻みを使うことで DOM 側 index と対応が取れる。 */
export function nthMatchIndex(flat: string, q: string, occ: number): number {
  if (!q) return -1;
  const step = Math.max(1, q.length);
  let from = 0;
  let seen = 0;
  for (;;) {
    const i = flat.indexOf(q, from);
    if (i < 0) return -1;
    if (seen === occ) return i;
    seen++;
    from = i + step;
  }
}

// ---------- ハイライト幾何（位置ずれ根治） ----------
//
// 旧実装は「正規化 index → 元 index → DOM Range → getClientRects」で矩形を得ていた。
// が、pdf.js の item は行単位で長い（概ね 1 行 24〜37 字・長いものは 160 字超）ため
// 1 行 = 1 span となり、行の途中にある短いヒットの矩形は「span 全体を代替フォントの
// 字送りで測って scaleX で縮めた」ブラウザ補間に依存する。埋込フォントと代替フォントの
// 字送り差が接頭辞長ぶん累積し、数文字ぶんずれる（＋行境界で余白に幽霊矩形）。
//
// 根治: 矩形を PDF 自身の item 幾何（item.transform と item.width）から直接求める。
// フォント・ブラウザ非依存で、item 境界では厳密、item 内は等分近似。node で決定論検証可能。

/** flat（正規化）→ full（元テキスト）index の逆写像。
    normText と同じ規則（連続空白は先頭 1 個だけ残す・per-char lowercase）を単一走査で辿り、
    posMap[k] = flat[k] を生む full 内 index。posMap[flat.length] = full.length（排他終端用）。
    lowercase が字数を変える文字（例 'İ'）でも 1 文字に付き同一 orig を複数回積むので破綻しない。 */
export function buildFlat(full: string): { flat: string; posMap: number[] } {
  let flat = "";
  const posMap: number[] = [];
  let prevSpace = false;
  for (let i = 0; i < full.length; i++) {
    const ch = full[i];
    const isSpace = /\s/.test(ch);
    if (isSpace) {
      if (prevSpace) continue; // 連続空白の 2 個目以降 → flat には出さない
      posMap.push(i);
      flat += " ";
      prevSpace = true;
    } else {
      const lo = ch.toLowerCase();
      for (let k = 0; k < lo.length; k++) posMap.push(i);
      flat += lo;
      prevSpace = false;
    }
  }
  posMap.push(full.length);
  return { flat, posMap };
}

/** flat（buildFlat 済み・空白は単一 " "）から空白を除いた squash と、
    squash index → flat index の写像を返す。sqMap[squash.length] = flat.length（排他終端用）。
    注釈 quote の照合はこの空白除去空間で行う: 選択テキストは行境界に <br> 由来の改行
    （normText で空白化）を含む一方、geom.full は item.str の無区切り連結で行境界に空白を
    持たない PDF がある（実測: fink は hasEOL 88 行全てが行末空白なし）。空白を含む照合は
    行またぎ quote で構造的に不一致になるため、空白の有無自体を照合から排除する。 */
export function buildSquash(flat: string): { squash: string; sqMap: number[] } {
  let squash = "";
  const sqMap: number[] = [];
  for (let i = 0; i < flat.length; i++) {
    if (flat[i] === " ") continue;
    sqMap.push(i);
    squash += flat[i];
  }
  sqMap.push(flat.length);
  return { squash, sqMap };
}

export interface Rect {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** 1 item の描画幾何（device/viewport px・原点は左上）。
    ox,oy = ベースライン起点（tx[4],tx[5]）／(dx,dy)=字送り単位方向／fa=全字送り長／fh=フォント高。
    font = 実グリフ幅校正（advanceOffset）に使う fontFamily（textContent.styles[fontName].fontFamily）。
    不在なら校正は等分近似へフォールバックする。
    ascent/descent = styles のフォントメトリクス（em 比・descent は正値で保持）。fragmentRects の
    charBox モードが使う（塗りハイライト帯を文字ボックスへ合わせ descender 欠けを断つ）。 */
export interface GeomItem {
  str: string;
  start: number; // full 内の元 index
  ox: number;
  oy: number;
  dx: number;
  dy: number;
  fa: number;
  fh: number;
  font?: string;
  ascent?: number;
  descent?: number;
}

/** tx = Util.transform(viewport.transform, item.transform)（呼び出し側で pdfjs.Util を使い算出）と
    item.width・viewport.scale から純粋に GeomItem を組む（pdfjs 非依存 = node で検証可能）。
    ascent/descent は styles[fontName] の em 比（descent は負値で来る）。異常値に備え
    ascent は [0.5, 1.25]・descent は [0, 0.5] へクランプして保持する（不在は undefined のまま）。 */
export function makeGeomItem(
  str: string,
  start: number,
  tx: number[],
  itemWidth: number,
  viewportScale: number,
  font?: string,
  ascent?: number,
  descent?: number
): GeomItem {
  const hyp = Math.hypot(tx[0], tx[1]) || 1;
  const asc =
    typeof ascent === "number" && Number.isFinite(ascent) && ascent > 0
      ? Math.min(1.25, Math.max(0.5, ascent))
      : undefined;
  const dscRaw = typeof descent === "number" && Number.isFinite(descent) ? Math.abs(descent) : undefined;
  const dsc = dscRaw !== undefined ? Math.min(0.5, dscRaw) : undefined;
  return {
    str,
    start,
    ox: tx[4],
    oy: tx[5],
    dx: tx[0] / hyp,
    dy: tx[1] / hyp,
    fa: (itemWidth || 0) * viewportScale,
    fh: Math.hypot(tx[2], tx[3]),
    font,
    ascent: asc,
    descent: dsc,
  };
}

/** item の実グリフ幅を測る関数（DOM canvas measureText を注入する形。node では stub を渡す）。
    引数は (item, 対象文字列) で、item.font / item.fh からフォントを決めて幅を返す想定。 */
export type MeasureFn = (item: GeomItem, str: string) => number;

/** item 内の文字境界 k（0..str.length・UTF-16 単位）における「字送り方向オフセット（px, 0..fa）」を返す。
    校正: measure があり全長 measure>0 なら実グリフ幅比 measure(prefix)/measure(whole) で按分する。
    フォールバック: measure 不在／全長 0／非有限のときは等分近似 fa*(k/L) へ落ちる（旧挙動と一致）。
    相対比のみ使うので測定フォントと埋込フォントの絶対字送り差・font-load の揺れを構造的に受けない
    （字形プロポーションは概ねフォント間で保存される）。node で stub metrics により決定論検証可能。 */
export function advanceOffset(it: GeomItem, k: number, measure?: MeasureFn): number {
  const L = it.str.length;
  if (L <= 0) return 0;
  const kk = k <= 0 ? 0 : k >= L ? L : k;
  if (kk === 0) return 0;
  if (kk === L) return it.fa; // item 境界は厳密（校正しても whole/whole=1 で同値）
  if (measure) {
    const whole = measure(it, it.str);
    if (whole > 0 && Number.isFinite(whole)) {
      const pre = measure(it, it.str.slice(0, kk));
      if (Number.isFinite(pre) && pre >= 0) {
        const r = pre / whole;
        return it.fa * (r < 0 ? 0 : r > 1 ? 1 : r);
      }
    }
  }
  return it.fa * (kk / L); // 等分フォールバック
}

/** full 内の元 index 区間 [oStart, oEnd) に重なる各 item の矩形（AABB）を返す。
    item 内文字位置は advanceOffset で決める（measure 指定時は実グリフ幅比で校正、無指定なら等分近似）。
    回転・傾き item も 4 隅から AABB を取る。矩形は必ず item の字送り範囲内 → 余白に幽霊矩形は出ない。
    行（item）跨ぎは item ごとに 1 矩形。
    charBox=true は塗りハイライト用の文字ボックス帯: 上 = ascent·fh（不在は fh）・下 = descent·fh
    （不在は 0）。既定 false は従来帯 [baseline−fh, baseline]（下線=ベースライン・取消線=帯中央の
    意味を保つため underline/strikethrough はこちらを使い続ける）。 */
export function fragmentRects(
  items: GeomItem[],
  oStart: number,
  oEnd: number,
  measure?: MeasureFn,
  charBox = false
): Rect[] {
  const out: Rect[] = [];
  if (oEnd <= oStart) return out;
  for (const it of items) {
    const L = it.str.length;
    if (L <= 0 || it.fa <= 0) continue;
    const s = Math.max(oStart, it.start);
    const e = Math.min(oEnd, it.start + L);
    if (e <= s) continue; // 重ならない
    const off0 = advanceOffset(it, s - it.start, measure);
    const off1 = advanceOffset(it, e - it.start, measure);
    // ベースライン上の始点/終点（オフセットは字送り方向 (dx,dy) に沿う px 量）
    const sBx = it.ox + off0 * it.dx;
    const sBy = it.oy + off0 * it.dy;
    const eBx = it.ox + off1 * it.dx;
    const eBy = it.oy + off1 * it.dy;
    // 上方向（ascender 側）= 字送り方向を +90° 回した (dy, -dx)
    const ux = it.dy;
    const uy = -it.dx;
    const up = charBox && it.ascent ? it.ascent * it.fh : it.fh;
    const dn = charBox && it.descent ? it.descent * it.fh : 0;
    // 下端（descender 側）= ベースラインから dn だけ上方向の逆へ
    const sDx = sBx - dn * ux;
    const sDy = sBy - dn * uy;
    const eDx = eBx - dn * ux;
    const eDy = eBy - dn * uy;
    const sTx = sBx + up * ux;
    const sTy = sBy + up * uy;
    const eTx = eBx + up * ux;
    const eTy = eBy + up * uy;
    const left = Math.min(sDx, eDx, sTx, eTx);
    const right = Math.max(sDx, eDx, sTx, eTx);
    const top = Math.min(sDy, eDy, sTy, eTy);
    const bottom = Math.max(sDy, eDy, sTy, eTy);
    const width = right - left;
    const height = bottom - top;
    if (width <= 0 || height <= 0) continue;
    out.push({ left, top, width, height });
  }
  return out;
}

// ---------- 読み順ソート（2 カラム逸脱選択の根治） ----------
//
// 一部の PDF は本文テキストストリームを左右カラム「行単位で交互」に織り込む（左行→右行→左行…）。
// pdf.js のテキスト層 span はストリーム順に DOM へ並ぶため、ネイティブ選択（DOM 順の範囲）が
// 左カラム段落を選ぶと反対カラムの行断片まで巻き込む（保存 quote にも混入し得る）。
// 対策 = readTextContent が返す items を「視覚読み順」へ並べ替えてから全消費者（pageTexts /
// TextLayer / manualTextLayer / buildGeom / prefetch）へ流す。矩形描画は幾何直算なので順序不変。
//
// 設計は保守的（誤爆で表・1 カラム・図を壊さないことが最優先）:
//   - y でバンド化し、各バンドの内部ギャップから「ページ中央ガター G」をモード検出。
//   - G を跨がず左右どちらかに完全に属し、かつ列を埋める（fill）連続バンド区間だけを対象にする。
//   - さらに「元ストリームで左右が交互織り込み」の区間だけ並べ替える（左右スイッチ回数 S ≥ バンド数）。
//     既に左塊→右塊で整列済み（S が小）の区間は逸脱選択が起きないので元順を保つ（true no-op）。
//   - 中央を跨ぐ行（見出し・図キャプション・数式）や複数内部ギャップ（表）や短いセル（表）は
//     セクション境界として元順を保つ。カラム未検出なら配列を無変更で返す（true no-op）。
//   - 決定論・回転ページは呼び出し側で除外（v1 は rotation 0 のみ適用）。

/** 並べ替え入力の最小形。blank=true は空白のみ item（構造検出から除外し、出力では左側に随伴させる）。 */
export interface ReadItem {
  x: number; // ベースライン起点 x（PDF ユーザー空間・transform[4]）
  y: number; // 同 y（transform[5]。上が大）
  w: number; // 字送り幅（item.width）
  h: number; // フォント高（item.height）
  blank?: boolean;
}

function median(a: number[]): number {
  if (!a.length) return 0;
  const b = [...a].sort((x, y) => x - y);
  return b[b.length >> 1];
}

/** items（x/y/w/h・任意 blank）を視覚読み順へ並べ替えた**新しい配列**（同一要素参照）を返す。
    2 カラム本文と確信できる縦区間だけを左→右へ再配置し、それ以外は元順を厳密保持する。
    検出できなければ入力の shallow copy（＝順序不変）を返す。純関数・DOM 非依存・O(n·bands)。 */
export function orderItemsForReading<T extends ReadItem>(items: readonly T[]): T[] {
  const n = items.length;
  if (n < 8) return items.slice();

  type W = { it: T; i: number; x: number; y: number; w: number; h: number; blank: boolean };
  const idx: W[] = items.map((it, i) => ({
    it,
    i,
    x: +it.x,
    y: +it.y,
    w: +it.w,
    h: +it.h,
    blank: !!it.blank,
  }));
  const isStruct = (o: W) => o.w > 0 && !o.blank && Number.isFinite(o.x) && Number.isFinite(o.y);
  const ana = idx.filter(isStruct);
  if (ana.length < 8) return items.slice();

  const X0 = Math.min(...ana.map((o) => o.x));
  const X1 = Math.max(...ana.map((o) => o.x + o.w));
  const W = X1 - X0;
  if (!(W > 0)) return items.slice();

  const GAP = Math.max(12, W * 0.03); // カラム間ガターとみなす最小水平ギャップ
  const medH = median(ana.map((o) => o.h)) || 9;
  const tolY = Math.max(2, medH * 0.6); // 同一バンド（行帯）とみなす y 許容
  const eps = Math.max(1, medH * 0.15);
  const MIN_SEC = 3; // 並べ替える区間の最小カラムバンド数
  const MIN_G = 4; // ガター一致に必要な最小バンド数
  const FILL = 0.55; // カラム充填率の下限（本文行と短い表セルを分離）

  // 1. 全 item を y でバンド化（上→下）。
  const all = idx.slice().sort((a, b) => b.y - a.y || a.x - b.x || a.i - b.i);
  const bands: { y: number; items: W[]; type?: number; fillL?: number; fillR?: number }[] = [];
  let cur: { y: number; items: W[] } | null = null;
  for (const o of all) {
    if (!cur || Math.abs(o.y - cur.y) > tolY) {
      cur = { y: o.y, items: [o] };
      bands.push(cur);
    } else cur.items.push(o);
  }

  // 構造 item（空白除く）を x 昇順に見て GAP 超の内部ギャップ区間を返す。
  const interiorGaps = (list: W[]): [number, number][] => {
    const s = list.filter(isStruct).sort((a, b) => a.x - b.x);
    const gaps: [number, number][] = [];
    if (s.length < 2) return gaps;
    let cursor = s[0].x + s[0].w;
    for (let k = 1; k < s.length; k++) {
      const g = s[k].x - cursor;
      if (g > GAP) gaps.push([cursor, s[k].x]);
      cursor = Math.max(cursor, s[k].x + s[k].w);
    }
    return gaps;
  };

  // 2. モードガター G = 単一ギャップバンドのギャップ区間に最も多く含まれる x。
  const singleGap: [number, number][] = [];
  for (const b of bands) {
    const g = interiorGaps(b.items);
    if (g.length === 1) singleGap.push(g[0]);
  }
  if (singleGap.length < MIN_G) return items.slice();
  const cLo = X0 + 0.15 * W;
  const cHi = X0 + 0.85 * W;
  const cands: number[] = [];
  for (const [a, b] of singleGap) {
    const m = (a + b) / 2;
    if (m >= cLo && m <= cHi) cands.push(m);
  }
  if (!cands.length) return items.slice();
  let G = 0;
  let Gcount = -1;
  for (const x of cands) {
    let c = 0;
    for (const [a, b] of singleGap) if (x >= a && x <= b) c++;
    if (c > Gcount) {
      Gcount = c;
      G = x;
    }
  }
  if (Gcount < MIN_G || G < cLo || G > cHi) return items.slice();

  // 3. 各バンドを G で分類（構造 item のみで判定）。COLUMN バンドに充填率を付す。
  for (const b of bands) {
    const s = b.items.filter(isStruct);
    const left = s.filter((o) => o.x + o.w <= G + eps);
    const right = s.filter((o) => o.x >= G - eps);
    const straddle = s.some((o) => o.x < G - eps && o.x + o.w > G + eps);
    if (straddle || left.length === 0 || right.length === 0) {
      b.type = 0;
      continue;
    }
    // 左右どちらかの内部にさらにギャップ（＝サブカラム）があれば表とみなし境界化。
    if (interiorGaps(left).length > 0 || interiorGaps(right).length > 0) {
      b.type = 0;
      continue;
    }
    b.type = 1;
    const lS = Math.min(...left.map((o) => o.x));
    const lE = Math.max(...left.map((o) => o.x + o.w));
    const rS = Math.min(...right.map((o) => o.x));
    const rE = Math.max(...right.map((o) => o.x + o.w));
    b.fillL = (lE - lS) / (G - X0);
    b.fillR = (rE - rS) / (X1 - G);
  }

  // 4. 連続 COLUMN バンドを区間化。長さと充填率を満たす区間だけを並べ替えブロック化する。
  //    左右の振り分けは x 始点 vs G（空白/0 幅 item は始点で随伴側が決まる＝行末の空白は左に残る）。
  const sideLeft = (o: W) => o.x < G;
  const blocks: { members: Set<number>; ordered: W[] }[] = [];
  const memberBlock = new Map<number, { members: Set<number>; ordered: W[] }>();
  let sec: typeof bands = [];
  const consider = () => {
    if (!sec.length) return;
    const fL = median(sec.map((b) => b.fillL ?? 0));
    const fR = median(sec.map((b) => b.fillR ?? 0));
    // 既に「左ブロック→右ブロック」に整列済みの区間は並べ替え不要（＝逸脱選択が起きない）。
    // 元ストリーム順での左右スイッチ回数 S で判別する: 交互織り込みは S≈2·バンド数と大きく、
    // 整列済みは S≈2（左塊+右塊、ヘッダ等）と小さい。S < バンド数 の区間は真の no-op として保持し、
    // 既カラム順ページ（psma 等）で hasEOL 破棄や語頭連結（"theUnited"）の副作用を出さない。
    let switches = 0;
    {
      const seq = sec
        .flatMap((b) => b.items.filter(isStruct))
        .sort((a, b) => a.i - b.i)
        .map((o) => (sideLeft(o) ? 0 : 1));
      for (let k = 1; k < seq.length; k++) if (seq[k] !== seq[k - 1]) switches++;
    }
    if (sec.length >= MIN_SEC && fL >= FILL && fR >= FILL && switches >= sec.length) {
      const ordered: W[] = [];
      for (const b of sec) {
        const L = b.items.filter(sideLeft).sort((a, c) => a.x - c.x || a.i - c.i);
        for (const o of L) ordered.push(o);
      }
      for (const b of sec) {
        const R = b.items.filter((o) => !sideLeft(o)).sort((a, c) => a.x - c.x || a.i - c.i);
        for (const o of R) ordered.push(o);
      }
      const members = new Set(ordered.map((o) => o.i));
      const block = { members, ordered };
      blocks.push(block);
      for (const i of members) memberBlock.set(i, block);
    }
    sec = [];
  };
  for (const b of bands) {
    if (b.type === 1) sec.push(b);
    else consider();
  }
  consider();

  if (!blocks.length) return items.slice(); // 未検出 → true no-op

  // 5. 先頭出現位置に差し込み: 非メンバーは元ストリーム順を厳密保持し、並べ替えブロックは
  //    そのメンバーが最初に現れるストリーム位置へまとめて挿入する。
  const out: W[] = [];
  const emitted = new Set<{ members: Set<number>; ordered: W[] }>();
  for (const o of idx) {
    const blk = memberBlock.get(o.i);
    if (!blk) {
      out.push(o);
      continue;
    }
    if (emitted.has(blk)) continue;
    emitted.add(blk);
    for (const m of blk.ordered) out.push(m);
  }

  // 6. 安全網: 全 item を過不足なく保持できなければ元順にフォールバック。
  if (out.length !== n) return items.slice();
  const seen = new Set(out.map((o) => o.i));
  if (seen.size !== n) return items.slice();

  return out.map((o) => o.it);
}

// ---------- ページ索引・読み順・ブロック分割・柱検出（pdfjs / DOM 非依存） ----------
//
// PdfViewer は content（readTextContent の戻り）と GeomIndex を渡すだけ。クロージャ（docBodySize・scale・
// キャッシュ）は PdfViewer 側に残し、ここは引数で受ける。util.test.mjs が node で固定する。

/** ページのテキスト索引: raw = item.str 連結（geom.full と同じ）、full = normText(raw)（= buildFlat の flat・検索照合用）、
    posMap = full index → raw index、itemStarts = 各 item（str!==""）の raw 開始 offset（スニペットの語境界補い用） */
export type PageText = { content: any; full: string; raw: string; posMap: number[]; itemStarts: number[] };

export function itemsFull(content: any): string {
  let s = "";
  for (const it of content.items as any[]) s += it.str ?? "";
  return s;
}

export function makePageText(content: any): PageText {
  const raw = itemsFull(content);
  const { flat, posMap } = buildFlat(raw);
  const itemStarts: number[] = [];
  let o = 0;
  for (const it of (content?.items ?? []) as any[]) {
    const s = (it.str ?? "") as string;
    if (s === "") continue;
    itemStarts.push(o);
    o += s.length;
  }
  return { content, full: flat, raw, posMap, itemStarts };
}

// geom = そのページの描画ハイライト幾何。位置ずれ根治のため矩形は DOM Range/getClientRects では
// なく PDF item 幾何（item.transform + item.width）から算出する（フォント/scaleX/font-load 非依存）。
export type GeomIndex = {
  items: GeomItem[];
  full: string; // item.str 連結（= PageText.raw と同じ参照。描画のたびに作り直さない）
  flat: string; // normText 済み（= PageText.full と同じ参照）
  posMap: number[]; // flat index → full index（= PageText.posMap と同じ参照）
  squash: string; // flat から空白を除いた照合空間（行境界の空白有無差を quote 照合から排除）
  sqMap: number[]; // squash index → flat index
  eol: boolean[]; // items と並行: その item で行が終わる（hasEOL）。本文テキスト組立の改行位置
};

/** 2×3 アフィン行列の積 m1 × m2（pdfjs-dist 6 の Util.transform と同じ式） */
export function transform(m1: number[], m2: number[]): number[] {
  return [
    m1[0] * m2[0] + m1[2] * m2[1],
    m1[1] * m2[0] + m1[3] * m2[1],
    m1[0] * m2[2] + m1[2] * m2[3],
    m1[1] * m2[2] + m1[3] * m2[3],
    m1[0] * m2[4] + m1[2] * m2[5] + m1[4],
    m1[1] * m2[4] + m1[3] * m2[5] + m1[5],
  ];
}

/** ページ索引（PageText）と viewport（transform + scale）から描画幾何を組む。items は矩形算出に使う。
    full / flat / posMap は索引の raw / full / posMap をそのまま参照する（item.str 連結は raw と同じ文字列なので
    連結し直さず・buildFlat も掛け直さない = 描画中のページが文字列と posMap を 2 組持たない）。squash / sqMap は
    flat から 1 走査で作る（描画中のページだけが持つ）。pt 不在（テキスト層の取得失敗）は空の幾何。
    前提 = makePageText の後に content.items を書き換えない（item.str 連結 = pt.raw）。呼び出し側（PdfViewer.ensureRendered）が
    items の末尾 offset と raw の長さを突き合わせて診断に出す（長さだけの検査 = 同じ長さの並べ替えは見逃す）。
    矩形は item.transform + item.width から求めるので DOM Range/getClientRects/フォント読込に依存しない。 */
export function buildGeom(pt: PageText | undefined, viewport: { transform: number[]; scale: number }): GeomIndex {
  const items: GeomItem[] = [];
  const eol: boolean[] = [];
  let start = 0;
  const content = pt?.content;
  const styles = content?.styles;
  if (content?.items) {
    for (const it of content.items as any[]) {
      const s = (it.str ?? "") as string;
      if (s === "") continue; // 空 str は span を持たない（テキストノードなし）＝ full にも寄与しない
      eol.push(!!it.hasEOL);
      const tx = transform(viewport.transform, it.transform);
      // fontFamily は manualTextLayer と同じ流儀（styles[fontName].fontFamily）。
      // グリフ幅校正（advanceOffset）はこの font を測定に使い、行途中ヒットの横ずれを断つ。
      // ascent/descent は charBox 帯（塗りハイライトの descender 欠け解消）に使う。
      const st = styles?.[it.fontName];
      items.push(makeGeomItem(s, start, tx, it.width, viewport.scale, st?.fontFamily, st?.ascent, st?.descent));
      start += s.length;
    }
  }
  const full = pt?.raw ?? "";
  const flat = pt?.full ?? "";
  const posMap = pt?.posMap ?? [0];
  const { squash, sqMap } = buildSquash(flat);
  return { items, full, flat, posMap, squash, sqMap, eol };
}

/** ページ内のテキストブロック（連続する行のまとまり）。kind は選択の「途中」に含めるかの判定に使う:
    body = 本文（本文サイズ以上。見出しも含む）/ small = 本文より小さい文字（キャプション・表・脚注・所属）/
    caption = "Fig./Table N" で始まる本文未満サイズ / running = 柱・ノンブル・ロゴ（全 item が runningSet）/
    meta = 4 行以内で email・URL・DOI・Received/Accepted 等を含む（責任著者・脚注・投稿日。サイズが本文と同じでも外す） */
export type TextBlock = {
  first: number;
  last: number;
  start: number;
  end: number;
  fh: number;
  lines: number;
  kind: "body" | "small" | "caption" | "running" | "meta";
};

/** 2 カラム逸脱選択の根治（単一適用点）: items を視覚読み順へ並べ替えてから返す。これで
   全消費者（pageTexts.full / TextLayer / manualTextLayer / buildGeom / prefetch）が同一順序を共有する。
   - v1 は intrinsic rotation 0 のページのみ適用（回転ページは元順維持）。
   - 座標は PDF ユーザー空間（transform[4],[5] と item.width/height）。orderItemsForReading は
     「元ストリームで左右が行単位に交互織り込みされた」確実な 2 カラム縦区間だけを左→右へ再配置する。
     既に左塊→右塊で整列済みのページ（多くの雑誌 PDF）・表・見出し・1 カラムは真の no-op で元順維持。
   - changed（実際に順序が変わった）ときだけ hasEOL を破棄する。破棄の根拠は実測: ブラウザの
     getSelection().toString() は <br> を "\n" にする（"the"+<br>+"United" → "the\nUnited"）。
     一方 geom.full は item.str の単純連結で行境界に区切りを持たない蔵書がある（例 psma は行末に
     空白を持たず "theUnited"）。よって <br> を残すと選択 quote に余分な改行が入り geom.flat 照合が
     壊れる。hasEOL 破棄で選択文字列＝items 連結＝geom.full となり quote 照合が決定的に一致する
     （交互織り込みページの元コピーはそもそも混線しており、この破棄は改善方向）。<br> は描画専用で
     ハイライト幾何（transform 直算）には無関係。 */
export function applyReadingOrder(page: any, textContent: any) {
  if (((page?.rotate ?? 0) % 360) !== 0) return;
  const items = textContent.items;
  if (!Array.isArray(items) || items.length < 8) return;
  const desc = items.map((it: any) => ({
    x: it?.transform?.[4] ?? 0,
    y: it?.transform?.[5] ?? 0,
    w: it?.width ?? 0,
    h: it?.height ?? (it?.transform ? Math.hypot(it.transform[2], it.transform[3]) : 0),
    blank: !((it?.str ?? "") as string).trim(),
    it,
  }));
  const ordered = orderItemsForReading(desc);
  let changed = false;
  for (let k = 0; k < ordered.length; k++) {
    if (ordered[k].it !== items[k]) {
      changed = true;
      break;
    }
  }
  if (!changed) return; // 整列済み/表/1 カラム: 完全 no-op（hasEOL も保持）
  textContent.items = ordered.map((o) => o.it);
  for (const it of textContent.items) if (it) it.hasEOL = false;
}

/** geom.full の offset o を含む item の index（items は start 昇順・連続）。範囲外は端にクランプ。 */
export function itemIndexAt(geom: GeomIndex, o: number): number {
  const items = geom.items;
  let lo = 0;
  let hi = items.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (items[mid].start <= o) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}

/** item を幾何で「行」（baseline の差が 0.45 文字高以内。上付き文字も同じ行）にまとめ、連続する行を「ブロック」にまとめる
    （行送りが 0.5〜2.2 文字高・x 範囲が重なる = 同じ列・文字高の比が 0.88〜1.14）。ブロック種別は
    run（柱・ノンブル・ロゴの item index = runningIndexes。非空 item が全部 run なら running）/ 文字高（本文の 95% 未満 = small）/
    "Fig./Table N" 先頭 + 本文の 97% 未満（caption）/ 4 行以内で email・URL・DOI・投稿日を含む（meta）/ それ以外 body。
    bodyFh = 本文の文字高（viewport px・bodyFontPx）。ブロックの代表文字高は 0.25px 刻みに丸めた文字数重みの最頻値。
    回転ページでは行送り判定が粗くなるが、分割が細かくなる方向（= 本文サイズなら採用）なので害は小さい。
    PdfViewer の blocksOf(n, L) がページごとにキャッシュして呼ぶ。 */
export function splitBlocks(geom: GeomIndex, run: Set<number>, bodyFh: number): { blocks: TextBlock[]; itemBlock: number[] } {
  const items = geom.items;
  type Line = { items: number[]; oy: number; fh: number; x0: number; x1: number; chars: number };
  const lines: Line[] = [];
  let ln: Line | null = null;
  for (let i = 0; i < items.length; i++) {
    const it = items[i];
    const x0 = Math.min(it.ox, it.ox + it.fa * it.dx);
    const x1 = Math.max(it.ox, it.ox + it.fa * it.dx);
    const chars = it.str.trim().length;
    if (ln && Math.abs(it.oy - ln.oy) <= 0.45 * Math.max(it.fh, ln.fh)) {
      ln.items.push(i);
      ln.x0 = Math.min(ln.x0, x0);
      ln.x1 = Math.max(ln.x1, x1);
      if (chars > ln.chars) {
        ln.fh = it.fh; // 行の代表文字高 = 最も長い item のもの（上付き・記号に引きずられない）
        ln.chars = chars;
      }
    } else {
      ln = { items: [i], oy: it.oy, fh: it.fh, x0, x1, chars };
      lines.push(ln);
    }
  }
  const blocks: TextBlock[] = [];
  const fhCounts: Map<number, number>[] = []; // blocks と並行: 文字高 → 文字数
  const itemBlock: number[] = new Array(items.length).fill(0);
  let prev: Line | null = null;
  for (const line of lines) {
    let split = !prev || blocks.length === 0;
    if (!split && prev) {
      const pitch = line.oy - prev.oy; // viewport px・下が正
      const fhRef = Math.max(prev.fh, line.fh);
      const overlap = Math.min(line.x1, prev.x1) - Math.max(line.x0, prev.x0);
      const ratio = line.fh / (prev.fh || 1);
      // 文字高の差は ±12% で切る（実測: 見出し 12pt と本文 10pt = 比 0.83、本文 10pt とキャプション 8.5pt = 0.85 を分ける）
      split = pitch < 0.5 * fhRef || pitch > 2.2 * fhRef || overlap <= 0 || ratio < 0.88 || ratio > 1.14;
    }
    const first = line.items[0];
    const last = line.items[line.items.length - 1];
    if (split) {
      blocks.push({ first, last, start: items[first].start, end: items[last].start + items[last].str.length, fh: line.fh, lines: 1, kind: "body" });
      fhCounts.push(new Map());
    } else {
      const b = blocks[blocks.length - 1];
      b.last = last;
      b.end = items[last].start + items[last].str.length;
      b.lines++;
    }
    const fc = fhCounts[fhCounts.length - 1];
    for (const i of line.items) {
      itemBlock[i] = blocks.length - 1;
      const c = items[i].str.trim().length;
      if (!c) continue;
      const k = Math.round(items[i].fh * 4) / 4;
      fc.set(k, (fc.get(k) ?? 0) + c);
    }
    prev = line;
  }
  // ブロックの代表文字高 = 文字数重みの最頻値（先頭行の見出し・上付き・記号に引きずられない）
  for (let bi = 0; bi < blocks.length; bi++) {
    let best = blocks[bi].fh;
    let bestC = -1;
    for (const [k, c] of fhCounts[bi]) {
      if (c > bestC) {
        best = k;
        bestC = c;
      }
    }
    blocks[bi].fh = best;
  }
  const capRe = /^(fig(ure)?s?\.?|tab(le)?\.?|suppl(ementary)?\.?)\s*\d/i;
  // 責任著者・脚注・投稿日など「本文サイズでも本文でない」短いブロックの手掛かり
  const metaRe = /@[\w.-]+\.[a-z]{2,}|https?:\/\/|\bdoi\.org\b|\b(received|accepted|revised|published online)\s*:/i;
  for (const b of blocks) {
    let nonBlank = 0;
    let running = 0;
    for (let i = b.first; i <= b.last; i++) {
      if (!items[i].str.trim()) continue;
      nonBlank++;
      if (run.has(i)) running++;
    }
    const text0 = geom.full.slice(b.start, Math.min(b.end, b.start + 40)).trim();
    if (nonBlank > 0 && running === nonBlank) b.kind = "running";
    else if (b.fh < 0.95 * bodyFh) b.kind = "small"; // 本文 10pt なら 9.5pt 未満（図説・脚注の 8〜9pt を確実に外す）
    else if (b.fh < 0.97 * bodyFh && capRe.test(text0)) b.kind = "caption";
    else if (b.lines <= 4 && metaRe.test(geom.full.slice(b.start, b.end))) b.kind = "meta";
    else b.kind = "body";
  }
  return { blocks, itemBlock };
}

/** サイズ → 文字数 の集計から「シェア 20% 以上のうち最大のサイズ」を返す（無ければ最頻値）。 */
export function majorLargestSize(sizeCount: Map<number, number>, total: number): number | null {
  let best: number | null = null;
  let mode = 0;
  let modeC = -1;
  for (const [k, c] of sizeCount) {
    if (c > modeC) {
      mode = k;
      modeC = c;
    }
    if (total > 0 && c / total >= 0.2 && (best === null || k > best)) best = k;
  }
  return best ?? (mode > 0 ? mode : null);
}

/** 本文の文字高（viewport px）。文書全体の推定（docBodySize・prefetch 完了後）を優先し、
    無ければこのページで同じ規則（シェア 20% 以上の最大サイズ）。 */
export function bodyFontPx(geom: GeomIndex, docBodySize: number | null, scale: number): number {
  if (docBodySize) return docBodySize * scale;
  const count = new Map<number, number>();
  let total = 0;
  for (const it of geom.items) {
    const c = it.str.trim().length;
    if (!c) continue;
    const k = Math.round(it.fh * 4) / 4;
    count.set(k, (count.get(k) ?? 0) + c);
    total += c;
  }
  return majorLargestSize(count, total) || 1;
}

// 柱・ノンブル（running header / footer）検出:
// 「複数ページの同じ位置に同じ文字で出る item」を柱・ノンブル・出版社ロゴとみなす（PDF 本文抽出の定石）。
// key = 正規化文字列（小文字・数字→#・空白畳み）+ PDF ユーザー座標 y（丸め）。2 ページ以上に出れば running。
// 本文行が別ページの同じ y に同じ文字で現れることは実質なく、短い図ラベル等の偶然一致はページ境界の
// 先頭/末尾に来たときだけ効く。表は prefetch 完了時に 1 回構築（それまではトリムしない）。
export function runningKey(it: any): string | null {
  const s = (it?.str ?? "") as string;
  if (!s.trim()) return null;
  const norm = s.toLowerCase().replace(/\d/g, "#").replace(/\s+/g, " ").trim();
  const y = Math.round(it?.transform?.[5] ?? 0);
  return `${norm}|${y}`;
}

/** 全ページの item 列から柱表（key → 出現ページ数）と文書本文サイズを作る（PdfViewer.buildRunningTable の集計部）。
    bodySize = 文書全体で「文字数シェアが 20% 以上あるサイズのうち最大のもの」（0.25pt 刻み・文字数重み）。
    根拠: 本文は量があり、図説・脚注・表・参考文献は本文より必ず小さく、見出し・題名は本文より大きいが量がない。
    却下した案（実測）: 全体最頻値 → sample.pdf で参考文献・表に引きずられ 8.5pt / 各ページ最頻値の中央値 →
    Guja 2023（4 頁・図説と参考文献のページが 3/4）で 8.5pt になり、脚注・著者・所属（8.5pt）が本文扱いになった。 */
export function runningTable(pages: Iterable<any[]>): { counts: Map<string, number>; bodySize: number | null } {
  const counts = new Map<string, number>();
  const sizeCount = new Map<number, number>();
  let total = 0;
  for (const items of pages) {
    const seen = new Set<string>();
    for (const it of items) {
      const k = runningKey(it);
      if (k && !seen.has(k)) {
        seen.add(k);
        counts.set(k, (counts.get(k) ?? 0) + 1);
      }
      const str = (it.str ?? "") as string;
      const c = str.trim().length;
      const tr = it.transform;
      if (!c || !tr) continue;
      const sz = Math.round(Math.hypot(tr[2], tr[3]) * 4) / 4;
      if (!(sz > 0)) continue;
      sizeCount.set(sz, (sizeCount.get(sz) ?? 0) + c);
      total += c;
    }
  }
  return { counts, bodySize: majorLargestSize(sizeCount, total) };
}

/** 1 ページの item 列のうち柱表で 2 ページ以上に出るものの index 集合（geom.items と同じ番号付け = str==="" を飛ばす）。 */
export function runningIndexes(items: any[], counts: Map<string, number>): Set<number> {
  const s = new Set<number>();
  let gi = 0;
  for (const it of items) {
    if (((it.str ?? "") as string) === "") continue;
    const k = runningKey(it);
    if (k && (counts.get(k) ?? 0) >= 2) s.add(gi);
    gi++;
  }
  return s;
}

/** geom.full の [oStart, oEnd) を item 単位で組み立て、hasEOL の item 末尾で改行する
    （Selection.toString() が <br> を "\n" にするのと同じ読み味。item 連結に行末空白が無い PDF でも語が繋がらない） */
export function bodyTextOf(geom: GeomIndex, oStart: number, oEnd: number): string {
  let out = "";
  for (let i = 0; i < geom.items.length; i++) {
    const it = geom.items[i];
    const s = Math.max(oStart, it.start);
    const e = Math.min(oEnd, it.start + it.str.length);
    if (e <= s) continue;
    out += it.str.slice(s - it.start, e - it.start);
    if (geom.eol[i] && e === it.start + it.str.length) out += "\n";
  }
  return out;
}

// ---------- 注釈 v2 ----------

export type AnnotKind = "highlight" | "underline" | "strikethrough" | "note";

/** 注釈の色パレット。user 4 色 + purple（Claude 予約）。
    highlight は multiply 合成前提なので彩度高めの値を選ぶ。 */
export const ANNOT_PALETTE: Record<string, string> = {
  yellow: "#f5c518",
  green: "#34a853",
  blue: "#4285f4",
  red: "#ea4335",
  purple: "#a855f7", // Claude 予約（選択 UI には出さない）
};

/** ポップオーバーで user に提示する色（purple を除く 4 種）。既定は先頭の yellow。 */
export const USER_COLOR_CHOICES = ["yellow", "green", "blue", "red"] as const;

const AUTHOR_FALLBACK = { user: "#f5c518", claude: "#a855f7" };

/** a.color がパレットにあればその hex、無ければ author フォールバック（旧 colorOf 互換）。DOM 非依存。 */
export function colorForAnnotation(color: string | null | undefined, author: string): string {
  if (color && ANNOT_PALETTE[color]) return ANNOT_PALETTE[color];
  return author.startsWith("claude") ? AUTHOR_FALLBACK.claude : AUTHOR_FALLBACK.user;
}

// ---------- 目次 ----------

export interface RawOutlineNode {
  title: string;
  dest: any;
  items?: RawOutlineNode[];
  bold?: boolean;
  italic?: boolean;
}

export interface FlatOutlineRow {
  id: string; // 経路ベースの安定 id（"0/2/1"）
  depth: number;
  title: string;
  dest: any;
  hasChildren: boolean;
}

/** getOutline() の木を深さ優先で平坦化。collapsed に含まれる id 配下は展開しない。
    id は経路なので折りたたみ状態を安定に保持できる。DOM 非依存。 */
export function flattenOutline(
  items: RawOutlineNode[],
  collapsed: Set<string>,
  parentId = "",
  depth = 0,
  out: FlatOutlineRow[] = []
): FlatOutlineRow[] {
  if (!Array.isArray(items)) return out;
  items.forEach((it, i) => {
    const id = parentId ? `${parentId}/${i}` : String(i);
    const hasChildren = !!(it.items && it.items.length);
    out.push({ id, depth, title: it.title ?? "", dest: it.dest, hasChildren });
    if (hasChildren && !collapsed.has(id)) {
      flattenOutline(it.items!, collapsed, id, depth + 1, out);
    }
  });
  return out;
}

export interface PopoverPos {
  left: number;
  top: number;
  placement: "above" | "below";
}

/** 選択矩形（container 相対）の近傍にポップオーバーを収める。上に置けなければ下へ。
    left/top は container 相対でクランプ済み。DOM 非依存。 */
export function popoverPosition(
  sel: { left: number; top: number; right: number; bottom: number },
  containerW: number,
  containerH: number,
  popW: number,
  popH: number,
  gap = 8,
  pad = 6
): PopoverPos {
  const cx = (sel.left + sel.right) / 2;
  let left = Math.round(cx - popW / 2);
  left = Math.max(pad, Math.min(left, Math.max(pad, containerW - popW - pad)));
  let placement: "above" | "below" = "above";
  let top = Math.round(sel.top - popH - gap);
  if (top < pad) {
    top = Math.round(sel.bottom + gap);
    placement = "below";
  }
  top = Math.max(pad, Math.min(top, Math.max(pad, containerH - popH - pad)));
  return { left, top, placement };
}
