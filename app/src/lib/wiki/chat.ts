// OpenNote のチャットの純粋な部分: Claude Code の stream-json の 1 行を
// 画面の会話（Conv）へ畳む reducer と、表示の補助。Svelte にも Tauri にも依存しない（node のテスト = chat.test.mjs）。
// 解釈は寛容に: JSON でない行・未知の type・形の合わない値は無視して同じ Conv を返す（落ちない・捏造しない）。
// 想定した行の形（Claude Code の `-p --output-format=stream-json --include-partial-messages --verbose`）:
//   {"type":"system","subtype":"init",…}                                    → 出さない（system は全部）
//   {"type":"stream_event","event":{"type":"message_start","message":{"id"}}} → 今のメッセージの id
//   {"type":"stream_event","event":{"type":"content_block_start","index":i,"content_block":{"type":"text"|"tool_use",…}}}
//   {"type":"stream_event","event":{"type":"content_block_delta","index":i,"delta":{"type":"text_delta","text"}|{"type":"input_json_delta","partial_json"}}}
//   {"type":"stream_event","event":{"type":"content_block_stop","index":i}}  → 道具の引数（partial_json を繋いだもの）を読む
//   {"type":"assistant","message":{"id","content":[{"type":"text","text"}|{"type":"tool_use","id","name","input"}]}}
//       → 確定の本文で逐次の本文を置き換える。content が 1 ブロックずつ届く形・それまでの全ブロックが届く形のどちらでも
//   {"type":"user","message":{"content":[{"type":"tool_result","tool_use_id","content":string|[{type:"text",text}],"is_error"}]}}
//   {"type":"result","subtype":"success"|"error_…","is_error","duration_ms","result"} → ターンの終わり（逐次の本文を確定にする）
//   parent_tool_use_id が付いた行（下位の agent の中身）は出さない
// 実物（Claude Code 2.1.197・2026-11-01・chat-fixtures/ = 置換済みの stdout の行）で確かめた形: system/init はターンごとに出る（起動直後
//   ではなく最初の発言の後）・確定の assistant は 1 ブロックずつ届き content_block_stop より前に来る・thinking ブロック（中身は空 + signature）が
//   index を 1 つ取る・system の status / thinking_tokens・rate_limit_event・control_response（interrupt の応答）も流れる・道具のエラーは
//   tool_result の is_error: true（content は文字列のことがある）・中断の result = error_during_execution + terminal_reason "aborted_streaming"
//   + errors ["[ede_diagnostic] …"]・user の行に tool_use_result（道具ごとの生の結果）が付く（使わない）
// 画面の文言（日本語）はここに置かない: note は code で持ち、文は WikiChat が t() で組む

import type { WikiChatChange, WikiChatContext } from "./types";
import { wikiLinksOf } from "./render/md.ts";

export type ChatEntry =
  | { k: "user"; id: string; turn: number | null; text: string; ctx?: WikiChatContext }
  | { k: "text"; id: string; turn: number; msg: string | null; text: string; final: boolean }
  | {
      k: "tool";
      id: string;
      turn: number;
      /** tool_use の id（tool_result の突き合わせ） */
      tool: string | null;
      name: string;
      /** 主な引数 1 行 */
      arg: string;
      result: string | null;
      error: boolean;
      done: boolean;
    }
  | { k: "result"; id: string; turn: number; ok: boolean; subtype: string; ms: number | null; text: string | null }
  | { k: "note"; id: string; turn: number | null; tone: "info" | "error"; code: string; detail: string };

export interface Conv {
  entries: ChatEntry[];
  /** id の連番 */
  seq: number;
  /** 逐次の今のメッセージ（message_start の id） */
  msg: string | null;
  /** `${msg}\u0000${index}` → 逐次で作った entry の id */
  blocks: Record<string, string>;
  /** 道具の entry の id → input_json_delta を繋いだもの */
  json: Record<string, string>;
  /** message.id → 確定の assistant で覆ったブロックの数（1 ブロックずつ届く形の次の index） */
  fin: Record<string, number>;
  /** 上限で刈った古い発言の数（画面が「古い発言を省きました」と出す） */
  dropped: number;
}

export function emptyConv(): Conv {
  return { entries: [], seq: 0, msg: null, blocks: {}, json: {}, fin: {}, dropped: 0 };
}

