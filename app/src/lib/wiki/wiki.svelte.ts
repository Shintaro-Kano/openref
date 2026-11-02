// OpenNote の画面側の状態と動詞。骨組み = モード・タブ・木・ページの遷移・右ペインの読込。
// 描画は MdView / HtmlView / MediaView。Tauri command は main.rs の wiki_*（読みは登録表と同じ形 = invoke(name, { args })）。
// Wiki が無効（app.wikiOn = false）の間は何も描かず、既存の挙動を変えない（原則 1）
// 検索の動詞 = search.svelte.ts・ファイル操作（新規 / 改名 / ゴミ箱）= files.svelte.ts・参照フォルダの追加 / 外す = refdirs.svelte.ts（B-33。どれもここを読む一方向）

import { errorCode, invoke, WIKI_CODES } from "./ipc";
import { listen } from "@tauri-apps/api/event";
import { SvelteMap, SvelteSet } from "svelte/reactivity";
import { t } from "../i18n.svelte";
import { api } from "../api";
import { activateTab, app, frontWiki, modalOpen, openTab, reveal, setLeaveGuard, setModalGuard, setView, tabOpsAllowed, toast, type TabWiki } from "../state.svelte";
import { viewKindOf } from "./render/asset.ts";
import { findHeading } from "./render/sanitize.ts";
import type {
  WikiBacklink,
  WikiChanged,
  WikiHeading,
  WikiInboxFile,
  WikiListEntry,
  WikiNav,
  WikiPageData,
  WikiRenamePreview,
  WikiSearchHit,
  WikiStatus,
  WikiTagCount,
} from "./types";
import { discardEdit, dropPaths, ed, flush, leave, setNotice, setOnSaved, strandEdit } from "./edit.svelte";
import { chat } from "./chat.svelte"; // chat.svelte も wk を読む（循環）= guard の中でだけ読む（module の評価では触らない）

// 読み取り専用の大きな配列（木の 1 万件・検索の結果・箱・tag）は $state.raw = 丸ごと置き換えるだけ（push / splice しない = grep で確認）。
// wk の深い proxy に入れると読むたびに要素まで proxy に包まれる（1 万件の木の組み直しごと）ので、wk には getter / setter で見せる
// （wk.pages の読みは raw の信号を読む = 置き換えで派生（木・タブの題名・チャットの [[…]]）が組み直る・要素はそのままの object）
let pagesRaw = $state.raw<WikiListEntry[]>([]);
let resultsRaw = $state.raw<WikiSearchHit[]>([]);
let tagsRaw = $state.raw<WikiTagCount[]>([]);
let inboxRaw = $state.raw<WikiInboxFile[]>([]);
let chatLogRaw = $state.raw<WikiInboxFile[]>([]);

/** 変更なし（起動時・OFF の間）= 全走査と同じ「全部」の意味にはしない（all = false・配列は空 = 何も touched でない） */
const NO_CHANGE: WikiChanged = { all: false, added: [], removed: [], modified: [] };

