// OpenNote のチャットの画面側の状態と動詞。受け渡しは types.ts の「チャットの契約」だけ:
// command = wiki_chat_status / wiki_chat_set_enabled / wiki_chat_start / wiki_chat_send / wiki_chat_interrupt /
// wiki_chat_undo / wiki_chat_set_model / wiki_chat_auth / wiki_chat_stop（invoke(name, { args })。stop は別のワークスペースの会話を替えるときだけ）・
// Tauri event "wiki-chat"（line / stderr / state / changes / exit）と "wiki-chat-consent"。
// stream-json の解釈は chat.ts の reduceChat（純粋）。OpenRef は会話本文を保存しない（この画面の表示は起動の間だけ）。
// - 面はアプリ右端の柱 1 本（ChatDock・開閉 = タブ列右端のボタン / ⌘⌥C・ui_chat_dock）。中身は WikiChat 1 つ
// - チャットが無効（status.enabled = false）の間は柱もボタンも出さない・子プロセスも起こさない（初回の送信で起こす）
// - Wiki が無効の間は event を積まない・invoke しない
// - 自動で起こし直さない（異常終了は知らせるだけ = 利用枠の嵐を作らない）
// - ワークスペース: 子は 1 本・起こした root に縛られる（start / send に前面の root）。会話の root（chat.root）と前面の root
//   （画面の root = 前面 / 最後の前面の OpenNote タブ）が違う間は会話を隠して 1 行（chatHidden）= タブの行き来では消さない・子も止めない。
//   その状態で送ると前の子を止めて（wiki_chat_stop）前面の root で起こし直す = 新しい会話
// - Claude の書込（wiki_write も Edit / Write も）はワークスペースの監視（wiki-changed）と編集の規則（edit.svelte.ts）で画面に反映される（ここでは何もしない）。
//   [元に戻す] に出るのは wiki_write / wiki_move の変更だけ

import { untrack } from "svelte";
import { invoke } from "../wiki/ipc";
import { listen } from "@tauri-apps/api/event";
import { t } from "../i18n.svelte";
import { api } from "../api";
import { app, toast } from "../state.svelte";
import { clampDockW, clipSelection, DOCK_W, emptyConv, parseDockPref, reduceChat, type ChatDockPref, type ChatInput, type Conv } from "./chat";
import { clearViewingSelection, setViewingSelection, setViewingWanted, viewing, viewingKey, viewingSource } from "../viewing.svelte";
import type {
  WikiChatChange,
  WikiChatContext,
  WikiChatEvent,
  WikiChatSendResult,
  WikiChatStartResult,
  WikiChatState,
  WikiChatStatus,
  WikiChatUndoResult,
} from "../wiki/types";
import { shownRoot } from "../wiki/wiki.svelte";

/** 診断（stderr）に持つ行数・1 行の長さの上限（古い行から捨てる・長い行は切る） */
export const STDERR_MAX = 300;
export const STDERR_LINE_MAX = 2000;
/** 新しい会話の後、前の子プロセスの終了を異常として知らせない間（ms） */
const EXPECT_EXIT_MS = 15_000;