/** 画面に持つ上限（それより古いものは刈る = 本文は Claude Code が持つ）。規則: ターン（発言とその応答の束）単位で古い方から刈る =
    発言は直近 MAX_TURNS まで・entry の合計が MAX_ENTRIES を超えたらさらに古いターンを刈る。ただし直近 KEEP_TURNS ターンは必ず残す（途中では切らない） */
export const MAX_TURNS = 40;
export const MAX_ENTRIES = 800;
export const KEEP_TURNS = 10;

/** reducer への入力。line = stdout の 1 行・user = 自分の発言（送る前に足す）・turn = 送信の結果のターン番号を発言に付ける・
    note = 画面の知らせ（終了・起動の失敗等）・end = 逐次の本文を確定にする（exit・停止） */
export type ChatInput =
  | { t: "line"; turn: number; line: string }
  | { t: "user"; text: string; ctx?: WikiChatContext }
  | { t: "turn"; id: string; turn: number | null }
  | { t: "drop"; id: string }
  | { t: "prune" }
  | { t: "note"; tone: "info" | "error"; code: string; detail?: string; turn?: number | null }
  | { t: "end" };

/** 道具の結果を画面に持つ上限（それより長い結果は切る = 畳んだ中身で全文は要らない） */
export const RESULT_MAX = 4000;
/** 引数の 1 行の上限 */
const ARG_MAX = 140;

const isObj = (v: unknown): v is Record<string, any> => !!v && typeof v === "object" && !Array.isArray(v);
const str = (v: unknown): string | null => (typeof v === "string" ? v : null);

function next(c: Conv): string {
  c.seq += 1;
  return `c${c.seq}`;
}

/** 浅い写し（entries・blocks・json は新しい入れ物・中の entry は変えたものだけ作り直す） */
function copy(c: Conv): Conv {
  return { entries: [...c.entries], seq: c.seq, msg: c.msg, blocks: { ...c.blocks }, json: { ...c.json }, fin: { ...c.fin }, dropped: c.dropped };
}

/** 上限を超えた古いターンを刈る（ターンの境 = 発言の位置で切る・直近 KEEP_TURNS は残す）・消えた entry を指す blocks / json を捨てる */
function prune(c: Conv): boolean {
  const users = c.entries.reduce<number[]>((a, e, i) => (e.k === "user" ? (a.push(i), a) : a), []);
  if (!users.length) return false;
  // 刈ってよい境 = 直近 KEEP_TURNS の発言より前の発言の位置
  const limit = Math.max(0, users.length - KEEP_TURNS);
  let k = Math.max(0, users.length - MAX_TURNS); // 何番目の発言から残すか
  while (k < limit && c.entries.length - users[k] > MAX_ENTRIES) k++;
  if (k === 0) return false;
  const cut = users[k];
  c.entries = c.entries.slice(cut);
  c.dropped += k;
  const alive = new Set(c.entries.map((e) => e.id));
  for (const key of Object.keys(c.blocks)) if (!alive.has(c.blocks[key])) delete c.blocks[key];
  for (const key of Object.keys(c.json)) if (!alive.has(key)) delete c.json[key];
  return true;
}

function replace(c: Conv, id: string, f: (e: ChatEntry) => ChatEntry): boolean {
  const i = c.entries.findIndex((e) => e.id === id);
  if (i < 0) return false;
  c.entries[i] = f(c.entries[i]);
  return true;
}

/** 1 行を畳む。変化が無ければ同じ Conv を返す（画面は描き直さない） */
export function reduceChat(conv: Conv, input: ChatInput): Conv {
  switch (input.t) {
    case "user": {
      const c = copy(conv);
      const e: ChatEntry = { k: "user", id: next(c), turn: null, text: input.text };
      if (input.ctx && (input.ctx.path || input.ctx.item || input.ctx.selection)) e.ctx = input.ctx;
      c.entries.push(e);
      c.msg = null;
      return c;
    }
    case "turn": {
      const c = copy(conv);
      return replace(c, input.id, (e) => (e.k === "user" ? { ...e, turn: input.turn } : e)) ? c : conv;
    }
    case "prune": {
      const c = copy(conv);
      return prune(c) ? c : conv;
    }
    case "drop": {
      if (!conv.entries.some((e) => e.id === input.id)) return conv;
      const c = copy(conv);
      c.entries = c.entries.filter((e) => e.id !== input.id);
      return c;
    }
    case "note": {
      const c = copy(conv);
      c.entries.push({ k: "note", id: next(c), turn: input.turn ?? null, tone: input.tone, code: input.code, detail: input.detail ?? "" });
      return c;
    }
    case "end":
      return finalize(conv, null);
    case "line":
      return reduceLine(conv, input.turn, input.line);
  }
}