export const wk = $state({
  status: null as WikiStatus | null,
  /** 木の元 = wiki_list の全件（パス順）。raw = 置き換えだけ */
  get pages(): WikiListEntry[] {
    return pagesRaw;
  },
  set pages(v: WikiListEntry[]) {
    pagesRaw = v;
  },
  treeLoading: false,
  treeError: null as string | null,
  /** 開いたフォルダ（vault 相対・永続しない） */
  open: new SvelteSet<string>(),
  /** 前面の Wiki タブで表示中のページ（WikiPage が読み、WikiInfo が使う） */
  page: null as WikiPageData | null,
  pageError: null as string | null,
  /** 表示中のページの見出し（描画部品の onHeadings） */
  headings: [] as WikiHeading[],
  /** 目次のクリック等 → 描画部品の scrollTo */
  scrollTo: null as { id: string; nonce: number } | null,
  /** ⌘F → 描画部品の findRequest */
  findRequest: 0,
  /** 索引が変わった回数と、最後の変更（"wiki-changed" の payload = 種別ごとのパス。all = 全走査の後 = 全部） */
  stamp: 0,
  changed: NO_CHANGE as WikiChanged,
  /** `![[ページ]]` の読みを覚える世代（ShadowPage）。索引の変更（stamp）と GUI 自身の保存の成功で進む
      （保存から監視の event まで 0.5〜1.5 秒 = その間の描き直しに前の世代の埋め込みを出さない。stamp と別 = touched の連鎖を起こさない） */
  embedGen: 0,
  /** 読んだページの題名（path → title。wiki_read_gui の結果）。木（wiki_list）は modified（題名だけの変更）では読み直さないので、
      タブの題名は 開いた / 読み直した / 保存した ページはこちら → 無ければ木の索引（titleIndex）。木を読んだら捨てる（索引が新しい） */
  titleOverrides: new SvelteMap<string, string>(),
  /** 監視・走査の失敗（"wiki-error"）。[再索引] で消す */
  error: null as string | null,
  /** 右ペイン（WikiInfo）を出す（ui_wiki_info = "0" で隠す）。隠すと中央が右端まで広がる */
  infoOpen: true,
  /** md ページの見出しを h1〜h6 で色分けする（ui_wiki_heading_colors = "0" で切る） */
  headColors: true,
  /** 表示設定（infoOpen / headColors）を読み終えた（読むまで右ペインを描かない = 一瞬出て消える・押下が上書きされるのを防ぐ） */
  prefsReady: false,
  // ---- 検索（ヘッダーの検索欄 → 中央の結果一覧。MY LIBRARY の検索とは独立） ----
  /** 検索欄の文字列 */
  q: "",
  /** Tags 箱で選んだ tag（AND） */
  qTags: [] as string[],
  /** 中央に結果一覧を出す（結果をクリックしてページを開くと畳む・検索欄に戻ると再び出す） */
  showResults: false,
  /** 検索の結果（raw = 置き換えだけ） */
  get results(): WikiSearchHit[] {
    return resultsRaw;
  },
  set results(v: WikiSearchHit[]) {
    resultsRaw = v;
  },
  resultsTotal: 0,
  searching: false,
  searchError: null as string | null,
  /** 結果一覧の選択（検索欄の ↑↓ / Enter） */
  sel: 0,
  // ---- Tags 箱・Ingest 箱（raw = 置き換えだけ） ----
  get tags(): WikiTagCount[] {
    return tagsRaw;
  },
  set tags(v: WikiTagCount[]) {
    tagsRaw = v;
  },
  tagsTotal: 0,
  get inbox(): WikiInboxFile[] {
    return inboxRaw;
  },
  set inbox(v: WikiInboxFile[]) {
    inboxRaw = v;
  },
  inboxTotal: 0,
  /** Chat log 箱（chat-log/ = チャットの記録） */
  get chatLog(): WikiInboxFile[] {
    return chatLogRaw;
  },
  set chatLog(v: WikiInboxFile[]) {
    chatLogRaw = v;
  },
  chatLogTotal: 0,
  // ---- 編集 ----
  /** 編集中の配置: false = エディタだけ・true = 左にエディタ・右にプレビュー（永続しない） */
  split: false,
  /** 新規ページ / 新規フォルダ / 改名 / 削除のダイアログ（WikiDialogs） */
  dialog: null as
    | null
    | { kind: "new"; dir: string }
    | { kind: "newdir"; dir: string }
    | { kind: "rename"; path: string; preview: WikiRenamePreview | null; to: string }
    | { kind: "trash"; path: string; dir: boolean }
    | { kind: "discard"; path: string; error: string },
  /** 木の右クリック / ツールバーの「…」/ ヘッダーの [＋ 新規] のメニュー（fixed で出す位置）。
      file = ページ（改名・ゴミ箱）・dir = フォルダ・new = ヘッダー（dir / new の path = 作る場所のフォルダ） */
  menu: null as null | { kind: "file" | "dir" | "new" | "vault"; path: string; x: number; y: number },
  /** この起動で作った空のフォルダ（wiki_list はページしか返さない = 木に出すために覚える・永続しない） */
  newDirs: new SvelteSet<string>(),
  /** 参照フォルダ（設定 wiki_ref_dirs・絶対パス）。その下の pdf / html / 画像 / 動画 / 音声への file:// は内蔵の表示で開く */
  refDirs: [] as string[],
});

/** Tags 箱に読む tag の数（多い順の先頭） */
const TAGS_LIMIT = 300;

let nonce = 0;

// 編集のセッション（edit.svelte.ts）の知らせは toast。前面のタブを離れる前（切替・閉じる）は未保存を保存する
setNotice((m) => toast(m, 8000));
// 保存できたら wk.page（右ペイン・表示へ戻したときの本文）を手元の値で揃える（読み直しを待たない・古い読みを adopt しない）
setOnSaved((path, content, r) => {
  wk.embedGen++; // 保存したページを埋め込んでいる描画は次から読み直す（監視の event を待たない）
  if (wk.page?.path === path) {
    wk.page = { ...wk.page, content, sha256: r.sha256, size: r.size, mtime: r.mtime, chars: content.length, truncated: false };
  }
});
setLeaveGuard(() => {
  const r = leave();
  if (r === true) return true;
  return r.then((ok) => ok || afterFail());
});
// Wiki のモーダルが開いている間はタブの操作（⌘1〜9・⌘⇧]・⌘T・⌘W・クリック）と ⌘⌥I を受け付けない（state の tabOpsAllowed / modalOpen）。
// WikiDialogs は WikiPage の中に居る = タブを外れると消え、戻ると古いダイアログが出る。conflict（WikiConflict）・チャットの説明（WikiChatConsent）・
// 救出（WikiRescue）も同じ扱い。メニュー（wk.menu）はモーダルではない（外側の押下で閉じる）
setModalGuard(() => !!wk.dialog || ed.dialog || chat.consent || ed.stranded.length > 0);

/** 前面の Wiki のページを離れてよいか（未保存を保存する。保存できなければ破棄して移るかを尋ねる） */
export async function canLeave(): Promise<boolean> {
  const r = leave();
  if (r === true) return true;
  return (await r) || afterFail();
}

/** 保存できずに離れられない: conflict ならダイアログが出ている（false）・書けない状態なら「変更を破棄して移る」を尋ねる */
function afterFail(): Promise<boolean> | false {
  if (!ed.path || ed.state === "conflict") return false;
  toast(`✗ ${t("保存できていない変更があります（保存できるまで移れません）")}`, 8000);
  return askDiscard();
}

let discardAnswer: ((ok: boolean) => void) | null = null;
/** 「変更を破棄して移る」の確認（WikiDialogs）。答えるまで待つ */
function askDiscard(): Promise<boolean> {
  discardAnswer?.(false);
  wk.dialog = { kind: "discard", path: ed.path ?? "", error: ed.error ?? "" };
  return new Promise((r) => (discardAnswer = r));
}