export const chat = $state({
  status: null as WikiChatStatus | null,
  /** 今の会話で動いているモデル(system/init の model。子が起きるまで null) */
  model: null as string | null,
  /** wiki_chat_status の呼び出し自体の失敗（画面に出す） */
  statusError: null as string | null,
  /** 柱（ChatDock）の開閉と幅（ui_chat_dock = JSON で覚える。どのタブ種でも共通） */
  dock: { open: false, w: DOCK_W } as ChatDockPref,
  /** 柱を開いた = 入力欄へ明示 focus する（WikiChat が mount の後でも拾う。WKWebView は autofocus が効かない） */
  focusPending: false,
  conv: emptyConv() as Conv,
  /** 今の会話の id（event の session_id と違えば別の会話 = 表示を新しくする） */
  session: null as string | null,
  /** 表示している会話のワークスペース（子の root = wiki_chat_status.root・起こした root）。前面の root と違う間は会話を隠す（chatHidden） */
  root: null as string | null,
  /** 保存済みの会話を --resume で続けた（過去の発言はここに出ない旨の知らせ） */
  resumed: false,
  /** このターンで変わったもの（turn → 全体。changes event のたびに置き換え） */
  changes: {} as Record<number, WikiChatChange[]>,
  /** [元に戻す] の結果（`${turn}:${index}`） */
  undo: {} as Record<string, { busy?: boolean; ok?: boolean; reason?: string }>,
  /** 停止を押したターン（result の error を「停止しました」と出す） */
  stopped: [] as number[],
  /** turn が付く前に停止を押した発言の id（turn が付いたら stopped へ） */
  stopIds: [] as string[],
  stderr: [] as string[],
  draft: "",
  /** 起動・送信の途中（ボタンを止める） */
  sending: false,
  /** 起動・送信・停止の失敗（入力欄の上に出す） */
  error: null as string | null,
  /** 起動の知らせ（resume に失敗して新しい会話にした等 = Rust の notice） */
  notice: null as string | null,
  /** 文脈から外した元（× を押した viewingKey = ページの path か `item:<key>`。元が変われば戻る）。選択範囲は viewing.sel（viewing.svelte.ts） */
  ctxOff: null as string | null,
  /** 新しい会話の確認中 */
  confirmNew: false,
  /** Settings の [チャット] を ON にする前の説明（event wiki-chat-consent） */
  consent: false,
});

function saveDock() {
  api.setUiPref("ui_chat_dock", JSON.stringify(chat.dock)).catch((e) => toast(`✗ ${t("表示設定を保存できません")}: ${String(e).slice(0, 160)}`));
}

/** 柱を開く / 閉じる（覚える）。開いたら入力欄へ明示 focus（focusPending = 柱の WikiChat が mount の後に受ける） */
export function setDockOpen(open: boolean) {
  dockOpenTouched = true;
  chat.dock = { ...chat.dock, open };
  if (open) chat.focusPending = true;
  saveDock();
}

/** 柱の開閉（タブ列右端のボタン・⌘⌥C） */
export function toggleDock() {
  setDockOpen(!chat.dock.open);
}

/** 柱の幅を確定する（つまみを離したとき 1 回だけ = 動かしている間は呼ばない）。320〜窓幅の半分に収めて覚える */
export function setDockWidth(w: number) {
  dockWTouched = true;
  const nw = clampDockW(w, window.innerWidth);
  if (nw === chat.dock.w) return;
  chat.dock = { ...chat.dock, w: nw };
  saveDock();
}

let prefsLoaded = false;
/** 読込が返る前に user が開閉 / 幅を変えた = その値は読んだ値で上書きしない（開閉と幅は別々 = 先に開いても覚えていた幅は戻す） */
let dockOpenTouched = false;
let dockWTouched = false;
/** 前の子プロセスを止める新しい会話を始めた時刻（その後 EXPECT_EXIT_MS の exit は知らせない）。0 = なし */
let expectExitAt = 0;
/** 最後に知らせた状態の読込の失敗（同じ文を toast で繰り返さない） */
let lastStatusError: string | null = null;
let statusSeq = 0;
/** 子の状態の event（state）を受けた回数（読みの間に来たら、状態は event の方を残す） */
let stateStamp = 0;

function call<T>(name: string, args: Record<string, unknown> = {}): Promise<T> {
  return invoke<T>(name, { args });
}

function apply(input: ChatInput) {
  const c = reduceChat(chat.conv, input);
  if (c !== chat.conv) chat.conv = c;
}

/** 使える = 有効・claude が見つかった・未ログインと分かっていない・openref-mcp がある */
export function usable(s: WikiChatStatus | null = chat.status): boolean {
  return !!s && s.enabled && !!s.cli && s.logged_in !== false && !!s.mcp;
}

