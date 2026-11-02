// OpenNote の編集のセッション。前面の Wiki タブで編集中のページ 1 枚の
// 保存の規則だけを持つ（画面は WikiEditor / WikiPage・CodeMirror は editor/cm.ts）。
// - 自動保存: 入力停止 800ms・タブの切替・閉じる・⌘S（leave / flush）。wiki_save {path, content, if_sha256: 読んだときの sha256}
// - conflict（外部 = Claude Code が先に書いた）: 自動保存を止めて「読み直す / 上書き」を尋ねる（黙って負けない・勝たない）
// - 外部の書換え（wiki-changed）: 未保存の変更が無ければ黙って読み直す・あれば触らない（次の保存が conflict になる）
// - 未保存の内容を黙って失わない: 書けないまま編集を終えた（Wiki を無効にした・フォルダが変わった・conflict のまま等）本文は
//   ed.stranded に残し、WikiRescue が「保存できていない変更があります」とクリップボードへのコピーを出す
// - 改行: エディタの中は \n・書き出しはファイルの改行コード（editor/eol.ts）。読んだ原文とは同じ正規化で比べる（開いて離れるだけで書かない）
// state.svelte（タブ）と wiki.svelte（木）には依存しない = 部品テスト（edit.test.mjs）で invoke を差し替えて確かめる

import { invoke } from "./ipc";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { t } from "../i18n.svelte";
import type { WikiImportResult, WikiPageData, WikiSaveBlobResult, WikiWriteResult } from "./types";
import { eolOf, withEol } from "./editor/eol";

/** エディタ（cm.ts の EditorHandle のうちセッションが使う分） */
export interface EditHandle {
  getText(): string;
  setText(s: string): void;
  insert(s: string): void;
  focus(): void;
}

export type SaveState = "clean" | "dirty" | "saving" | "saved" | "error" | "conflict";

export const ed = $state({
  /** 編集中のページ（vault 相対・null = 編集していない） */
  path: null as string | null,
  state: "clean" as SaveState,
  error: null as string | null,
  /** 競合のダイアログを出している（「キャンセル」で閉じても state は conflict のまま = 自動保存しない） */
  dialog: false,
  /** 競合の相手（今のファイルの sha256・消えていれば null） */
  theirs: null as string | null,
  /** 分割のプレビューに渡す本文（入力の debounce） */
  preview: "",
  /** 書けないまま編集を終えた本文（WikiRescue がコピー / 破棄を尋ねる。古い順） */
  stranded: [] as { path: string; content: string; reason: string }[],
});

/** 間合い（テストで縮める）。retry = 保存に失敗したときの自動の再試行 */
export const cfg = { delay: 800, previewDelay: 250, retry: 5000 };

/** 画面へ知らせる（wiki.svelte が toast を入れる。テストでは集める） */
let notice: (msg: string) => void = () => {};
export function setNotice(f: (msg: string) => void) {
  notice = f;
}

/** 保存できた（wiki.svelte が wk.page の本文・hash・大きさを手元の値で更新する） */
type Saved = (path: string, content: string, r: { sha256: string; size: number; mtime: number }) => void;
let onSaved: Saved = () => {};
export function setOnSaved(f: Saved) {
  onSaved = f;
}

let base: string | null = null; // 読んだ（書いた）ときのファイルの sha256
let saved = ""; // 最後に読んだ / 書いた本文（エディタの改行の規則で正規化した形 = current() と比べられる）
let text = ""; // エディタが無い間の本文
let handle: EditHandle | null = null;
let timer: ReturnType<typeof setTimeout> | undefined;
let retryTimer: ReturnType<typeof setTimeout> | undefined;
let previewTimer: ReturnType<typeof setTimeout> | undefined;
let saving: Promise<boolean> | null = null;
/** 保存が成功した回数（外部の読み直しが保存を追い越したかの判定） */
let saveGen = 0;