/** 逐次の本文・結果待ちの道具を確定にする（turn = null なら全部） */
function finalize(conv: Conv, turn: number | null): Conv {
  const open = conv.entries.some((e) => (turn == null || e.turn === turn) && ((e.k === "text" && !e.final) || (e.k === "tool" && !e.done)));
  const kept = Object.keys(conv.blocks).length || Object.keys(conv.json).length || Object.keys(conv.fin).length;
  if (!open && !kept) return conv;
  const c = copy(conv);
  // ターンの終わり = 逐次の突き合わせ表は要らない（content_block_stop が来なかった道具の json もここで捨てる）
  c.blocks = {};
  c.json = {};
  c.fin = {};
  c.entries = c.entries.map((e) => {
    if (turn != null && e.turn !== turn) return e;
    if (e.k === "text" && !e.final) return { ...e, final: true };
    if (e.k === "tool" && !e.done) return { ...e, done: true };
    return e;
  });
  c.msg = null;
  return c;
}

function parse(line: string): Record<string, any> | null {
  const s = line.trim();
  if (!s.startsWith("{")) return null;
  try {
    const v = JSON.parse(s);
    return isObj(v) ? v : null;
  } catch {
    return null;
  }
}

function reduceLine(conv: Conv, turn: number, line: string): Conv {
  const o = parse(line);
  if (!o) return conv;
  // 下位の agent（Task 等）の中身は出さない（チャットの道具は Read / Glob / Grep と openref だけの想定）
  if (o.parent_tool_use_id != null) return conv;
  switch (o.type) {
    case "stream_event":
      return isObj(o.event) ? streamEvent(conv, turn, o.event) : conv;
    case "assistant":
      return isObj(o.message) ? assistant(conv, turn, o.message) : conv;
    case "user":
      return isObj(o.message) ? toolResults(conv, o.message) : conv;
    case "result":
      return result(conv, turn, o);
    default:
      return conv; // system（init・compact 等）・control_response・未知の type
  }
}

function streamEvent(conv: Conv, turn: number, ev: Record<string, any>): Conv {
  switch (ev.type) {
    case "message_start": {
      const id = isObj(ev.message) ? str(ev.message.id) : null;
      const c = copy(conv);
      c.msg = id ?? `m${c.seq + 1}`;
      if (!id) c.seq += 1;
      return c;
    }
    case "content_block_start": {
      const b = ev.content_block;
      if (!isObj(b) || typeof ev.index !== "number") return conv;
      const key = `${conv.msg ?? ""}\u0000${ev.index}`;
      // 確定の assistant が逐次より先に届いていた（同じ message の同じ index）= 作らない
      if (conv.blocks[key]) return conv;
      const c = copy(conv);
      if (b.type === "text") {
        const id = next(c);
        c.entries.push({ k: "text", id, turn, msg: c.msg, text: str(b.text) ?? "", final: false });
        c.blocks[key] = id;
        return c;
      }
      if (b.type === "tool_use") {
        const tool = str(b.id);
        // 確定の assistant が先に来ていた（同じ tool の id）なら作らない
        const had = tool ? c.entries.find((e) => e.k === "tool" && e.tool === tool) : undefined;
        if (had) {
          c.blocks[key] = had.id;
          return c;
        }
        const id = next(c);
        const name = str(b.name) ?? "?";
        const input = isObj(b.input) && Object.keys(b.input).length ? b.input : null;
        c.entries.push({ k: "tool", id, turn, tool, name, arg: input ? summarize(name, input) : "", result: null, error: false, done: false });
        c.blocks[key] = id;
        return c;
      }
      return conv; // thinking 等
    }
    case "content_block_delta": {
      const d = ev.delta;
      if (!isObj(d) || typeof ev.index !== "number") return conv;
      const id = conv.blocks[`${conv.msg ?? ""}\u0000${ev.index}`];
      if (!id) return conv;
      if (d.type === "text_delta" && typeof d.text === "string" && d.text) {
        const c = copy(conv);
        return replace(c, id, (e) => (e.k === "text" && !e.final ? { ...e, text: e.text + d.text } : e)) ? c : conv;
      }
      if (d.type === "input_json_delta" && typeof d.partial_json === "string") {
        const c = copy(conv);
        c.json[id] = (c.json[id] ?? "") + d.partial_json;
        return c;
      }
      return conv;
    }
    case "content_block_stop": {
      if (typeof ev.index !== "number") return conv;
      const id = conv.blocks[`${conv.msg ?? ""}\u0000${ev.index}`];
      const js = id ? conv.json[id] : undefined;
      if (!id || js == null) return conv;
      const c = copy(conv);
      delete c.json[id];
      let input: unknown = null;
      try {
        input = JSON.parse(js);
      } catch {
        return c;
      }
      if (!isObj(input)) return c;
      replace(c, id, (e) => (e.k === "tool" && !e.arg ? { ...e, arg: summarize(e.name, input as Record<string, any>) } : e));
      return c;
    }
    default:
      return conv; // message_delta・message_stop・ping 等
  }
}