/** チャットの柱とボタンを出すか（Wiki が有効・チャットが有効） */
export function chatShown(): boolean {
  return app.wikiOn && !!chat.status?.enabled;
}

export function busy(): boolean {
  const st = chat.status?.state;
  return st === "busy" || st === "starting" || st === "stopping";
}

/** 表示している会話が別のワークスペースの物（会話の root ≠ 前面の root）で、隠す物がある（発言がある・子が生きている） = 会話を隠して 1 行。
    root が分からない間（起動直後・子も会話も無い）は隠さない */
export function chatHidden(): boolean {
  const front = shownRoot();
  if (!chat.root || !front || chat.root === front) return false;
  return chat.conv.entries.length > 0 || (!!chat.status && chat.status.state !== "off");
}

/** 状態を読み直す（柱を開いた・柱が開いたまま前面のタブが変わった・state / exit・wiki-state・start の後）。失敗は toast と画面に出し、無効として扱う（柱を出さない）。
    force = 画面の [再確認]（Rust が覚えた claude auth status を捨てて読む）。自動の読み直しは force なし = Rust は 30 秒の間 auth status を起こさない */
export async function loadChatStatus(opts: { force?: boolean } = {}) {
  if (!app.wikiOn) {
    chat.status = null;
    return;
  }
  const my = ++statusSeq;
  const stamp = stateStamp;
  statusLoading++;
  try {
    const s = await call<WikiChatStatus>("wiki_chat_status", opts.force ? { force: true } : {});
    if (my !== statusSeq) return; // 後から頼んだ読みが先に返った（古い状態で上書きしない）
    // 読みの間に子の状態の event が来た = 子の状態は event の方が新しい（exit の読み直しが後の idle を off で上書きしない）
    chat.status = stamp !== stateStamp && chat.status ? { ...s, state: chat.status.state } : s;
    chat.statusError = null;
    lastStatusError = null;
    if (s.enabled) void loadDockPref();
    // 子がいる = その root の会話（session_id は子の物）。子がいない = session_id は設定の root（= 前面）の保存済みの会話 =
    // 隠している別の root の会話とは比べない（タブの行き来で消さない = 戻れば見える）
    if (typeof s.root === "string" && s.root) chat.root = s.root;
    else if (chatHidden()) return;
    else chat.root = shownRoot() ?? chat.root;
    // 会話が無くなった = 子がいない（off）・送信中でない・session_id が null のときだけ表示を消す。別の会話 = 新しくする
    if (s.session_id == null) {
      if (chat.session != null && s.state === "off" && !chat.sending) newView(null);
    } else if (chat.session && s.session_id !== chat.session) newView(s.session_id);
    else if (!chat.session) chat.session = s.session_id;
  } catch (e) {
    if (my !== statusSeq) return;
    const msg = String(e);
    chat.statusError = msg;
    if (lastStatusError !== msg) toast(`✗ ${t("チャットの状態を読めません")}: ${msg.slice(0, 160)}`);
    lastStatusError = msg;
    if (chat.status) chat.status = { ...chat.status, enabled: false };
  } finally {
    statusLoading--;
  }
}
/** 読み中の数（柱を開いたときの読み直しが起動時の読みと重ならないように。読みの入れ替わりは statusSeq が守る） */
let statusLoading = 0;
export function statusInFlight(): boolean {
  return statusLoading > 0;
}

/** 柱の開閉と幅を読む（チャットが有効と分かったとき 1 回。旧キー ui_wiki_right_tab / ui_lib_right_tab / ui_wiki_chat_float は読まない） */
async function loadDockPref() {
  if (prefsLoaded) return;
  prefsLoaded = true;
  try {
    const p = parseDockPref(await api.getUiPref("ui_chat_dock"));
    chat.dock = { open: dockOpenTouched ? chat.dock.open : p.open, w: dockWTouched ? chat.dock.w : p.w };
    // 読みの前に触った = その時に書いた値は読んだ側（幅など）を知らない = 合わせた値を書き直す
    if (dockOpenTouched || dockWTouched) saveDock();
  } catch (e) {
    prefsLoaded = false;
    toast(`✗ ${t("表示設定を読めません")}: ${String(e).slice(0, 160)}`);
  }
}