/** 確認の答え（true = 編集を捨てて移る） */
export function answerDiscard(ok: boolean) {
  const r = discardAnswer;
  discardAnswer = null;
  if (wk.dialog?.kind === "discard") wk.dialog = null;
  if (ok) {
    // 破棄した = タブも表示に戻す（後段で遷移が中止されても、セッションの無いエディタを残さない）
    for (const tab of app.tabs) if (tab.wiki) tab.wiki.editing = false;
    discardEdit();
  }
  r?.(ok);
}

/** 編集を書かずに畳む（Wiki を無効にした・vault のフォルダが変わった）。未保存は ed.stranded（WikiRescue）へ。
    tryFlush = 畳む前に今の root へ保存を試みる（無効化 = 設定が先に変わっていれば失敗して残る）。どのタブも表示に戻す */
async function closeEdit(reason: string, tryFlush: boolean) {
  // 先に保存を試みて書かずに畳んでから表示に戻す（editing を先に外すと WikiPage の stopEdit がもう 1 本書きに行く）
  if (ed.path) {
    if (tryFlush) await flush().catch(() => false);
    strandEdit(reason);
  }
  for (const tab of app.tabs) if (tab.wiki) tab.wiki.editing = false;
}

export function wikiArgs(name: string, args: Record<string, unknown> = {}) {
  return invoke<any>(name, { args });
}

/** 参照フォルダのページ = OpenNote のタブの path が絶対パス（ワークスペース相対は `/` で始まらない） */
export function isRefPath(p: string | null | undefined): boolean {
  return !!p && p.startsWith("/");
}

/** 1 ページを読む。既定の 20 万字では KB の最大級（425KB）が切れるので 200 万字まで（core の 16MB 上限はそのまま）。
    絶対パス = 参照フォルダの中のファイル（wiki_read_ref・索引に触らない） */
export function readPage(path: string): Promise<WikiPageData> {
  if (isRefPath(path)) return wikiArgs("wiki_read_ref", { abs: path, max_chars: 2_000_000 });
  return wikiArgs("wiki_read_gui", { path, max_chars: 2_000_000 }).then((pg: WikiPageData) => {
    noteTitle(pg);
    return pg;
  });
}

/** 読んだページの題名を覚える（wikiTabTitle が木の索引より先に引く）。参照フォルダのファイルは索引に無い = 覚えない */
export function noteTitle(pg: WikiPageData) {
  if (pg.external) return;
  wk.titleOverrides.set(pg.path, pg.title);
}

/** 参照フォルダの一覧を読む（起動時 = loadWikiEnabled・有効にしたとき・wiki-state のたび）。読めなければ今のまま（toast） */
export async function loadRefDirs() {
  try {
    const r = await invoke<{ dirs: string[] }>("wiki_ref_dirs_get");
    wk.refDirs = r.dirs ?? [];
  } catch (e) {
    toast(`✗ ${t("参照フォルダを読めません")}: ${String(e).slice(0, 160)}`);
  }
}

/** 左の木のワークスペース名のメニュー「OpenNote のフォルダを選び直す…」= Rust のダイアログ（検査・保存・適用・toast は Rust）。取り消しは null */
export async function pickRoot() {
  try {
    await invoke("wiki_pick_root");
  } catch (e) {
    console.warn("wiki_pick_root:", e);
  }
}

export function backlinks(arg: { path: string } | { key: string }): Promise<{ total: number; pages: WikiBacklink[] }> {
  return wikiArgs("wiki_backlinks", arg);
}

/** vault 名（root の最後の区画） */
export function vaultName(): string {
  const r = wk.status?.root ?? "";
  return r.split("/").filter(Boolean).pop() ?? "";
}

/** path → 題名の索引。木（wk.pages）の配列ごとに 1 回だけ組む（WeakMap = 配列が置き換わるまで同じ Map。タブの描画ごとの線形探索をしない） */
const titleIndexes = new WeakMap<WikiListEntry[], Map<string, string>>();
export function titleIndex(pages: WikiListEntry[]): Map<string, string> {
  let m = titleIndexes.get(pages);
  if (!m) {
    m = new Map();
    for (const p of pages) m.set(p.path, p.title);
    titleIndexes.set(pages, m);
  }
  return m;
}

/** OpenNote のタブの題名: 読んだページの題名（titleOverrides）→ 索引の題名 → ファイル名（拡張子なし）。ホームは OpenNote */
export function wikiTabTitle(w: TabWiki): string {
  if (!w.path) return "OpenNote";
  const title = wk.titleOverrides.get(w.path) || titleIndex(wk.pages).get(w.path);
  if (title) return title;
  const base = w.path.split("/").pop() ?? w.path;
  return base.replace(/\.(md|html?)$/i, "");
}

/** 起動時: 有効 / 無効だけを設定から読む（wiki_settings = 索引に触らない）。restoreTabs が Wiki タブを戻すかに使う。
    状態と木（索引の走査を待ちうる）は後で loadWikiStatus が読む = 起動の MY LIBRARY を Wiki の走査で待たせない。
    読めなければ無効として扱い、知らせる */
export async function loadWikiEnabled() {
  try {
    const s = await invoke<{ enabled: boolean; root: string | null }>("wiki_settings");
    if (s.enabled) await loadWikiPrefs();
    app.wikiOn = !!s.enabled;
    // 参照フォルダの一覧も起動時に読む（loadWikiStatus は「無効 → 有効の遷移」でしか読まず、起動はここで先に wikiOn = true に
    // なるので読めなかった = 設定には残っているのに一覧が空で、リンクが外部で開いた）
    if (s.enabled) void loadRefDirs();
    // 状態（wiki_status）が届くまでの root（起動直後の Wiki タブの asset URL・vault 名）。届いたら canonical な値で上書き
    if (s.enabled && !wk.status) wk.status = { enabled: true, root: s.root };
  } catch (e) {
    app.wikiOn = false;
    toast(`✗ ${t("OpenNote の状態を読めません")}: ${String(e).slice(0, 160)}`);
  }
}