/** 確定の assistant メッセージ。content の各ブロックを message の中の index で逐次の entry（blocks）と突き合わせて置き換え、無ければ足す。
    index の決め方: content が 2 ブロック以上・その message で初めて = 全ブロックが届く形（content[i] が index i）/
    1 ブロックでその message の 2 回目以降 = 1 ブロックずつ届く形（index = それまでに覆った数）。message.id が無ければ突き合わせずに足す */
function assistant(conv: Conv, turn: number, m: Record<string, any>): Conv {
  const content = Array.isArray(m.content) ? m.content.filter(isObj) : [];
  if (!content.length) return conv;
  const msg = str(m.id);
  const c = copy(conv);
  const covered = msg ? (c.fin[msg] ?? 0) : 0;
  const base = content.length === 1 && covered > 0 ? covered : 0;
  content.forEach((b, i) => {
    const index = base + i;
    const key = msg ? `${msg}\u0000${index}` : null;
    const had = key ? c.blocks[key] : undefined;
    if (b.type === "text" && typeof b.text === "string") {
      if (had && c.entries.some((e) => e.id === had && e.k === "text")) {
        replace(c, had, (e) => ({ ...e, text: b.text, final: true }) as ChatEntry);
        return;
      }
      // index が合わない（逐次にだけ出るブロックがある等）= 同じ message のまだ確定していない逐次の本文を前から埋める
      // message.id が無い確定 = まだ確定していない逐次の本文なら message を問わず前から
      const open = c.entries.find((e) => e.k === "text" && !e.final && (msg ? e.msg === msg : true));
      if (open) {
        replace(c, open.id, (e) => ({ ...e, text: b.text, final: true }) as ChatEntry);
        if (key) c.blocks[key] = open.id;
        return;
      }
      if (!b.text) return;
      const id = next(c);
      c.entries.push({ k: "text", id, turn, msg, text: b.text, final: true });
      if (key) c.blocks[key] = id;
      return;
    }
    if (b.type === "tool_use") {
      const tool = str(b.id);
      const name = str(b.name) ?? "?";
      const arg = isObj(b.input) ? summarize(name, b.input) : "";
      const prev = (had && c.entries.find((e) => e.id === had && e.k === "tool")) || (tool ? c.entries.find((e) => e.k === "tool" && e.tool === tool) : undefined);
      if (prev) {
        replace(c, prev.id, (e) => (e.k === "tool" ? { ...e, name, tool: e.tool ?? tool, arg: arg || e.arg } : e));
        delete c.json[prev.id];
        if (key) c.blocks[key] = prev.id;
      } else {
        const id = next(c);
        c.entries.push({ k: "tool", id, turn, tool, name, arg, result: null, error: false, done: false });
        if (key) c.blocks[key] = id;
      }
    }
    // thinking 等は出さない（index は数える）
  });
  if (msg) c.fin[msg] = Math.max(covered, base + content.length);
  return c;
}

function toolResults(conv: Conv, m: Record<string, any>): Conv {
  const content = Array.isArray(m.content) ? m.content.filter(isObj) : [];
  const rs = content.filter((b) => b.type === "tool_result" && typeof b.tool_use_id === "string");
  if (!rs.length) return conv; // 自分の発言の写し等は出さない（画面が自分で足している）
  const c = copy(conv);
  let hit = false;
  for (const r of rs) {
    const e = c.entries.find((x) => x.k === "tool" && x.tool === r.tool_use_id);
    if (!e) continue;
    hit = true;
    replace(c, e.id, (x) => (x.k === "tool" ? { ...x, result: resultText(r.content), error: r.is_error === true, done: true } : x));
  }
  return hit ? c : conv;
}