/** 立っている focusPending を el（WikiChat の入力欄）で受ける = focus して消費する。見えていない el は消費しない = 見えたときに受ける
    （面は柱 1 つだが、明示の focus は残す = WKWebView は autofocus が効かない）。focus が移らなかったときも消費しない。
    見えているが disabled（使えない = 未ログイン等）なら focus せずに消費する（残すと、使えるようになった後の mount のし直しで
    エディタ等から focus を奪う） */
export function claimFocus(el: HTMLElement): boolean {
  if (!chat.focusPending) return false;
  if (el.checkVisibility?.({ visibilityProperty: true, checkVisibilityCSS: true }) === false) return false;
  if (el.matches(":disabled")) {
    chat.focusPending = false;
    return false;
  }
  el.focus();
  if (document.activeElement !== el) return false;
  chat.focusPending = false;
  return true;
}

/** 表示を新しい会話にする（本文は Claude Code が持つ = ここでは捨てるだけ） */
function newView(session: string | null) {
  chat.conv = emptyConv();
  chat.changes = {};
  chat.undo = {};
  chat.stopped = [];
  chat.stopIds = [];
  chat.session = session;
  chat.resumed = false;
  chat.notice = null;
  chat.model = null;
}

/** 子プロセスを起こす（new = 新しい会話・root = そのワークスペースで = 前面の root）。起こせなければ例外。
    成否によらず返った後に状態を読み直す（starting のまま固まらない） */
async function start(fresh: boolean, root: string | null = shownRoot()): Promise<void> {
  if (chat.status) chat.status = { ...chat.status, state: "starting" };
  try {
    const r = await call<WikiChatStartResult>("wiki_chat_start", root ? { new: fresh, root } : { new: fresh });
    // 別のワークスペースの会話から替えた = 前の会話の表示は捨てる（同じ session_id でも = root ごとの会話）
    if (fresh || (chat.session && chat.session !== r.session_id) || (root && chat.root && chat.root !== root)) newView(r.session_id);
    if (root) chat.root = root;
    chat.session = r.session_id;
    chat.resumed = r.resumed && !chat.conv.entries.length;
    chat.notice = r.notice ?? null;
  } catch (e) {
    if (chat.status) chat.status = { ...chat.status, state: "off" };
    throw e;
  } finally {
    await loadChatStatus();
  }
}

/** 今の文脈（元と選択範囲は viewing が持つ・× で外したものは除く）。添えるものが無ければ undefined */
export function currentContext(): WikiChatContext | undefined {
  const s = viewingSource();
  const k = viewingKey(s);
  const ctx: WikiChatContext = {};
  if (s && chat.ctxOff !== k) {
    if (s.kind === "page") ctx.path = s.path;
    else {
      ctx.item = s.key;
      if (s.title) ctx.title = s.title;
    }
  }
  const sel = viewing.sel;
  if (sel && sel.key === k && sel.text) ctx.selection = clipSelection(sel.text).text;
  return ctx.path || ctx.item || ctx.selection ? ctx : undefined;
}

/** 刈った発言より古いターンの changes / undo / stopped を捨てる */
function trimTurns() {
  if (!chat.conv.dropped) return;
  const turns = chat.conv.entries.map((e) => e.turn).filter((n): n is number => n != null);
  if (!turns.length) return;
  const min = Math.min(...turns);
  const old = Object.keys(chat.changes).filter((k) => Number(k) < min);
  if (old.length) {
    const ch = { ...chat.changes };
    for (const k of old) delete ch[Number(k)];
    chat.changes = ch;
    chat.undo = Object.fromEntries(Object.entries(chat.undo).filter(([k]) => Number(k.split(":")[0]) >= min));
  }
  if (chat.stopped.some((n) => n < min)) chat.stopped = chat.stopped.filter((n) => n >= min);
}

