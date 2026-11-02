/* OpenNote: `<style>` の中身と style 属性の自前の掃除（DOMPurify は `<style>` の中を見ない）。
   保証の水準は「既知の経路を塞ぐ」（外部読込ゼロの証明ではない）:
   1. CSS エスケープを解く（`\75 rl(` → `url(`・`\5c` が作る 2 段目のエスケープも解けなくなるまで）・残った `\` は消す・コメントは消す
   2. `@import` を規則ごと除去（`@import` の後ろの `;` まで）
   3. `url(…)` / `src(…)` は中身が asset: か data:image/ か（rewrite で asset に書き換えられる）相対パスのときだけ残し、ほかは `none` に。
      asset: は呼び手の asset（vault の中か = img の src と同じ assetInRoot）が URL を返したときだけ残す（無ければ none）
   4. `image-set()` `-webkit-image-set()` `image()` `cross-fade()` は名前を壊して無効化（url() を使わず文字列で読みに行ける）
   5. セレクタに `:host`（`:host(…)` `:host-context(…)` も）を含む規則を除去（shadow の外の箱を動かさない）
   結果は解いた後の文字列（ブラウザが解く前と同じ意味）。DOM に依存しない純関数 */

/** CSS のエスケープを 1 段解く。`\` + 16 進 1〜6 桁 + 空白 1 つ → その文字・`\` + 改行 → 消す・`\` + 他 → その文字 */
function unescapeOnce(s: string): string {
  return s.replace(/\\(?:([0-9a-fA-F]{1,6})[ \t\n\r\f]?|(\r\n|[\n\r\f])|([\s\S]))/g, (_m, hex, nl, ch) => {
    if (hex) {
      const cp = parseInt(hex, 16);
      return cp === 0 || cp > 0x10ffff || (cp >= 0xd800 && cp <= 0xdfff) ? "�" : String.fromCodePoint(cp);
    }
    if (nl) return "";
    return ch ?? "";
  });
}

export function decodeCss(s: string): string {
  let cur = s;
  for (let i = 0; i < 8 && cur.includes("\\"); i++) cur = unescapeOnce(cur);
  return cur.replace(/\\/g, "");
}