let prefsLoaded = false;
/** 読込中に押された表示設定（読み終えた値で押下を上書きしない） */
const prefsTouched = new Set<string>();
/** 表示設定（右ペインの開閉・見出しの色分け）を読む。Wiki が有効になったときに 1 回だけ（無効の間は読まない）。
    読み終えるまで右ペインを描かない（wk.prefsReady）。失敗しても既定の値で描く（次に有効になったとき読み直す） */
async function loadWikiPrefs() {
  if (prefsLoaded) return;
  prefsLoaded = true;
  try {
    const [info, hc] = await Promise.all([api.getUiPref("ui_wiki_info"), api.getUiPref("ui_wiki_heading_colors")]);
    if (!prefsTouched.has("ui_wiki_info")) wk.infoOpen = info !== "0";
    if (!prefsTouched.has("ui_wiki_heading_colors")) wk.headColors = hc !== "0";
  } catch (e) {
    prefsLoaded = false;
    toast(`✗ ${t("表示設定を読めません")}: ${String(e).slice(0, 160)}`);
  } finally {
    wk.prefsReady = true;
  }
}

function savePref(key: string, on: boolean) {
  prefsTouched.add(key);
  api.setUiPref(key, on ? "1" : "0").catch((e) => toast(`✗ ${t("表示設定を保存できません")}: ${String(e).slice(0, 160)}`));
}

/** 右ペインの開閉（WikiHeader のボタン・⌘⌥I） */
export function toggleInfo() {
  wk.infoOpen = !wk.infoOpen;
  savePref("ui_wiki_info", wk.infoOpen);
}

/** ⌘⌥I（右ペインの開閉）を受け付けるか = 前面が Wiki で、モーダル（本体・Wiki のダイアログ）が開いていない（App の keydown が見る） */
export function infoKeyAllowed(): boolean {
  return !!frontWiki() && !modalOpen();
}

/** md ページの見出しの色分けの切替（WikiPage のツールバー） */
export function toggleHeadColors() {
  wk.headColors = !wk.headColors;
  savePref("ui_wiki_heading_colors", wk.headColors);
}

let statusTimer: ReturnType<typeof setTimeout> | undefined;
/** 知らせ済みの索引の作り直し（wiki_status の recovered.at） */
let lastRecovered: number | null = null;
let statusSeq = 0;

/** wiki_status を読み、app.wikiOn を揃える。無効になったら全タブの Wiki を外す（一覧に戻る = 痕跡を残さない）。
    索引を作り直している間は 1 秒ごとに読み直し、終わったら木を読む。読めなければ（IPC の失敗）有効 / 無効は wiki_settings で決め、
    失敗はヘッダーの警告（wk.status.statusError）に出して 10 秒後に読み直す（Wiki タブは外さない） */
export async function loadWikiStatus() {
  clearTimeout(statusTimer);
  const seq = ++statusSeq;
  let st: WikiStatus;
  try {
    st = await wikiArgs("wiki_status");
  } catch (e) {
    if (seq !== statusSeq) return;
    // 状態の失敗を「Wiki 無効」と扱わない（Wiki タブを保存から消さない）。有効 / 無効は設定（wiki_settings = 索引に触らない）で
    // 決め、失敗はヘッダーの警告に出して 10 秒後に読み直す。設定も読めなければ今の有効 / 無効のまま
    let s: { enabled: boolean; root: string | null } | null = null;
    try {
      s = await invoke<{ enabled: boolean; root: string | null }>("wiki_settings");
    } catch {
      /* 今のまま */
    }
    if (seq !== statusSeq) return;
    if (s && !s.enabled) {
      await closeEdit(t("OpenNote が無効になりました"), true);
      wk.status = { enabled: false, root: s.root };
      setWikiOn(false);
      return;
    }
    const msg = String(e);
    if (wk.status?.statusError !== msg) toast(`✗ ${t("OpenNote の状態を読めません")}: ${msg.slice(0, 160)}`);
    wk.status = { ...(wk.status ?? { root: s?.root ?? null }), enabled: true, statusError: msg };
    if (s?.enabled && !app.wikiOn) setWikiOn(true);
    statusTimer = setTimeout(() => void loadWikiStatus(), 10_000);
    return;
  }
  if (seq !== statusSeq) return;
  if (st.recovered && st.recovered.at !== lastRecovered) {
    lastRecovered = st.recovered.at;
    toast(t("OpenNote の索引が壊れていたので作り直しました"), 6000);
  }
  const was = app.wikiOn;
  const prevRoot = wk.status?.root;
  // 無効になった = 畳む前に保存を試み、書けなければ本文を残す（画面を切り替える前に知らせる）
  if (!st.enabled && ed.path) await closeEdit(t("OpenNote が無効になりました"), true);
  // vault のフォルダが変わった = 新しい root へは書かない（保存を止めて本文を残す）
  else if (was && prevRoot && st.root && prevRoot !== st.root && ed.path) {
    await closeEdit(t("フォルダが変わったので保存を止めました"), false);
    toast(`⚠ ${t("フォルダが変わったので保存を止めました")}`, 8000);
  }
  if (seq !== statusSeq) return;
  wk.status = st;
  setWikiOn(!!st.enabled);
  if (!st.enabled) return;
  if (!was) void loadRefDirs();
  if (st.rebuilding) {
    statusTimer = setTimeout(() => void loadWikiStatus(), 1000);
    return;
  }
  if (prevRoot !== st.root) wk.newDirs.clear(); // 覚えていた空のフォルダは前の vault のもの
  if (!st.error && (!was || prevRoot !== st.root || !wk.pages.length)) await loadAll();
}