/** tool_result の content（文字列 / ブロックの配列）→ 文字列（画像は印だけ） */
export function resultText(v: unknown): string {
  let s = "";
  if (typeof v === "string") s = v;
  else if (Array.isArray(v)) {
    s = v
      .map((b) => (isObj(b) && b.type === "text" && typeof b.text === "string" ? b.text : isObj(b) && b.type === "image" ? "[image]" : ""))
      .filter(Boolean)
      .join("\n");
  }
  return s.length > RESULT_MAX ? `${s.slice(0, RESULT_MAX)}…` : s;
}

function result(conv: Conv, turn: number, o: Record<string, any>): Conv {
  const f = finalize(conv, turn);
  const c = f === conv ? copy(conv) : f;
  const subtype = str(o.subtype) ?? "";
  const ok = o.is_error !== true && (subtype === "" || subtype === "success");
  const ms = typeof o.duration_ms === "number" ? o.duration_ms : null;
  const text = ok ? null : (str(o.result) ?? (Array.isArray(o.errors) ? o.errors.filter((x: unknown) => typeof x === "string").join("\n") : null));
  c.entries.push({ k: "result", id: next(c), turn, ok, subtype, ms, text });
  c.msg = null;
  return c;
}

/** 道具の名前の表示（mcp__openref__call → call） */
export function toolName(name: string): string {
  return name.replace(/^mcp__openref__/, "");
}

function clip(s: string, n = ARG_MAX): string {
  const one = s.replace(/\s+/g, " ").trim();
  return one.length > n ? `${one.slice(0, n - 1)}…` : one;
}

/** 道具の主な引数 1 行。openref の call = コマンド名 + path / key / query・Read = file_path・Grep / Glob = pattern（+ path）・sql = 文 */
export function summarize(name: string, input: Record<string, any>): string {
  const n = toolName(name);
  if (n === "call") {
    const cmd = str(input.name) ?? "";
    const a = isObj(input.args) ? input.args : {};
    const target =
      a.from && a.to ? `${a.from} → ${a.to}` : (str(a.path) ?? str(a.key) ?? str(a.query) ?? str(a.identifier) ?? str(a.dir) ?? "");
    return clip([cmd, target].filter(Boolean).join(" "));
  }
  if (n === "sql") return clip(str(input.sql) ?? "");
  if (n === "Grep" || n === "Glob") return clip(str(input.path) ? `${str(input.pattern) ?? ""} (${input.path})` : (str(input.pattern) ?? ""));
  for (const k of ["file_path", "path", "pattern", "query", "command", "url"]) {
    const v = str(input[k]);
    if (v) return clip(v);
  }
  const first = Object.values(input).find((v) => typeof v === "string");
  return typeof first === "string" ? clip(first) : "";
}

/** ターン番号の並び（発言・応答の出現順・null は前に付いた turn に寄せる）。画面がターンごとに束ねる */
export function groupByTurn(entries: ChatEntry[]): { turn: number | null; entries: ChatEntry[] }[] {
  const out: { turn: number | null; entries: ChatEntry[] }[] = [];
  for (const e of entries) {
    const last = out[out.length - 1];
    // 新しい発言は新しい束（turn が付く前でも）
    if (e.k === "user" || !last) {
      out.push({ turn: e.turn, entries: [e] });
      continue;
    }
    if (last.turn == null && e.turn != null && last.entries[0].k === "user") last.turn = e.turn;
    last.entries.push(e);
  }
  return out;
}

/** 変更 1 件の表示の材料（文言は画面が kind で選ぶ）。open = [開く] の行き先 */
export function changeView(c: WikiChatChange): { kind: WikiChatChange["kind"]; label: string; open: { page: string } | { item: string } | null } {
  switch (c.kind) {
    case "write":
    case "create":
      return { kind: c.kind, label: c.path, open: { page: c.path } };
    case "move":
      return { kind: "move", label: `${c.from} → ${c.to}`, open: { page: c.to } };
    case "item":
      return { kind: "item", label: c.title ? `${c.title} (${c.key})` : c.key, open: { item: c.key } };
    default:
      return { kind: (c as { kind: WikiChatChange["kind"] }).kind, label: "", open: null };
  }
}