/** 送信（未起動なら起こしてから）。応答中・送信中は送らない。送れなければ入力欄の文章と選択を戻す */
export async function send(): Promise<boolean> {
  const text = chat.draft.trim();
  if (!text || chat.sending || busy() || !usable()) return false;
  chat.sending = true;
  chat.error = null;
  let id: string | null = null;
  const draft = chat.draft;
  const sel = viewing.sel;
  try {
    // 文脈は Enter を押したとき（起動を待つ間にタブを切り替えても、見えていたチップのものを添える）
    const ctx = currentContext();
    // 送る先 = 前面の root。会話が別のワークスペースの物（隠している） = 前の子を止めて前面の root で起こし直す = 新しい会話
    const root = shownRoot();
    const other = chatHidden();
    if (other && chat.status && chat.status.state !== "off") {
      expectExitAt = Date.now(); // 止めた子の終了を異常として知らせない
      await call("wiki_chat_stop");
    }
    if (other || !chat.status || chat.status.state === "off") await start(false, root);
    apply({ t: "user", text, ctx });
    id = chat.conv.entries[chat.conv.entries.length - 1].id;
    chat.draft = "";
    clearViewingSelection(); // 選択範囲はこの発言に添えた（次の発言には付けない）
    const args: Record<string, unknown> = ctx ? { text, context: ctx } : { text };
    if (root) args.root = root;
    const r = await call<WikiChatSendResult>("wiki_chat_send", args);
    apply({ t: "turn", id, turn: r.turn });
    apply({ t: "prune" }); // 刈るのは送れた後（送れなければ何も刈らない）
    if (chat.stopIds.includes(id)) {
      chat.stopIds = chat.stopIds.filter((x) => x !== id);
      if (!chat.stopped.includes(r.turn)) chat.stopped = [...chat.stopped, r.turn];
    }
    trimTurns();
    return true;
  } catch (e) {
    // 送れなかった発言は会話から外し、入力欄と選択を戻す（失敗の表示は入力欄の上の 1 か所）
    if (id) apply({ t: "drop", id });
    if (!chat.draft) chat.draft = draft;
    if (!viewing.sel && sel) setViewingSelection(sel.key, sel.text);
    chat.error = String(e);
    void loadChatStatus();
    return false;
  } finally {
    chat.sending = false;
  }
}

/** 停止 = このターンだけ止める（会話は続く）。turn が付く前に押しても停止として扱う */
export async function interrupt() {
  const last = [...chat.conv.entries].reverse().find((e) => e.k === "user");
  if (last?.turn != null) {
    if (!chat.stopped.includes(last.turn)) chat.stopped = [...chat.stopped, last.turn];
  } else if (last && !chat.stopIds.includes(last.id)) chat.stopIds = [...chat.stopIds, last.id];
  try {
    await call("wiki_chat_interrupt");
  } catch (e) {
    chat.error = String(e);
  }
}

/** 新しい会話（子プロセスを起こし直す = --session-id） */
export async function newConversation() {
  chat.confirmNew = false;
  if (chat.sending) return;
  chat.sending = true;
  chat.error = null;
  // 止めるべき前の子プロセスがあるときだけ、その終了を異常として知らせない
  const running = !!chat.status && chat.status.state !== "off";
  expectExitAt = running ? Date.now() : 0;
  try {
    await start(true);
  } catch (e) {
    chat.error = String(e);
  } finally {
    chat.sending = false;
  }
}