function setWikiOn(on: boolean) {
  app.wikiOn = on;
  if (on) {
    void loadWikiPrefs();
    return;
  }
  for (const tab of app.tabs) tab.wiki = null;
  // 開いたままのダイアログ・メニューは畳む（WikiDialogs が消えても状態が残ると、次に ON にしたとき古いダイアログが出る）。
  // 「変更を破棄して移る」の確認を待っている切替・閉じるは「移らない」で終える（約束を宙に残さない）
  answerDiscard(false);
  wk.dialog = null;
  wk.menu = null;
  chat.consent = false; // WikiChatConsent は OFF で描かれなくなる = 見えないモーダルがタブ操作を止め続けないように
  wk.pages = [];
  wk.page = null;
  wk.headings = [];
  wk.error = null;
  wk.q = "";
  wk.qTags = [];
  wk.showResults = false;
  wk.results = [];
  wk.tags = [];
  wk.inbox = [];
  wk.chatLog = [];
  wk.newDirs.clear();
}

/** 木・Tags 箱・Ingest 箱・Chat log 箱を全部読み直す（状態の読み直し・画面からの作成 / 改名 / ゴミ箱の後）。
    監視の通知（wiki-changed）は種別で読む物を選ぶ（initWiki） */
export async function loadAll() {
  if (!app.wikiOn) return;
  void loadTags();
  void loadIngest();
  void loadChatLog();
  await loadTree();
}

let treeSeq = 0;
/** 木を読み直す（vault の全ページ = 1 回の wiki_list・箱は読まない）。作り直しの間のエラー（code = rebuilding）は状態の読み直しに任せる */
export async function loadTree() {
  if (!app.wikiOn) return;
  const seq = ++treeSeq;
  wk.treeLoading = true;
  try {
    const r = await wikiArgs("wiki_list", { limit: 1_000_000 });
    if (seq !== treeSeq) return;
    wk.pages = r.pages ?? [];
    wk.titleOverrides.clear(); // 索引の題名が新しい
    wk.treeError = null;
  } catch (e) {
    if (seq !== treeSeq) return;
    wk.treeError = String(e);
    if (errorCode(e) === WIKI_CODES.rebuilding) void loadWikiStatus();
  } finally {
    if (seq === treeSeq) wk.treeLoading = false;
  }
}

let tagsSeq = 0;
/** Tags 箱（wiki_tags・多い順の先頭 TAGS_LIMIT 件）。失敗は箱の中ではなく toast（木は読めている） */
async function loadTags() {
  if (!app.wikiOn) return;
  const seq = ++tagsSeq;
  try {
    const r = await wikiArgs("wiki_tags", { limit: TAGS_LIMIT });
    if (seq !== tagsSeq) return;
    wk.tags = r.tags ?? [];
    wk.tagsTotal = r.total ?? wk.tags.length;
  } catch (e) {
    if (seq === tagsSeq && errorCode(e) !== WIKI_CODES.rebuilding) toast(`✗ ${t("tag を読めません")}: ${String(e).slice(0, 160)}`);
  }
}

let inboxSeq = 0;
/** Ingest 箱（wiki_inbox = ingest/ の中の未処理の素材。OpenRef は動かさない）。Chat log 箱（loadChatLog）とは独立（片方の失敗で他方を止めない・知らせはそれぞれ） */
async function loadIngest() {
  if (!app.wikiOn) return;
  const seq = ++inboxSeq;
  try {
    const r = await wikiArgs("wiki_inbox", { limit: 500 });
    if (seq !== inboxSeq) return;
    wk.inbox = r.files ?? [];
    wk.inboxTotal = r.total ?? wk.inbox.length;
  } catch (e) {
    if (seq === inboxSeq && errorCode(e) !== WIKI_CODES.rebuilding) toast(`✗ ${t("ingest/ を読めません")}: ${String(e).slice(0, 160)}`);
  }
}

let chatLogSeq = 0;
/** Chat log 箱（chat-log/ = チャットの記録。応答中の追記 = wiki-changed の modified に chat-log/ が載るたび = 木は読まずこれだけ） */
async function loadChatLog() {
  if (!app.wikiOn) return;
  const seq = ++chatLogSeq;
  try {
    const c = await wikiArgs("wiki_inbox", { limit: 500, dir: "chat-log" });
    if (seq !== chatLogSeq) return;
    wk.chatLog = c.files ?? [];
    wk.chatLogTotal = c.total ?? wk.chatLog.length;
  } catch (e) {
    if (seq === chatLogSeq && errorCode(e) !== WIKI_CODES.rebuilding) toast(`✗ ${t("chat-log/ を読めません")}: ${String(e).slice(0, 160)}`);
  }
}

/** チャットのログ（Rust の wiki_chat_log が追記だけする）。応答中は約 1 秒ごとに wiki-changed に載る */
const isChatLog = (p: string) => p.startsWith("chat-log/");
/** Ingest 箱のフォルダ */
const isIngest = (p: string) => p.startsWith("ingest/");
/** 変更に入る全部のパス（種別を問わず） */
const changedPaths = (c: WikiChanged): string[] => [...c.added, ...c.removed, ...c.modified];

// ---------- 検索 ----------