/** ページ一覧の名前索引（パスの集合・小文字のファイル名（拡張子なし）→ パスの一覧） */
export interface PageIndex {
  byPath: Set<string>;
  byName: Map<string, string[]>;
}

const pageIndexes = new WeakMap<{ path: string }[], PageIndex>();

/** ページ一覧（wk.pages）の名前索引。配列ごとに 1 回だけ組む（WeakMap = 木が置き換わるまで同じ物。応答のブロックごとに組み直さない） */
export function pageIndex(pages: { path: string }[]): PageIndex {
  let ix = pageIndexes.get(pages);
  if (ix) return ix;
  const byPath = new Set(pages.map((p) => p.path));
  const byName = new Map<string, string[]>();
  for (const p of pages) {
    const base = (p.path.split("/").pop() ?? p.path).replace(/\.md$/i, "").toLowerCase();
    const l = byName.get(base);
    if (l) l.push(p.path);
    else byName.set(base, [p.path]);
  }
  ix = { byPath, byName };
  pageIndexes.set(pages, ix);
  return ix;
}

/** 応答の md の `[[名前]]`（描画と同じ走査 = md.ts の wikiLinksOf）を vault のページへ引き当てる（索引の解決の近似 = パスそのもの・拡張子の補い・ファイル名が 1 つに決まるもの）。
    MdView に渡す links（WikiLink）にする。決まらなければ未解決のまま（推測で別のページへ飛ばさない） */
export function chatLinks(text: string, pages: { path: string }[]): { kind: "wiki" | "embed"; target: string; path: string | null; ambiguous: boolean }[] {
  const out: { kind: "wiki" | "embed"; target: string; path: string | null; ambiguous: boolean }[] = [];
  const seen = new Set<string>();
  const { byPath, byName } = pageIndex(pages);
  for (const { kind, target } of wikiLinksOf(text)) {
    const key = `${kind}\u0000${target}`;
    if (seen.has(key)) continue;
    seen.add(key);
    const name = target.split("#")[0];
    let path: string | null = null;
    let ambiguous = false;
    if (byPath.has(name)) path = name;
    else if (byPath.has(`${name}.md`)) path = `${name}.md`;
    else if (!name.includes("/")) {
      const l = byName.get(name.replace(/\.md$/i, "").toLowerCase()) ?? [];
      if (l.length === 1) path = l[0];
      else if (l.length > 1) ambiguous = true;
    }
    out.push({ kind, target, path, ambiguous });
  }
  return out;
}

/** 選択範囲として送る上限（それより長ければ切って印を付ける） */
export const SELECTION_MAX = 20000;

/** 選択範囲を code point 単位で SELECTION_MAX まで切る（サロゲートペアの途中で切らない）。length = 切った後の code point の数 */
export function clipSelection(s: string): { text: string; length: number; cut: boolean } {
  let n = 0;
  let end = 0;
  for (const ch of s) {
    if (n === SELECTION_MAX) return { text: s.slice(0, end), length: n, cut: true };
    n++;
    end += ch.length;
  }
  return { text: s, length: n, cut: false };
}

/** 文字数（code point） */
export function codePoints(s: string): number {
  let n = 0;
  for (const _ of s) n++;
  return n;
}

// ---- 浮かせたパネル（WikiChatFloat）の幾何 ----

export type ChatFloat = { x: number; y: number; w: number; h: number };
export const FLOAT_MIN_W = 320;
export const FLOAT_MIN_H = 260;

/** 浮かせたパネルを窓の中に収める（全体が窓の中 = 入力欄と右下の角が外に出ない。大きさは窓まで・最小 320×260）。
    vw / vh = 窓の大きさ（呼び手が window.innerWidth / innerHeight を渡す = ここは window に触らない） */
export function clampFloat(f: ChatFloat, vw: number, vh: number): ChatFloat {
  const w = Math.max(FLOAT_MIN_W, Math.min(Math.round(f.w), vw));
  const h = Math.max(FLOAT_MIN_H, Math.min(Math.round(f.h), vh));
  const x = Math.max(0, Math.min(Math.round(f.x), vw - w));
  const y = Math.max(0, Math.min(Math.round(f.y), vh - h));
  return { x, y, w, h };
}
