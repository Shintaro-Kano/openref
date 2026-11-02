/* OpenNote: asset URL と vault 内のパスの計算。DOM に依存しない純関数（node test が直接読む）。
   asset URL = `asset://localhost/` + 絶対パスを区画ごとに encodeURIComponent して `/` で join。
   絶対パスは `/` で始まるので URL は `asset://localhost//Users/…` の形になる（`//` を落とすと dev で壊れる） */

/** 絶対パス → asset URL */
export function assetUrl(abs: string): string {
  return "asset://localhost/" + abs.split("/").map(encodeURIComponent).join("/");
}

/** vault 相対パス → 絶対パス（root の末尾 `/` は畳む） */
export function absOf(root: string, rel: string): string {
  return root.replace(/\/+$/, "") + "/" + rel.replace(/^\/+/, "");
}

/** `a/./b/../c` → `a/c`。root より上へ出たら null */
export function normalizeRel(p: string): string | null {
  const out: string[] = [];
  for (const seg of p.split("/")) {
    if (seg === "" || seg === ".") continue;
    if (seg === "..") {
      if (!out.length) return null;
      out.pop();
    } else out.push(seg);
  }
  return out.join("/");
}

/** 親のフォルダ（`a/b/c.md` → `a/b`・`/` 無し・空・null → ""） */
export function dirOf(rel: string | null | undefined): string {
  if (!rel) return "";
  const i = rel.lastIndexOf("/");
  return i < 0 ? "" : rel.slice(0, i);
}

/** 最後の区画（`a/b/c.md` → `c.md`・末尾 `/` なら ""） */
export function baseName(p: string): string {
  return p.split("/").pop() ?? p;
}

/** `%xx` を解く（壊れた escape はそのまま） */
export function safeDecode(s: string): string {
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
}

/** URL の `#…` と `?…` を外した本体と見出し（`#` の後ろ・decode 済み） */
export function splitHash(dest: string): { base: string; hash: string | null } {
  const h = dest.indexOf("#");
  const base = h < 0 ? dest : dest.slice(0, h);
  const q = base.indexOf("?");
  return { base: q < 0 ? base : base.slice(0, q), hash: h < 0 ? null : safeDecode(dest.slice(h + 1)) };
}

/** URL の scheme（core links.rs の scheme と同じ = 2 文字以上・`C:` は scheme にしない）。無ければ null */
export function schemeOf(d: string): string | null {
  const m = /^([A-Za-z][A-Za-z0-9+.-]*):/.exec(d.trim());
  return m && m[1].length >= 2 ? m[1].toLowerCase() : null;
}

/** 相対の dest（`%` escape 可）→ vault 相対パス。`/` 始まりは vault の root から。root より上は null */
export function resolveRelative(srcPath: string, dest: string): string | null {
  const { base } = splitHash(dest);
  if (!base) return null;
  const decoded = safeDecode(base).normalize("NFC");
  return decoded.startsWith("/") ? normalizeRel(decoded) : normalizeRel((dirOf(srcPath) ? dirOf(srcPath) + "/" : "") + decoded);
}

/** `file:///…` → 絶対パス（decode 済み・NFC）。file: でなければ null */
export function filePath(url: string): string | null {
  const m = /^file:\/\/(localhost)?(\/[^?#]*)/i.exec(url.trim());
  return m ? safeDecode(m[2]).normalize("NFC") : null;
}

/** 絶対パスの `.` `..` `//` を畳む（`/` より上へ出たら null）。相対（`/` で始まらない）は null */
export function normalizeAbs(p: string): string | null {
  if (!p.startsWith("/")) return null;
  const r = normalizeRel(p);
  return r == null ? null : "/" + r;
}

/** `asset://localhost/<区画 encode>`（`//Users/…` の形も convertFileSrc の `%2FUsers…` の形も）→ 絶対パス（decode・NFC・正規化済み）。他は null */
export function assetPath(url: string): string | null {
  const m = /^asset:\/\/localhost\/([^?#]*)/i.exec(url.trim());
  if (!m) return null;
  let p = safeDecode(m[1]).normalize("NFC");
  if (!p.startsWith("/")) p = "/" + p;
  return normalizeAbs(p);
}

/** 絶対パスが vault の中なら vault 相対（NFC で比べる）。外なら null */
export function relInRoot(root: string, abs: string): string | null {
  const r = root.normalize("NFC").replace(/\/+$/, "");
  const a = abs.normalize("NFC");
  if (!a.startsWith(r + "/")) return null;
  return normalizeRel(a.slice(r.length + 1));
}

/** asset URL が vault の中を指すなら、`..` を畳み区画ごとに encode し直した asset URL。外・asset URL でなければ null
   （img / video の src と CSS の url() が同じこれで絞る） */
export function assetInRoot(root: string, url: string): string | null {
  const abs = assetPath(url);
  const rel = abs ? relInRoot(root, abs) : null;
  return rel ? assetUrl(absOf(root, rel)) : null;
}

/** 絶対パスがどの参照フォルダの中か。root と同じく NFC で比べ、フォルダそのもの・`.` 始まりの区画を含むものは外。無ければ null */
export function refRootOf(refs: readonly string[] | undefined, abs: string): string | null {
  if (!refs?.length) return null;
  for (const r of refs) {
    const rel = relInRoot(r, abs);
    if (rel && !rel.split("/").some((s) => s.startsWith("."))) return r;
  }
  return null;
}

/** 参照フォルダの中で内蔵の表示で開ける種類か（pdf / html / 画像 / 動画 / 音声。md はリンクの解決に索引が要るので外 = ホストの open） */
export function refViewable(abs: string): boolean {
  const k = viewKindOf(abs);
  const e = extOf(abs);
  return k === "image" || k === "video" || k === "audio" || k === "pdf" || e === "html" || e === "htm";
}

const IMAGE = ["png", "jpg", "jpeg", "gif", "webp", "svg", "avif", "bmp"];
const VIDEO = ["mp4", "m4v", "mov"];
const AUDIO = ["mp3", "m4a", "wav"];

export function extOf(p: string): string {
  const name = p.slice(p.lastIndexOf("/") + 1);
  const i = name.lastIndexOf(".");
  return i <= 0 ? "" : name.slice(i + 1).toLowerCase();
}

/** 拡張子 → 描画の種類（中央の表示・`![[…]]` の展開に使う）。page = md / html */
export function viewKindOf(p: string): "image" | "video" | "audio" | "pdf" | "page" | "other" {
  const e = extOf(p);
  if (IMAGE.includes(e)) return "image";
  if (VIDEO.includes(e)) return "video";
  if (AUDIO.includes(e)) return "audio";
  if (e === "pdf") return "pdf";
  if (e === "md" || e === "html" || e === "htm" || e === "") return "page";
  return "other";
}