let searchTimer: ReturnType<typeof setTimeout> | undefined;
let searchSeq = 0;

/** 検索の条件があるか（語か tag） */
export function hasQuery(): boolean {
  return wk.q.trim() !== "" || wk.qTags.length > 0;
}

/** 検索欄の入力・tag の選択の後に呼ぶ（debounce 200ms）。条件が空なら結果を畳む */
export function searchSoon(delay = 200) {
  clearTimeout(searchTimer);
  if (!hasQuery()) {
    ++searchSeq;
    wk.showResults = false;
    wk.results = [];
    wk.resultsTotal = 0;
    wk.searching = false;
    wk.searchError = null;
    return;
  }
  wk.showResults = true;
  searchTimer = setTimeout(() => void runSearch(), delay);
}

/** keepSel = 裏の読み直し（wiki-changed）: 選んでいた行と同じ path があればその位置へ・無ければ範囲に収める */
async function runSearch(keepSel = false) {
  if (!app.wikiOn || !hasQuery()) return;
  const selPath = keepSel ? wk.results[wk.sel]?.path : undefined;
  const seq = ++searchSeq;
  wk.searching = true;
  try {
    const args: Record<string, unknown> = { limit: 200 };
    if (wk.q.trim()) args.query = wk.q;
    if (wk.qTags.length) args.tags = [...wk.qTags];
    const r = await wikiArgs("wiki_search", args);
    if (seq !== searchSeq) return;
    wk.results = r.pages ?? [];
    wk.resultsTotal = r.total ?? wk.results.length;
    wk.searchError = null;
    if (keepSel) {
      const i = selPath ? wk.results.findIndex((h) => h.path === selPath) : -1;
      wk.sel = i >= 0 ? i : Math.min(wk.sel, Math.max(0, wk.results.length - 1));
    } else wk.sel = 0;
  } catch (e) {
    if (seq !== searchSeq) return;
    // 記号だけの語（英数字も和文も無い = bad_query）は 0 件として扱う
    wk.results = [];
    wk.resultsTotal = 0;
    wk.searchError = errorCode(e) === WIKI_CODES.badQuery ? null : String(e);
  } finally {
    if (seq === searchSeq) wk.searching = false;
  }
}

let changeTimer: ReturnType<typeof setTimeout> | undefined;
/** 1 秒のまとめの間に来た変更の和（種別ごと・重複なし）。null = 待っている変更なし */
let pending: { all: boolean; added: Set<string>; removed: Set<string>; modified: Set<string> } | null = null;

/** まとめた変更から読み直す物を決める（純関数 = テストで固定）:
    木 = 増減（all / added / removed）のとき、または modified に木が知らないパス（known でない）があるとき
    （core が同期で索引に入れた書込 = GUI の保存・チャット / MCP の wiki_write・取込 は監視が拾う時点で索引にある = modified で届く。
    画面が知らない modified は増として扱う = 新しいページが木に出る）・Tags 箱 = 毎回（ログの `#tag` も索引に入る）・
    Ingest / Chat log 箱 = all か、そのフォルダのパスが（種別を問わず）入るとき */
export function reloadPlan(
  c: { all: boolean; added: Iterable<string>; removed: Iterable<string>; modified: Iterable<string> },
  known: (p: string) => boolean = () => true
): {
  tree: boolean;
  tags: boolean;
  ingest: boolean;
  chatLog: boolean;
} {
  const added = [...c.added];
  const removed = [...c.removed];
  const modified = [...c.modified];
  const paths = [...added, ...removed, ...modified];
  return {
    tree: c.all || added.length > 0 || removed.length > 0 || modified.some((p) => !known(p)),
    tags: true,
    ingest: c.all || paths.some(isIngest),
    chatLog: c.all || paths.some(isChatLog),
  };
}

/** payload の形を守る（Rust の WikiChanged。欠けた欄・配列でない欄は空・all でないものとして扱う = 古い形の string[] は何も touched でない） */
function asChanged(p: unknown): WikiChanged {
  const o = (p && typeof p === "object" && !Array.isArray(p) ? p : {}) as Partial<Record<keyof WikiChanged, unknown>>;
  const arr = (v: unknown): string[] => (Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : []);
  return { all: o.all === true, added: arr(o.added), removed: arr(o.removed), modified: arr(o.modified) };
}

