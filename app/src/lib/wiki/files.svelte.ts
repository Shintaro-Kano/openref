// OpenNote のファイル操作 = 新規ページ / 新規フォルダ / 改名 / ゴミ箱と、そのメニュー・ダイアログ（WikiDialogs）の流れ。
// wiki.svelte.ts から移しただけ（B-33）。状態（wk）と骨組みの動詞は wiki.svelte.ts を読む一方向（wiki.svelte.ts はここを読まない = 循環しない）

import { t } from "../i18n.svelte";
import { app, closeTab, frontWiki, toast } from "../state.svelte";
import { dirOf } from "./render/asset.ts";
import type { WikiCreateResult, WikiMkdirResult, WikiRenamePreview, WikiRenameResult, WikiTrashResult } from "./types";
import { dirty, discardEdit, ed, holdAutosave, renamed, resumeAutosave } from "./edit.svelte";
import { canLeave, isRefPath, loadAll, openWikiPage, wikiArgs, wk } from "./wiki.svelte";

/** 今のフォルダ（前面のページのフォルダ・ホームなら vault の直下 = ""・参照フォルダのページを見ているときも vault の直下） */
export function currentDir(): string {
  const p = frontWiki()?.path;
  return isRefPath(p) ? "" : dirOf(p);
}

/** ヘッダーの [＋ 新規]: 新規ページ… / 新規フォルダ… のメニュー（作る場所 = 今のフォルダ）。開いているときの再押下は閉じる
    （WikiDialogs は開いているメニューのボタンの mousedown では閉じない = click で開き直さない） */
export function openNewMenu(x: number, y: number) {
  if (!frontWiki()) return;
  wk.menu = wk.menu?.kind === "new" ? null : { kind: "new", path: currentDir(), x, y };
}

/** 新規ページ: dir（省けば今のフォルダ）に作る名前と形式（md / html）を尋ねる */
export function askNewPage(dir: string = currentDir()) {
  if (!frontWiki()) return;
  wk.menu = null;
  wk.dialog = { kind: "new", dir };
}

/** 新規フォルダ: dir（省けば今のフォルダ）に作る名前を尋ねる */
export function askNewFolder(dir: string = currentDir()) {
  if (!frontWiki()) return;
  wk.menu = null;
  wk.dialog = { kind: "newdir", dir };
}

/** フォルダの名前に使えない文字（ページの字に加えて `/` = 1 段だけ作る）。問題が無ければ null */
export function folderNameProblem(name: string): string | null {
  return nameProblem(name) ?? (name.includes("/") ? `${t("名前に使えない文字があります")}: /` : null);
}

/** 空のフォルダを作る（wiki_mkdir）。木に出すために覚え、祖先を開く */
export async function createFolder(dir: string, name: string): Promise<boolean> {
  const bad = folderNameProblem(name);
  if (bad || !name.trim()) {
    if (bad) toast(`✗ ${bad}`);
    return false;
  }
  try {
    const r = (await wikiArgs("wiki_mkdir", { dir, name: name.trim() })) as WikiMkdirResult;
    wk.dialog = null;
    wk.newDirs.add(r.path);
    const parts = r.path.split("/");
    for (let i = 1; i <= parts.length; i++) wk.open.add(parts.slice(0, i).join("/"));
    return true;
  } catch (e) {
    toast(`✗ ${t("フォルダを作れません")}: ${String(e).slice(0, 200)}`);
    return false;
  }
}