const current = (): string => (handle ? handle.getText() : text);
const readFile = (path: string) => invoke<WikiPageData>("wiki_read_gui", { args: { path, max_chars: 2_000_000 } });
/** 読んだ原文をエディタと同じ規則に（孤立した \r → ファイルの改行コード） */
const normal = (s: string) => withEol(s, eolOf(s));

/** 自分で U+FFFD を含めて保存したページ（その後に読んだ U+FFFD は lossy の印ではない） */
const wroteFffd = new Set<string>();

/** 編集できるページか（md / html の本文を全部読めた・照合の hash がある。途中までのページ・UTF-8 でない疑いのあるページ
    （core の read = wiki_read_gui に lossy の印が無いので U+FFFD で判定）を書き戻すと壊す） */
export function editable(p: WikiPageData | null | undefined): boolean {
  // 参照フォルダのファイルは読み取り専用（索引に無い・wiki_save はワークスペースの外を受けない）
  if (!p || p.external || (p.kind !== "md" && p.kind !== "html") || p.content == null || !p.sha256) return false;
  return !p.truncated && (!p.content.includes("\uFFFD") || wroteFffd.has(p.path));
}

/** 編集を始める（同じページなら何もしない）。前のページの未保存は stopEdit が書きに行く */
export function startEdit(p: WikiPageData) {
  if (ed.path === p.path) return;
  if (ed.path) stopEdit();
  ed.path = p.path;
  base = p.sha256 ?? null;
  saved = text = normal(p.content ?? "");
  ed.state = "clean";
  ed.error = null;
  ed.dialog = false;
  ed.theirs = null;
  ed.preview = saved;
}

/** エディタの本文（エディタが無ければ最後に知っている本文） */
export function editText(): string {
  return current();
}

/** 未保存の変更があるか */
export function dirty(): boolean {
  return !!ed.path && (ed.state === "conflict" || current() !== saved);
}

export function attach(h: EditHandle) {
  handle = h;
}

export function detach(h: EditHandle) {
  if (handle !== h) return;
  text = h.getText();
  handle = null;
}

/** エディタの本文が変わった（cm.ts の onChange） */
export function changed() {
  if (!ed.path) return;
  clearTimeout(previewTimer);
  previewTimer = setTimeout(() => (ed.preview = current()), cfg.previewDelay);
  if (ed.state === "conflict") return; // 解決するまで自動保存しない
  if (!saving && ed.state !== "error") ed.state = "dirty";
  clearTimeout(timer);
  timer = setTimeout(() => void flush(), cfg.delay);
}

/** 照合は必ず付ける（base が無いページは editable が編集させない = 無条件に書く経路を作らない） */
function body(path: string, content: string, b = base): Record<string, unknown> {
  return { path, content, if_sha256: b };
}

function wrote(path: string, content: string, r: { sha256: string; size: number; mtime: number }) {
  if (content.includes("\uFFFD")) wroteFffd.add(path);
  onSaved(path, content, r);
}

/** 今すぐ保存（⌘S・入力停止）。保存できた / 変更なし = true・conflict / 失敗 = false。conflict ならダイアログを出す */
export async function flush(): Promise<boolean> {
  clearTimeout(timer);
  clearTimeout(retryTimer);
  if (!ed.path) return true;
  while (saving) await saving;
  if (!ed.path) return true;
  if (ed.state === "conflict") {
    ed.dialog = true;
    return false;
  }
  const path = ed.path;
  const content = current();
  if (content === saved) {
    if (ed.state === "dirty" || ed.state === "error") ed.state = "clean";
    return true;
  }
  ed.state = "saving";
  let done!: (ok: boolean) => void;
  saving = new Promise((r) => (done = r));
  let ok = false;
  try {
    const r = await invoke<WikiWriteResult>("wiki_save", { args: body(path, content) });
    if (ed.path !== path) return (ok = r.ok); // 待つ間に編集を終えた（stopEdit 済み）
    if (r.ok) {
      warnIndex(r);
      base = r.sha256;
      saved = content;
      saveGen++;
      wrote(path, content, r);
      ok = true;
      ed.error = null;
      ed.state = current() === saved ? "saved" : "dirty";
      if (ed.state === "dirty") timer = setTimeout(() => void flush(), cfg.delay);
    } else {
      ed.state = "conflict";
      ed.theirs = r.sha256;
      ed.dialog = true;
    }
  } catch (e) {
    if (ed.path === path) {
      ed.state = "error";
      ed.error = String(e);
      // 一定の間合いで自動で再試行（⌘S なら即）
      retryTimer = setTimeout(() => {
        if (ed.path === path && ed.state === "error") void flush();
      }, cfg.retry);
    }
  } finally {
    saving = null;
    done(ok);
  }
  return ok;
}