/** Rust の event を受ける（App の onMount から。戻り値 = 解除） */
export function initWiki(): () => void {
  let disposed = false;
  const offs: (() => void)[] = [];
  // 解除が listen の完了より先に走っても取りこぼさない（解除の後に届いた off はその場で呼ぶ = initChat と同じ型）
  const on = <T>(name: string, f: (p: T) => void) =>
    listen<T>(name, (e) => f(e.payload)).then((off) => (disposed ? off() : offs.push(off)));
  void on("wiki-state", () => {
    void loadWikiStatus();
    void loadRefDirs();
  });
  // ⌘Q・窓の ×（Wiki 有効時だけ Rust が終了を保留して出す）= 保存してから答える。答えないと Rust は 15 秒で終了する
  void on("wiki-exit-request", () => void exitRequest());
  void on<WikiChanged>("wiki-changed", (payload) => {
    if (!app.wikiOn) return;
    const c = asChanged(payload);
    wk.changed = c;
    wk.stamp++;
    wk.embedGen++;
    // 木・Tags・箱・検索の結果は 1 秒まとめて読み直す（監視は 500ms ごとのバッチ = 大量の変更が続いても読み直しは 1 秒に 1 回まで）。
    // 読む物は種別で選ぶ（reloadPlan）: 木（全ページ = 1 回の wiki_list）は増減のときだけ = 応答中のログの追記（modified）では読まない。
    // 種別は Rust が core の取り込み結果から付ける = 画面は箱の中身で推測しない
    const q = (pending ??= { all: false, added: new Set(), removed: new Set(), modified: new Set() });
    q.all ||= c.all;
    for (const p of c.added) q.added.add(p);
    for (const p of c.removed) q.removed.add(p);
    for (const p of c.modified) q.modified.add(p);
    clearTimeout(changeTimer);
    changeTimer = setTimeout(() => {
      const known = titleIndex(wk.pages);
      const plan = reloadPlan(q, (p) => known.has(p));
      pending = null;
      if (plan.tree) void loadTree();
      if (plan.tags) void loadTags();
      if (plan.ingest) void loadIngest();
      if (plan.chatLog) void loadChatLog();
      if (wk.showResults && hasQuery()) void runSearch(true);
    }, 1000);
  });
  // WebView の遷移を Rust（openref-wiki-nav の許可リスト）が止めた = PDF の iframe の中のリンク等。外部ブラウザへは渡さない
  // （Rust が 2 秒に 1 回まで間引いて出す）。Wiki の有無に依らず知らせる
  void on("nav-blocked", () => toast(t("安全のため、このリンクは開きませんでした")));
  void on<string>("wiki-error", (msg) => {
    if (!app.wikiOn) return;
    wk.error = msg;
    toast(`✗ ${msg}`.slice(0, 240), 8000);
  });
  return () => {
    disposed = true;
    clearTimeout(changeTimer);
    pending = null;
    clearTimeout(statusTimer);
    clearTimeout(searchTimer);
    offs.forEach((f) => f());
  };
}

/** 終了の要求への答え（wiki_exit_ready {proceed}）。保存できない・残った本文がある = 即座に終了を取りやめる。
    ここで確認ダイアログは出さない（答えを待つ間に Rust の 15 秒のタイムアウトが終了を進めて本文を失う）。conflict も即 false */
export async function exitRequest() {
  let proceed = true;
  if (ed.path) {
    const r = leave();
    proceed = r === true || (await r);
    if (!proceed) toast(`✗ ${t("保存できていない変更があるため終了を取りやめました（⌘S で再試行・破棄するならエディタから）")}`, 10000);
  }
  if (proceed && ed.stranded.length) {
    proceed = false;
    toast(`✗ ${t("保存できていない変更があります")}`, 8000);
  }
  try {
    await invoke("wiki_exit_ready", { proceed }); // 他の編集の command と違い args に包まない（main.rs の包みの契約）
  } catch (e) {
    console.warn("wiki_exit_ready", e);
  }
}

/** [再索引] = 監視を張り直して全走査（背景。進捗は wiki_status） */
export async function reindex() {
  wk.error = null;
  try {
    await invoke("wiki_reindex");
  } catch (e) {
    toast(`✗ ${t("OpenNote の索引を作り直せません")}: ${String(e).slice(0, 200)}`);
  }
}

/** 変わったパス（wiki-changed・種別を問わず）に p が入るか。all = 全走査 = 全部 */
export function touched(p: string | null | undefined): boolean {
  return wk.changed.all || (!!p && changedPaths(wk.changed).includes(p));
}

/** バックリンク（ページ / item を参照するページ）を読み直すべき変更か: 全走査・ps（対象と、いま出している参照元）のどれか・
    md / html のページ（参照を足した新しいページがありうる = skill が書いた要約ページを数秒で ItemDetail に出す）。
    画像・動画などのファイルだけの変更（大量のコピー）と、チャットのログ（chat-log/ = 応答中は約 1 秒ごとに追記。
    参照元に並んでいるログでも = 文脈に [[ページ]] を書いたログは毎回それ）では読み直さない */
export function touchedAny(ps: string[]): boolean {
  return wk.changed.all || changedPaths(wk.changed).some((p) => !isChatLog(p) && (ps.includes(p) || /\.(md|html?)$/i.test(p)));
}

// ---------- モード・タブ ----------

const home = (): TabWiki => ({ path: null, scrollTop: 0, editing: false });

/** セグメント [MY LIBRARY | OPENNOTE] の押下（§3.1）。一覧タブ ⇄ Wiki ホームはそのタブを変える・PDF タブからは新しいタブ */
export async function switchMode(to: "library" | "wiki") {
  const tab = app.tabs.find((x) => x.id === app.tab);
  if (!tab || !tabOpsAllowed()) return; // 処理中・モーダル・エディタ・取込中はタブの切替と同じく受け付けない
  if (tab.wiki && to === "library" && !(await canLeave())) return; // 編集中の未保存を保存してから
  if (tab.viewer) {
    await openTab({ kind: "all" }, to === "wiki" ? home() : null);
    return;
  }
  if (to === "wiki") {
    if (!tab.wiki) tab.wiki = home();
    return;
  }
  if (!tab.wiki) return;
  tab.wiki = null;
  await setView({ kind: "all" });
}