/** [元に戻す] 1 件。ok:false の reason はそのまま出す */
export async function undoChange(turn: number, index: number) {
  const k = `${turn}:${index}`;
  if (chat.undo[k]?.busy || chat.undo[k]?.ok) return;
  chat.undo[k] = { busy: true };
  try {
    const r = await call<WikiChatUndoResult>("wiki_chat_undo", { turn, index });
    chat.undo[k] = r.ok ? { ok: true } : { ok: false, reason: r.reason };
  } catch (e) {
    chat.undo[k] = { ok: false, reason: String(e) };
  }
}

/** Settings の [チャット] の説明に答える（Rust の wiki-chat-consent）。有効にできなければ toast */
export async function answerConsent(ok: boolean) {
  chat.consent = false;
  if (!ok) return;
  try {
    chat.status = await call<WikiChatStatus>("wiki_chat_set_enabled", { enabled: true });
    chat.statusError = null;
    if (chat.status.enabled) void loadDockPref();
  } catch (e) {
    chat.statusError = String(e);
    toast(`✗ ${t("チャットを有効にできません")}: ${String(e).slice(0, 160)}`, 8000);
  }
}

/** system/init の行から動いているモデルを拾う(reducer は system を描かない = ここで) */
function noteModel(line: string) {
  try {
    const o = JSON.parse(line);
    if (o && o.type === "system" && o.subtype === "init" && typeof o.model === "string" && o.model) chat.model = o.model;
  } catch {
    // JSON でない行は reducer が無視する
  }
}

/** モデルの選択(設定 wiki_chat_model・空 = 既定)。子は止まり、次の送信で起こし直す */
export async function setModel(model: string): Promise<boolean> {
  try {
    chat.status = await call<WikiChatStatus>("wiki_chat_set_model", { model });
    chat.statusError = null;
    chat.model = null;
    return true;
  } catch (e) {
    toast(`✗ ${t("モデルを変えられません")}: ${String(e).slice(0, 160)}`);
    void loadChatStatus(); // 設定の値に <select> を戻す（status.model が変わらなくても WikiChat の {#key} が描き直す）
    return false;
  }
}

/** ログイン(Terminal.app に claude auth login を開く)/ ログアウト(claude auth logout) */
export async function authAction(action: "login" | "logout"): Promise<boolean> {
  try {
    chat.status = await call<WikiChatStatus>("wiki_chat_auth", { action });
    chat.statusError = null;
    if (action === "login") toast(t("ターミナルで claude auth login を開きました。終わったら [再確認] を押してください"), 10000);
    else {
      chat.model = null;
      toast(t("Claude Code からログアウトしました"));
    }
    return true;
  } catch (e) {
    toast(`✗ ${action === "login" ? t("ログインを開けません") : t("ログアウトできません")}: ${String(e).slice(0, 160)}`, 8000);
    return false;
  }
}