/** 離れる前（タブの切替・閉じる・別のページへ・表示へ戻す・MY LIBRARY へ）。保存の要らないときは同期で true
    （Wiki を使っていない間の切替に await を足さない）。保存できなければ false = 離れない（conflict ならダイアログ） */
export function leave(): true | Promise<boolean> {
  if (!ed.path || (!saving && ed.state !== "conflict" && current() === saved)) return true;
  return (async () => {
    for (;;) {
      if (!(await flush())) return false;
      if (!ed.path || (!saving && current() === saved)) return true;
    }
  })();
}

/** 競合のダイアログの答え。reload = 外部の内容で置き換え（自分の変更は捨てる）・overwrite = force で上書き（core が退避）・
    cancel = ダイアログだけ閉じる（自動保存は止めたまま・⌘S でまた尋ねる）。読み直した page を返す（wk.page の更新用） */
export async function resolveConflict(choice: "reload" | "overwrite" | "cancel"): Promise<WikiPageData | null> {
  const path = ed.path;
  if (!path || ed.state !== "conflict") return null;
  if (choice === "cancel") {
    ed.dialog = false;
    return null;
  }
  try {
    if (choice === "reload") {
      const p = await readFile(path);
      if (ed.path !== path) return null;
      if (!editable(p)) throw new Error(t("このページは編集できない形になりました（外部のアプリで確かめてください）"));
      base = p.sha256 ?? null;
      saved = normal(p.content ?? "");
      if (handle) handle.setText(p.content ?? "");
      else text = saved;
      ed.preview = saved;
      settle("clean");
      return p;
    }
    const content = current();
    // 同じ照合（if_sha256 = 読んだときの hash）+ force = core が上書き前を退避してから書く
    const r = await invoke<WikiWriteResult>("wiki_save", { args: { ...body(path, content), force: true } });
    if (ed.path !== path) return null;
    if (!r.ok) throw new Error(t("上書きできませんでした"));
    warnIndex(r);
    base = r.sha256;
    saved = content;
    saveGen++;
    wrote(path, content, r);
    const more = current() !== saved; // 上書きを待つ間に打った分
    settle(more ? "dirty" : "saved");
    if (more) timer = setTimeout(() => void flush(), cfg.delay);
    return null;
  } catch (e) {
    notice(`✗ ${String(e).slice(0, 200)}`);
    return null;
  }
}

/** 書けたが索引の更新に失敗した（core の任意の欄 index_error）= 知らせる（保存は成功） */
function warnIndex(r: unknown) {
  const e = (r as { index_error?: unknown }).index_error;
  if (e) notice(`⚠ ${t("保存しましたが、索引を更新できませんでした")}: ${String(e).slice(0, 160)}`);
}

function settle(s: SaveState) {
  ed.state = s;
  ed.dialog = false;
  ed.theirs = null;
  ed.error = null;
}

/** 外部でこのページが書き換えられた（wiki-changed）。未保存の変更が無ければ黙って読み直す・あれば本文には触らない
    （次の保存が conflict = ダイアログ）。読んだ page を返す（題名・リンクの更新用・読めなければ null） */