/** ページを開く。newTab = 新しい Wiki タブ（⌘クリック・ItemDetail の Wiki 節）・それ以外は前面の Wiki タブの中で遷移 */
export async function openWikiPage(path: string | null, newTab = false, heading?: string, edit = false) {
  if (!app.wikiOn) return;
  // 遷移は呼んだときのタブに対して（保存を待つ間に別のタブへ切り替わっても、そのタブの中身を変えない）
  const tabId = app.tab;
  // 同じタブの中で別のページへ = 編集中の未保存を保存してから（保存できなければ移らない）
  if (!newTab && frontWiki() && frontWiki()!.path !== path && !(await canLeave())) return;
  if (!newTab && app.tab !== tabId) return;
  pendingHeading = heading ?? null;
  const front = frontWiki();
  // ページが変わる = 前のページの見出しへの scrollTo を消す（描画の遅い html ページが前の id へ着地しない）
  if (newTab || front?.path !== path) wk.scrollTo = null;
  if (newTab || !front) {
    await openTab({ kind: "all" }, { path, scrollTop: 0, editing: edit });
    return;
  }
  if (front.path === path) {
    if (heading) jumpToHeading(heading);
    if (edit) front.editing = true;
    return;
  }
  front.path = path;
  front.scrollTop = 0;
  front.editing = edit; // 別のページは表示で開く（新規ページだけ編集で）
}

/** `[[name#heading]]` の着地を待つ見出し（ページの読込 → onHeadings で解く） */
let pendingHeading: string | null = null;

export function setHeadings(h: WikiHeading[]) {
  wk.headings = h;
  if (pendingHeading) {
    const want = pendingHeading;
    if (jumpToHeading(want)) pendingHeading = null;
  }
}

/** 見出し（id か本文）へスクロール。見つかったら true。本文は render の headingKey で緩く照合（埋め込みの節・同じページの #… と同じ規則） */
export function jumpToHeading(idOrText: string): boolean {
  const h = findHeading(wk.headings, idOrText);
  if (!h) return false;
  wk.scrollTo = { id: h.id, nonce: ++nonce };
  return true;
}

/** md 以外（pdf / html / 画像 / 動画 等）へのリンクは常に新しいタブで開く（user: 読んでいるページを離れない）。
    同じファイルのタブが既にあればそれへ切り替える（同じ PDF を何枚も開かない） */
export function opensInNewTab(path: string): boolean {
  return viewKindOf(path) !== "page" || /\.html?$/i.test(path);
}
async function openInOwnTab(path: string, heading?: string) {
  const had = app.tabs.find((x) => x.wiki?.path === path);
  if (had) {
    if (had.id === app.tab) {
      if (heading) jumpToHeading(heading);
    } else {
      // 切り替え先の見出しはまだ読めていない（wk.headings は前のタブのもの）= 読めたら着地する（setHeadings）
      pendingHeading = heading ?? null;
      await activateTab(had.id);
    }
    return;
  }
  await openWikiPage(path, true, heading);
}

/** 描画部品の遷移の要求を実行する（WikiPage の onNavigate） */
export async function navigate(nav: WikiNav, newTab = false) {
  try {
    switch (nav.kind) {
      case "page":
        if (opensInNewTab(nav.path)) await openInOwnTab(nav.path, nav.heading);
        else await openWikiPage(nav.path, newTab, nav.heading);
        return;
      case "item":
        // 頁の指定（page / att）は未対応。ここでは MY LIBRARY に出すだけ
        await reveal(nav.key);
        return;
      case "external":
        // http(s) は設定（Settings › OpenNote › リンク（URL）を OpenRef の窓で開く）で別窓 / 外部ブラウザ。mailto 等は今どおり
        if (/^https?:/i.test(nav.url)) await invoke("wiki_open_url", { url: nav.url });
        else await api.openExternal(nav.url);
        return;
      case "open":
        await openPath(nav.abs);
        return;
      case "ref":
        // Rust が拒否する（参照フォルダの外へ出る symlink・無い・未マウント・一覧が古い）ならホストの open に落とす（参照フォルダが無いときと同じ）
        try {
          await wikiArgs("wiki_read_ref", { abs: nav.abs, probe: true });
        } catch (e) {
          console.warn("wiki_read_ref probe:", e);
          await openPath(nav.abs);
          return;
        }
        await openInOwnTab(nav.abs); // 参照フォルダのファイルは全部 md 以外 = 常に自分のタブ
        return;
    }
  } catch (e) {
    toast(`✗ ${t("開けません")}: ${String(e).slice(0, 200)}`);
  }
}

/** vault の中の実在ファイルをホストのアプリで開く（wiki_open_path） */
export async function openPath(path: string) {
  try {
    // 文書の拡張子以外・実行ビット付き・「このアプリで開く」の指定付きは Rust が open せず Finder で場所を表示する（true）
    if (await invoke<boolean>("wiki_open_path", { path })) toast(t("安全のため、開かずに Finder で表示しました"));
  } catch (e) {
    toast(`✗ ${t("外部で開けません")}: ${String(e).slice(0, 200)}`);
  }
}

/** ⌘F（表示中の描画部品のページ内検索） */
export function requestFind() {
  wk.findRequest++;
}

// ---------- 編集 ----------

/** 表示 ⇄ 編集（⌘E・ツールバー）。表示へ戻すときは保存してから（保存できなければ編集のまま） */
export async function setEditing(on: boolean, split?: boolean) {
  const w = frontWiki();
  if (!w) return;
  if (split !== undefined) wk.split = split;
  if (w.editing === on) return;
  if (!on && !(await canLeave())) return;
  if (frontWiki() === w) w.editing = on;
}

/** OS からのドロップ（App の onDragDropEvent・前面が Wiki のとき）。編集中なら caret に `![[…]]`・そうでなければ知らせるだけ
    （蔵書へは流さない = drive モードで Drive に書く事故を作らない） */
export async function wikiDrop(paths: string[]) {
  if (!paths.length) return;
  if (!(await dropPaths(paths))) toast(t("ページへは編集モードでドロップしてください（蔵書には取り込みません）"));
}