/** event "wiki-chat" の 1 件 */
export function onChatEvent(ev: WikiChatEvent) {
  if (!ev || typeof ev !== "object") return;
  switch (ev.type) {
    case "line":
      if (!app.wikiOn) return;
      if (ev.session_id && chat.session && ev.session_id !== chat.session) newView(ev.session_id);
      else if (ev.session_id && !chat.session) chat.session = ev.session_id;
      if (typeof ev.line === "string") {
        if (ev.line.includes('"subtype":"init"')) noteModel(ev.line);
        apply({ t: "line", turn: ev.turn, line: ev.line });
      }
      return;
    case "stderr": {
      if (!app.wikiOn) return;
      const lines = String(ev.text ?? "")
        .split("\n")
        .filter((l) => l.trim())
        .map((l) => (l.length > STDERR_LINE_MAX ? `${l.slice(0, STDERR_LINE_MAX)}…` : l));
      if (!lines.length) return;
      const all = [...chat.stderr, ...lines];
      chat.stderr = all.length > STDERR_MAX ? all.slice(all.length - STDERR_MAX) : all;
      return;
    }
    case "state":
      // 子の状態だけ（status は読み直さない = claude --version / auth status を毎ターン起こさない）
      stateStamp++;
      if (chat.status) chat.status = { ...chat.status, state: ev.state as WikiChatState };
      if (ev.session_id && chat.session && ev.session_id !== chat.session) newView(ev.session_id);
      if (ev.state === "idle" || ev.state === "off") apply({ t: "end" });
      return;
    case "enabled":
      // 有効 / 無効の変化（メニュー・command・Wiki の ON/OFF・vault の切替）= 読み直す。無効は読み直しを待たずに柱とボタンを消す
      if (!ev.enabled && chat.status) chat.status = { ...chat.status, enabled: false };
      void loadChatStatus();
      return;
    case "changes":
      if (!app.wikiOn) return;
      if (ev.session_id && chat.session && ev.session_id !== chat.session) return; // 前の会話のもの
      chat.changes = { ...chat.changes, [ev.turn]: Array.isArray(ev.changes) ? ev.changes : [] };
      return;
    case "exit": {
      apply({ t: "end" });
      chat.model = null;
      if (chat.status) chat.status = { ...chat.status, state: "off" };
      // 新しい会話で止めた前の子プロセス・Rust の後始末（stdin を閉じる → 猶予 → SIGTERM）は知らせない
      const expected = (expectExitAt > 0 && Date.now() - expectExitAt < EXPECT_EXIT_MS) || ev.signal === "SIGTERM";
      const abnormal = !expected && ((ev.code != null && ev.code !== 0) || !!ev.signal);
      expectExitAt = 0;
      if (abnormal && app.wikiOn) apply({ t: "note", tone: "error", code: "exit", detail: ev.signal ? `signal ${ev.signal}` : `code ${ev.code ?? "?"}` });
      void loadChatStatus();
      return;
    }
  }
}

/** event を受ける（App の onMount から。戻り値 = 解除。listen の完了前に解除しても後から外す）。
    選択を拾う / 元が変わったら選択を捨てるのは viewing（initViewing）。ここはその読み手として chatShown を登録する */
export function initChat(): () => void {
  let disposed = false;
  setViewingWanted(chatShown); // チャットが出ている間だけ viewing が選択を拾う
  const offs: (() => void)[] = [];
  const on = <T>(name: string, f: (p: T) => void) =>
    listen<T>(name, (e) => f(e.payload)).then((off) => (disposed ? off() : offs.push(off)));
  void on<WikiChatEvent>("wiki-chat", onChatEvent);
  // 有効 / 無効・vault の切替（Rust の wiki-state）= チャットの状態も読み直す
  void on("wiki-state", () => void loadChatStatus());
  // Settings の [チャット] を ON にする前に説明を出す（Wiki が有効なときだけ）
  void on("wiki-chat-consent", () => {
    if (app.wikiOn) chat.consent = true;
  });
  const stopRoot = $effect.root(() => {
    // Wiki が有効になった（起動時の設定の読込・Settings で ON）= チャットの状態を読む。柱の mount を待たない
    // （タブ列のボタンは chat.status で出る = 起動直後から出す）
    $effect(() => {
      if (app.wikiOn) untrack(() => void loadChatStatus());
    });
    // ワークスペース: 前面の root が変わっても会話は消さない（chatHidden が隠す・戻れば見える）
    // 文脈の元（ページ / 文献）が変わった = 外した文脈を戻す（別の元の選択を捨てるのは viewing）
    $effect(() => {
      const k = viewingKey(viewingSource());
      untrack(() => pageChanged(k));
    });
  });
  return () => {
    disposed = true;
    stopRoot();
    offs.forEach((f) => f());
  };
}

/** 文脈の元（viewingKey）が変わったら外した文脈を戻す（initChat の effect が呼ぶ。別の元の選択を捨てるのは viewing の viewingChanged） */
export function pageChanged(key: string | null) {
  if (chat.ctxOff && chat.ctxOff !== key) chat.ctxOff = null;
}