export async function refreshFromDisk(): Promise<WikiPageData | null> {
  const path = ed.path;
  if (!path) return null;
  const g = saveGen;
  let p: WikiPageData;
  try {
    p = await readFile(path);
  } catch {
    return null; // 消えた等。次の保存が conflict（sha256 = null）になる
  }
  // 読む間に保存した・編集を終えた = 読んだ内容は古いかもしれない（呼び出し側に adopt させない）
  if (ed.path !== path || g !== saveGen || saving) return null;
  if (p.sha256 === base) return p; // 自分の保存の知らせ
  if (ed.state !== "conflict" && current() === saved && editable(p)) {
    base = p.sha256 ?? null;
    saved = normal(p.content ?? "");
    if (handle) handle.setText(p.content ?? ""); // 外部が改行コードを変えたらそれに追随
    else text = saved;
    ed.preview = saved;
    ed.state = "clean";
  }
  return p;
}

/** 書けないまま編集を終えた本文を残す（WikiRescue が出す） */
function strand(path: string, content: string, reason: string) {
  ed.stranded.push({ path, content, reason });
  notice(`✗ ${t("保存できていない変更があります")}: ${path}`);
}

function endSession() {
  clearTimeout(timer);
  clearTimeout(retryTimer);
  clearTimeout(previewTimer);
  ed.path = null;
  ed.state = "clean";
  ed.error = null;
  ed.dialog = false;
  ed.theirs = null;
  base = null;
  saved = text = "";
}

/** 編集を終える（表示へ戻した・ページが変わった）。通常の経路は先に leave() で保存済み。未保存が残っていれば照合付きで
    書きに行き（外部の書換えは上書きしない）、書けなければ・conflict のままなら本文を ed.stranded に残す（黙って捨てない） */
export function stopEdit() {
  if (!ed.path) return;
  const path = ed.path;
  const content = current();
  const b = base;
  const conflict = ed.state === "conflict";
  const pend = conflict || content !== saved;
  endSession();
  if (!pend) return;
  if (conflict) {
    strand(path, content, t("外部で書き換えられています"));
    return;
  }
  invoke<WikiWriteResult>("wiki_save", { args: body(path, content, b) })
    .then((r) => {
      if (r.ok) wrote(path, content, r);
      else strand(path, content, t("外部で書き換えられています"));
    })
    .catch((e) => strand(path, content, String(e).slice(0, 200)));
}

/** 書かずに編集を終える（Wiki を無効にした・vault のフォルダが変わった = 今の root へは書かない）。未保存は ed.stranded へ */
export function strandEdit(reason: string) {
  if (!ed.path) return;
  const path = ed.path;
  const content = current();
  const pend = ed.state === "conflict" || content !== saved;
  endSession();
  if (pend) strand(path, content, reason);
}

/** 編集を捨てて終える（ゴミ箱へ移したページ・「変更を破棄して移る」） */
export function discardEdit() {
  if (!ed.path) return;
  endSession();
}

/** 自動保存を止める（ゴミ箱へ移す間 = 移した後に書きに行かない） */
export function holdAutosave() {
  clearTimeout(timer);
  clearTimeout(retryTimer);
}

/** 止めた自動保存を戻す（ゴミ箱へ移せなかった） */
export function resumeAutosave() {
  if (ed.path && ed.state !== "conflict" && current() !== saved) timer = setTimeout(() => void flush(), cfg.delay);
}

/** 改名に追随（wiki_rename の後） */
export function renamed(from: string, to: string) {
  if (ed.path === from) ed.path = to;
}

/** ArrayBuffer → base64（大きな画像でも String.fromCharCode の引数の上限を越えないよう区切る） */
export function toBase64(buf: ArrayBuffer): string {
  const u = new Uint8Array(buf);
  let s = "";
  for (let i = 0; i < u.length; i += 0x8000) s += String.fromCharCode(...u.subarray(i, i + 0x8000));
  return btoa(s);
}