/** ページの名前に使えない文字（core がエラーにする = Obsidian のリンクが壊れる文字）。問題が無ければ null */
export function nameProblem(name: string): string | null {
  const bad = name.match(/[\[\]|#^\\:]/g);
  if (bad) return `${t("名前に使えない文字があります")}: ${[...new Set(bad)].join(" ")}`;
  // `.` 始まりの区画は core が拒否する（隠しファイル = 索引も監視も見ない）。先に止める = 新規ページが Untitled のまま残らない
  if (name.split("/").some((s) => s.trim().startsWith("."))) return t("名前を「.」で始めることはできません");
  return null;
}

/** 名前に拡張子が無ければ .md（.md / .html / .htm だけを受ける） */
export function pageName(name: string, ext = "md"): string {
  const n = name.trim().replace(/\.+$/, "");
  return /\.(md|html?)$/i.test(n) ? n : `${n}.${ext}`;
}

export type PageExt = "md" | "html";

/** 作るページの形式: 名前に拡張子（.md / .html / .htm）があればそれ・無ければ選んだ形式 */
export function pageExt(name: string, ext: PageExt): PageExt {
  const n = name.trim().replace(/\.+$/, ""); // pageName と同じく末尾の . は落としてから見る
  return /\.html?$/i.test(n) ? "html" : /\.md$/i.test(n) ? "md" : ext;
}

/** 新規ページを作って編集で開く（wiki_create = Untitled.md / .html → 名前があれば wiki_rename {rewrite: false} で改名）。
    改名に失敗しても（同名がある等）作ったページは Untitled のまま開いて知らせる */
export async function createPage(dir: string, name: string, ext: PageExt = "md"): Promise<boolean> {
  const bad = nameProblem(name);
  if (bad) {
    toast(`✗ ${bad}`);
    return false;
  }
  ext = pageExt(name, ext);
  try {
    const r = (await wikiArgs("wiki_create", { dir, name: `Untitled.${ext}` })) as WikiCreateResult;
    let path = r.path;
    if (name.trim()) {
      const to = (dir ? `${dir}/` : "") + pageName(name, ext);
      if (to !== path) {
        try {
          path = ((await wikiArgs("wiki_rename", { from: path, to, rewrite: false })) as WikiRenameResult).path;
        } catch (e) {
          toast(`✗ ${t("名前を変えられません")}: ${String(e).slice(0, 200)}`);
        }
      }
    }
    wk.dialog = null;
    void loadAll();
    await openWikiPage(path, false, undefined, true);
    return true;
  } catch (e) {
    toast(`✗ ${t("ページを作れません")}: ${String(e).slice(0, 200)}`);
    return false;
  }
}

/** 名前の変更を始める（木の右クリック・ツールバー） */
export function askRename(path: string) {
  wk.menu = null;
  wk.dialog = { kind: "rename", path, preview: null, to: "" };
}

/** 入力した新しい名前 → vault 相対のパス（`/` を含めば vault 相対・無ければ同じフォルダ。拡張子が無ければ元の拡張子） */
export function renameTarget(from: string, input: string): string {
  const ext = /\.([^./]+)$/.exec(from)?.[1] ?? "md";
  const v = input.trim().replace(/^\/+/, "");
  // 「PI-RADS v2.1」のような点入りの名前に拡張子を足し忘れない = 拡張子とみなすのはページ・元の拡張子だけ
  const last = (v.split("/").pop() ?? "").toLowerCase();
  const withExt = /\.(md|html?)$/.test(last) || last.endsWith(`.${ext.toLowerCase()}`) ? v : `${v}.${ext}`;
  return v.includes("/") ? withExt : (dirOf(from) ? `${dirOf(from)}/` : "") + withExt;
}

/** 改名の確認（wiki_rename_preview）。書き換えるリンクが無ければそのまま改名する（書き換えない参照がチャットのログだけでも = 確認しても選べることが無い） */
export async function previewRename(from: string, input: string) {
  const to = renameTarget(from, input);
  if (!input.trim() || to === from) return;
  const bad = nameProblem(input);
  if (bad) {
    toast(`✗ ${bad}`);
    return;
  }
  try {
    const pv = await wikiArgs("wiki_rename_preview", { from, to }) as WikiRenamePreview;
    if (!pv.links && !pv.other?.some((o) => o.kind !== "chat-log")) {
      await doRename(from, to, false);
      return;
    }
    if (wk.dialog?.kind === "rename" && wk.dialog.path === from) wk.dialog = { kind: "rename", path: from, preview: pv, to };
  } catch (e) {
    toast(`✗ ${t("名前を変えられません")}: ${String(e).slice(0, 200)}`);
  }
}

/** 改名（wiki_rename）。編集中のページなら先に保存。開いているタブの path・編集のセッションを追随させる */
export async function doRename(from: string, to: string, rewrite: boolean) {
  // 編集中なら（別のページの改名でも = リンクの書換えで編集中のページが書き換わりうる）先に保存
  if (ed.path && !(await canLeave())) return;
  try {
    const r = await wikiArgs("wiki_rename", { from, to, rewrite }) as WikiRenameResult;
    wk.dialog = null;
    renamed(from, r.path);
    for (const tab of app.tabs) if (tab.wiki?.path === from) tab.wiki.path = r.path;
    // skipped = 確認の後に書き換わったので触らなかったファイル（core の任意の欄）
    const skipped = r.skipped ?? [];
    if (skipped.length) {
      toast(`⚠ ${t("名前を変えました")} · ${t("確認の後に変わったため書き換えなかったページ")}: ${skipped.map((x) => x.path).join(", ").slice(0, 200)}`, 10000);
    } else if (r.rewritten.length) toast(`${t("名前を変えました")} · ${t("リンクを書き換えたページ")}: ${r.rewritten.length}`);
    else toast(t("名前を変えました"));
    void loadAll();
  } catch (e) {
    toast(`✗ ${t("名前を変えられません")}: ${String(e).slice(0, 200)}`);
  }
}

/** ゴミ箱に移す前の確認（dir = フォルダを中身ごと） */
export function askTrash(path: string, dir = false) {
  wk.menu = null;
  wk.dialog = { kind: "trash", path, dir };
}

/** path の下にあるか（dir のときはフォルダの中身も） */
function underPath(p: string | null | undefined, path: string, dir: boolean): boolean {
  return p != null && (p === path || (dir && p.startsWith(path + "/")));
}

/** path の下にあるページ（md / html）の数（フォルダの確認の文に出す。索引には画像・PDF も載るので kind で絞る） */
export function pagesUnder(path: string): number {
  return wk.pages.filter((p) => p.path.startsWith(path + "/") && (p.kind === "md" || p.kind === "html")).length;
}

/** 編集中のページが path（dir なら中も）に当たるか（確認の文に「未保存の変更は失われる」と出す） */
export function editingUnder(path: string, dir: boolean): boolean {
  return underPath(ed.path, path, dir) && dirty();
}

/** ゴミ箱へ（wiki_trash = macOS のゴミ箱 = vault の外・Finder の「戻す」が効く）。dir = フォルダを中身ごと。
    編集中のページが対象なら編集を捨てる。開いているタブは閉じる（最後の 1 枚はホームへ）。覚えていた空フォルダも忘れる */
export async function doTrash(path: string, dir = false) {
  let r: WikiTrashResult;
  try {
    const editing = underPath(ed.path, path, dir);
    if (editing) holdAutosave(); // 移す間に書きに行かない（失敗したら打鍵で再開・⌘S で即）
    r = await wikiArgs("wiki_trash", { path, dir }) as WikiTrashResult;
    if (editing && underPath(ed.path, path, dir)) discardEdit(); // 移せてから編集を捨てる（失敗したらエディタも状態も元のまま）
  } catch (e) {
    if (underPath(ed.path, path, dir)) resumeAutosave();
    toast(`✗ ${t("ゴミ箱に移せません")}: ${String(e).slice(0, 200)}`);
    return;
  }
  // ここから下はもう移した後 = 失敗しても「移せません」とは言わない（タブは次の読み直しで消えたページとして扱われる）
  wk.dialog = null;
  if (dir) {
    for (const d of [...wk.newDirs]) if (underPath(d, path, true)) wk.newDirs.delete(d);
  }
  if (r.index_error) toast(`${t("ゴミ箱に移しましたが、索引を更新できませんでした")}: ${r.index_error.slice(0, 160)}`, 8000);
  else toast(t("ゴミ箱に移しました（Finder の「戻す」で戻せます）"));
  try {
    const hit = app.tabs.filter((x) => underPath(x.wiki?.path, path, dir));
    for (const tab of hit) {
      if (app.tabs.length > 1 && tab.id !== app.tab) await closeTab(tab.id);
    }
    const front = app.tabs.find((x) => x.id === app.tab);
    if (front && underPath(front.wiki?.path, path, dir)) {
      if (app.tabs.length > 1) await closeTab(front.id);
      else front.wiki = { path: null, scrollTop: 0, editing: false };
    }
  } catch (e) {
    toast(`✗ ${t("タブを閉じられません")}: ${String(e).slice(0, 200)}`);
  }
  void loadAll();
}
