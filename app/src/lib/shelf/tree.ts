// 本棚の純関数と型（Svelte 非依存・副作用なし）。
// 木の形は core の shelf::scan（camelCase）= shelf_tree の tree。ここは「意味」だけ（検索の正規化・並び・パンくず・id と dir の索引）

export type ShelfSortKey = "title" | "mtime" | "size";

export interface ShelfBook {
  id: string;
  title: string;
  path: string;
  size: number;
  mtime: number;
  /** 親フォルダ名（検索・検索結果の所属の表示） */
  folder: string;
}

export interface ShelfFolder {
  id: string;
  name: string;
  path: string;
  children: ShelfFolder[];
  books: ShelfBook[];
  /** 配下（再帰）の総冊数 */
  totalBooks: number;
}

/** 履歴の 1 行（shelf_status。snake_case = wiki_status の workspaces と同じ） */
export interface ShelfRoot {
  root: string;
  name: string;
  last_used: number;
  exists: boolean;
  collapse_single: boolean;
  issue_titles: boolean;
  aliases: Record<string, string>;
}

/** タブの本棚。dir = root 相対（null = すべての本）。wiki と同じく前面も含めて常にここが正。永続するのは root と dir だけ */
export interface ShelfTab {
  root: string;
  dir: string | null;
  query: string;
  sort: ShelfSortKey;
  scrollTop: number;
}

/** 読み手のタブ。再起動で走査なしに戻すので、題名・id・大きさも ui_tabs に持つ（id = 読書位置と Finder の鍵） */
export interface ShelfBookTab {
  root: string;
  id: string;
  path: string;
  title: string;
  size: number;
  mtime: number;
}

/** 検索用の正規化: NFKC + 小文字化（全半角・カナ幅のゆらぎを吸収） */
export function norm(s: string): string {
  return s.normalize("NFKC").toLowerCase();
}

export function fmtSize(bytes: number): string {
  if (bytes >= 1 << 30) return `${(bytes / (1 << 30)).toFixed(1)} GB`;
  if (bytes >= 1 << 20) return `${Math.round(bytes / (1 << 20))} MB`;
  return `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

/** id → フォルダ（自身を含む全子孫） */
export function indexFolders(f: ShelfFolder, m: Map<string, ShelfFolder>): void {
  m.set(f.id, f);
  for (const c of f.children) indexFolders(c, m);
}

/** 子 id → 親 id（root は入らない = パンくずの終端） */
export function indexParents(f: ShelfFolder, m: Map<string, string>): void {
  for (const c of f.children) {
    m.set(c.id, f.id);
    indexParents(c, m);
  }
}

/** root 相対の dir（`Tab.shelf.dir`・shelf_list の dir と同じ）。root 自身は null */
export function dirOf(tree: ShelfFolder, f: ShelfFolder): string | null {
  return f.path === tree.path ? null : f.path.slice(tree.path.length + 1);
}

/** dir → フォルダ（root は入らない） */
export function indexDirs(tree: ShelfFolder, m: Map<string, ShelfFolder>): void {
  const walk = (f: ShelfFolder) => {
    for (const c of f.children) {
      m.set(dirOf(tree, c)!, c);
      walk(c);
    }
  };
  walk(tree);
}

/** 全子孫の本を out に積む（深さ優先・フォルダ順） */
export function collectBooks(f: ShelfFolder, out: ShelfBook[]): void {
  out.push(...f.books);
  for (const c of f.children) collectBooks(c, out);
}

/** 検索語への分解: NFKC + 小文字化して空白で AND。空（空白だけ）は [] = 呼び側の「検索中か」の判定にも使う */
export function searchTerms(query: string): string[] {
  const q = norm(query.trim());
  return q ? q.split(/\s+/).filter(Boolean) : [];
}

/** 「書名 + 親フォルダ名」に全語を含む本。語が無ければ全件をそのまま通す */
export function filterBooks(books: ShelfBook[], query: string): ShelfBook[] {
  const terms = searchTerms(query);
  return books.filter((b) => {
    const hay = norm(`${b.title} ${b.folder}`);
    return terms.every((t) => hay.includes(t));
  });
}

/** 並べ替えた新しい配列（引数は変えない = folder.books を直接渡せる）。名前 = 昇順・更新とサイズ = 大きい順 */
export function sortBooks(books: ShelfBook[], key: ShelfSortKey): ShelfBook[] {
  return [...books].sort((a, b) => (key === "title" ? a.title.localeCompare(b.title, "ja") : key === "mtime" ? b.mtime - a.mtime : b.size - a.size));
}

/** フォルダカードの代表の表紙（直下 → 子孫の順で最大 n 冊） */
export function repBooks(f: ShelfFolder, n = 3): ShelfBook[] {
  const out: ShelfBook[] = [];
  const walk = (folder: ShelfFolder) => {
    for (const b of folder.books) {
      if (out.length >= n) return;
      out.push(b);
    }
    for (const c of folder.children) {
      if (out.length >= n) return;
      walk(c);
    }
  };
  walk(f);
  return out;
}

/** パンくず（root → cur）。cur が null なら空 */
export function breadcrumbOf(cur: ShelfFolder | null | undefined, byId: ReadonlyMap<string, ShelfFolder>, parentOf: ReadonlyMap<string, string>): ShelfFolder[] {
  const chain: ShelfFolder[] = [];
  let f: ShelfFolder | undefined = cur ?? undefined;
  while (f) {
    chain.unshift(f);
    const pid = parentOf.get(f.id);
    f = pid ? byId.get(pid) : undefined;
  }
  return chain;
}

/** 最近開いた本（ui_shelf_recent）に足す: 同じ本は先頭へ・上限 n */
export function pushRecent<T extends { root: string; path: string }>(list: T[], item: T, n = 12): T[] {
  return [item, ...list.filter((r) => r.root !== item.root || r.path !== item.path)].slice(0, n);
}

/** 読書位置（ui_shelf_pos）に足す: 新しい物を末尾へ・上限 n を超えたら古い物から落とす（鍵は 16 hex = 挿入順が保たれる） */
export function pushPos(pos: Record<string, number>, id: string, page: number, n = 500): Record<string, number> {
  const out: Record<string, number> = { ...pos };
  delete out[id];
  out[id] = page;
  const keys = Object.keys(out);
  for (let i = 0; i < keys.length - n; i++) delete out[keys[i]];
  return out;
}

/** asset_reply の 1 区間の上限（wiki_asset.rs の MAX_LEN と同じ値 = range.test.mjs が結ぶ） */
export const ASSET_MAX = 1000 * 1024;

/** requestDataRange の区間を asset_reply の 1 区間の上限 max 以下に分ける（[begin, end) の列・つなぐと元の区間） */
export function rangeParts(begin: number, end: number, max: number): [number, number][] {
  const parts: [number, number][] = [];
  for (let p = begin; p < end; p += max) parts.push([p, Math.min(p + max, end)]);
  return parts;
}

/** 代わりの表紙の色相（題名から決まる） */
export function hueOf(s: string): number {
  let h = 0;
  for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) | 0;
  return Math.abs(h) % 360;
}