const EXT: Record<string, string> = { "image/png": "png", "image/jpeg": "jpg", "image/gif": "gif", "image/webp": "webp", "image/svg+xml": "svg", "image/avif": "avif" };

/** 貼付画像の名前（元名があればそれ・クリップボードの既定名 "image.png" 等は日時） */
export function blobName(f: { name?: string; type: string }, now = new Date()): string {
  const n = (f.name ?? "").trim();
  if (n && !/^image\.\w+$/i.test(n)) return n;
  const p = (x: number) => String(x).padStart(2, "0");
  const stamp = `${now.getFullYear()}${p(now.getMonth() + 1)}${p(now.getDate())}-${p(now.getHours())}${p(now.getMinutes())}${p(now.getSeconds())}`;
  return `Pasted image ${stamp}.${EXT[f.type] ?? "png"}`;
}

/** 貼り付けた画像を attachments/ へ保存して caret に `![[…]]`（wiki_save_blob） */
export async function pasteImages(files: File[]) {
  const path = ed.path;
  if (!path) return;
  const embeds: string[] = [];
  for (const f of files) {
    try {
      const data_base64 = toBase64(await f.arrayBuffer());
      const r = await invoke<WikiSaveBlobResult>("wiki_save_blob", { args: { name: blobName(f), data_base64, page: path } });
      embeds.push(r.embed);
    } catch (e) {
      notice(`✗ ${t("画像を保存できません")}: ${String(e).slice(0, 200)}`);
    }
  }
  if (embeds.length && ed.path === path) handle?.insert(embeds.join("\n"));
}

/** OS からのドロップ（App の onDragDropEvent）。編集中なら各ファイルを vault へ（wiki_import_file）→ caret に `![[…]]`。
    編集中でなければ false（呼び出し側が「編集モードで」と知らせる。蔵書へは流さない） */
export async function dropPaths(paths: string[]): Promise<boolean> {
  const path = ed.path;
  if (!path || !handle) return false;
  const embeds: string[] = [];
  for (const abs of paths) {
    try {
      const r = await invoke<WikiImportResult>("wiki_import_file", { args: { abs, page: path } });
      embeds.push(r.embed);
    } catch (e) {
      notice(`✗ ${t("取り込めません")}: ${abs.split("/").pop()} · ${String(e).slice(0, 160)}`);
    }
  }
  if (embeds.length && ed.path === path) handle?.insert(embeds.join("\n")); // 待つ間に別のページへ移ったら入れない
  return true;
}

/** 取り込めるファイルの拡張子（core の media_kind と同じ = 画像・動画・音声・PDF） */
export const PICK_EXT = [
  "png", "jpg", "jpeg", "gif", "webp", "svg", "avif", "heic", "bmp", "tif", "tiff",
  "mp4", "m4v", "mov", "webm", "mkv", "avi", "mp3", "m4a", "wav", "aac", "flac", "ogg", "opus", "pdf",
];

/** 「/」メニューの「画像・PDF を埋め込む…」= ファイル選択 → ドロップと同じ道（wiki_import_file → caret に `![[…]]`）。
    取り消したら何もしない（フォーカスだけ戻す） */
export async function pickFiles(): Promise<boolean> {
  if (!ed.path || !handle) return false;
  let p: unknown;
  try {
    p = await openDialog({ multiple: true, filters: [{ name: t("画像・PDF・動画・音声"), extensions: PICK_EXT }] });
  } catch (e) {
    notice(`✗ ${t("ファイルを選べません")}: ${String(e).slice(0, 160)}`);
    return false;
  }
  const paths = Array.isArray(p) ? (p as string[]) : typeof p === "string" ? [p] : [];
  if (!paths.length) {
    handle.focus();
    return false;
  }
  return dropPaths(paths);
}