/** url() の中身として残してよいか。relOk = 相対パスを許す（後で asset URL に書き換える呼び手） */
export function cssUrlAllowed(v: string, relOk: boolean): boolean {
  const t = v.trim();
  if (/^asset:/i.test(t)) return true;
  if (/^data:image\//i.test(t)) return true;
  if (!relOk) return false;
  // 相対 = scheme なし・`//`（protocol-relative）でない
  return t !== "" && !/^[a-z][a-z0-9+.-]*:/i.test(t) && !t.startsWith("//") && !t.startsWith("#");
}

/** 解いた後の CSS 文字列から url()/src() の値を走査して、許されない物を none に。rewrite で相対の書換え・asset で asset: の検査もここで行う */
function fixUrls(css: string, rewrite?: (v: string) => string | null, asset?: (v: string) => string | null): string {
  const out = css.replace(/(^|[^\w-])(url|src)\(\s*(?:"([^"]*)"|'([^']*)'|([^)]*?))\s*(?:\)|$)/gi, (_m, pre, _fn, d, s, bare) => {
    const v = (d ?? s ?? bare ?? "").trim();
    if (!cssUrlAllowed(v, !!rewrite)) return `${pre}none`;
    let r: string | null = v;
    if (/^asset:/i.test(v)) r = asset ? asset(v) : null;
    else if (rewrite && !/^data:/i.test(v)) r = rewrite(v);
    return r ? `${pre}url("${r.replace(/["\\\n]/g, "")}")` : `${pre}none`;
  });
  // 取りこぼし（閉じない文字列・入れ子の括弧）: asset / data:image で始まる形に直した物以外の url( src( は壊す
  return out.replace(/(^|[^\w-])(url|src)\((?!"(?:asset:|data:image\/))/gi, "$1wk-blocked(");
}

/** 入れ子の深さの上限（超えたら呼び手がその <style> を丸ごと捨てる） */
export const MAX_CSS_DEPTH = 32;

/** 文字列リテラルとブロックを意識して規則に分け、`:host` を含む規則と `@import` を落とす */
function dropRules(css: string, level = 0): string {
  if (level > MAX_CSS_DEPTH) throw new Error("css: too deep");
  let out = "";
  let i = 0;
  const n = css.length;
  while (i < n) {
    // 1 つの規則の前置き（セレクタ / at-rule の頭）を `{` か `;` まで読む（文字列の中は飛ばす）
    let j = i;
    let q: string | null = null;
    while (j < n) {
      const c = css[j];
      if (q) {
        if (c === q) q = null;
      } else if (c === '"' || c === "'") q = c;
      else if (c === "{" || c === ";" || c === "}") break;
      j++;
    }
    const prelude = css.slice(i, j);
    const c = css[j];
    if (c === ";" || j >= n) {
      // at-rule の文（@import / @charset / @namespace）か、ブロックの外に漏れた宣言
      if (!/@import/i.test(prelude)) out += prelude + (c === ";" ? ";" : "");
      i = j + 1;
      continue;
    }
    if (c === "}") {
      // 対応の無い `}`（入れ子の終わりは呼び手が切り出すのでここに来るのは壊れた CSS）
      out += prelude;
      i = j + 1;
      continue;
    }
    // ブロック `{ … }` の終わり（入れ子・文字列を数える）
    let k = j + 1;
    let depth = 1;
    q = null;
    while (k < n && depth > 0) {
      const d = css[k];
      if (q) {
        if (d === q) q = null;
      } else if (d === '"' || d === "'") q = d;
      else if (d === "{") depth++;
      else if (d === "}") depth--;
      k++;
    }
    const body = css.slice(j + 1, depth === 0 ? k - 1 : k);
    const p = prelude.trim();
    if (/:host/i.test(p) || /@import/i.test(p)) {
      // 規則ごと落とす
    } else if (body.includes("{")) {
      // 入れ子の規則を持つブロックは全部降りる（@media @supports @layer @container @scope @starting-style @document と
      // 未知の at-rule・CSS Nesting の `p { :host & {…} }`）。宣言だけのブロックはそのまま（各階層を 1 度ずつ読む = 深さ × 長さ）
      out += prelude + "{" + dropRules(body, level + 1) + "}";
    } else {
      out += prelude + "{" + body + "}";
    }
    i = k;
  }
  return out;
}

const BLOCKED_FN_RE = /(^|[^\w-])((?:-[a-z]+-)?(?:image-set|image|cross-fade|element))\(/gi;

/** 文字列で読みに行ける関数を引数ごと `none` に（括弧の対応・文字列を数える。閉じなければ末尾まで） */
function blockFns(s: string): string {
  let out = "";
  let last = 0;
  BLOCKED_FN_RE.lastIndex = 0;
  for (let m = BLOCKED_FN_RE.exec(s); m; m = BLOCKED_FN_RE.exec(s)) {
    const open = m.index + m[0].length; // `(` の次
    let k = open;
    let depth = 1;
    let q: string | null = null;
    while (k < s.length && depth > 0) {
      const c = s[k];
      if (q) {
        if (c === q) q = null;
      } else if (c === '"' || c === "'") q = c;
      else if (c === "(") depth++;
      else if (c === ")") depth--;
      k++;
    }
    out += s.slice(last, m.index) + m[1] + "none";
    last = k;
    BLOCKED_FN_RE.lastIndex = k;
  }
  return out + s.slice(last);
}

/** `<style>` の中身の掃除。rewrite = 相対の url() を asset URL に（null を返せば none）・asset = asset: の url() を vault の中なら
   正規化した URL に（null を返せば none・渡さなければ asset: は全部 none） */
export function cleanStyleSheet(css: string, rewrite?: (v: string) => string | null, asset?: (v: string) => string | null): string {
  let s = decodeCss(css).replace(/\/\*[\s\S]*?(\*\/|$)/g, "");
  try {
    s = dropRules(s);
  } catch {
    return "";
  }
  s = blockFns(s);
  s = fixUrls(s, rewrite, asset);
  // 最後に念のため: 残った @import の語を壊す（規則の分割をすり抜けた形）
  return s.replace(/@import/gi, "@wk-blocked");
}

/** style 属性の掃除（宣言の並び。規則は無いので `:host` は関係しない） */
export function cleanStyleAttr(css: string, rewrite?: (v: string) => string | null, asset?: (v: string) => string | null): string {
  let s = decodeCss(css).replace(/\/\*[\s\S]*?(\*\/|$)/g, "");
  s = blockFns(s);
  s = fixUrls(s, rewrite, asset);
  return s.replace(/@import/gi, "@wk-blocked").replace(/[{}]/g, "");
}
