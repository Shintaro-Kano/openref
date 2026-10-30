// Word ダイアログのページ JS（OpenRef Dialog.app が開く 3 ページ）の node テスト。本物のファイルを vm に読む。
//   cite-dialog.js    … Enter・IME の composing・pendingEnter・チップ・{type,keys}・0 件の削除・Esc・検索 500 の表示
//   settings-dialog.js … 4〜72pt の検査・Enter/⌘Enter・絞り込み・一覧 500 の表示・{type:"apply",…}
//   notice.js         … error/confirm/info の結果・Esc・既定フォーカス・コピー
// dialog-common.js も本物を読み、?host=dotm のときは fetch(/word/v2/dialog/<sid>/result) の本文を見る。
// DOM は依存なしの最小の偽物（id 引き・class・イベントの伝播・innerHTML への代入だけ）。
// 実行: node word/test-dialog-pages.cjs
"use strict";
const fs = require("fs");
const path = require("path");
const vm = require("vm");
const assert = require("assert");

const read = (f) => fs.readFileSync(path.join(__dirname, f), "utf8");
const PAGES = {
  cite: { js: "cite-dialog.js", html: "cite-dialog.html" },
  settings: { js: "settings-dialog.js", html: "settings-dialog.html" },
  notice: { js: "notice.js", html: "notice.html" },
};

// ---------- 偽 DOM ----------
function mkEl(tag) {
  const e = {
    tagName: String(tag).toUpperCase(),
    id: "",
    className: "",
    textContent: "",
    value: "",
    checked: false,
    hidden: false,
    disabled: false,
    title: "",
    placeholder: "",
    style: {},
    children: [],
    parentNode: null,
    listeners: {},
    attrs: {},
    _html: "",
    appendChild(c) {
      if (c.parentNode) c.parentNode.removeChild(c);
      c.parentNode = this;
      this.children.push(c);
      return c;
    },
    insertBefore(c, ref) {
      if (c.parentNode) c.parentNode.removeChild(c);
      c.parentNode = this;
      const i = this.children.indexOf(ref);
      this.children.splice(i < 0 ? this.children.length : i, 0, c);
      return c;
    },
    removeChild(c) {
      const i = this.children.indexOf(c);
      if (i >= 0) this.children.splice(i, 1);
      c.parentNode = null;
      return c;
    },
    setAttribute(k, v) { this.attrs[k] = String(v); },
    getAttribute(k) { return k in this.attrs ? this.attrs[k] : null; },
    addEventListener(t, fn) { (this.listeners[t] = this.listeners[t] || []).push(fn); },
    /** 偽のイベントを対象 → 親 → document へ伝播（stopPropagation を尊重） */
    dispatch(t, ev) {
      ev = ev || {};
      // 本物と同じく disabled の要素にはクリック・キー・入力のイベントが届かない
      if (this.disabled && /^(click|dblclick|mousedown|mousemove|keydown|input)$/.test(t)) return ev;
      if (!ev.target) ev.target = this;
      if (!ev.preventDefault) ev.preventDefault = function () { ev.defaultPrevented = true; };
      if (!ev.stopPropagation) ev.stopPropagation = function () { ev.stopped = true; };
      for (let n = this; n && !ev.stopped; n = n.parentNode) (n.listeners[t] || []).forEach((fn) => fn(ev));
      return ev;
    },
    querySelectorAll(sel) {
      const cls = sel.replace(/^\./, "");
      const out = [];
      (function walk(n) {
        n.children.forEach((c) => {
          if (c.className.split(/\s+/).includes(cls)) out.push(c);
          walk(c);
        });
      })(this);
      return out;
    },
    focus() { this.ownerDocument.activeElement = this; },
    select() {},
    scrollIntoView() {},
    get innerHTML() { return this._html; },
    set innerHTML(v) {
      this._html = v;
      if (v === "") {
        this.children.forEach((c) => (c.parentNode = null));
        this.children.length = 0;
      }
    },
  };
  e.classList = {
    _a() { return e.className.split(/\s+/).filter(Boolean); },
    add(c) { if (!this._a().includes(c)) e.className = this._a().concat(c).join(" "); },
    remove(c) { e.className = this._a().filter((x) => x !== c).join(" "); },
    contains(c) { return this._a().includes(c); },
    toggle(c, force) {
      const on = force === undefined ? !this.contains(c) : !!force;
      if (on) this.add(c);
      else this.remove(c);
    },
  };
  return e;
}

/**
 * 実 HTML(<body> 以降)を偽 DOM の木にする。id・class・hidden・disabled・type・直下のテキスト・入れ子・<script src> の順を
 * そのまま持ってくる(手組みの木・手書きの id 一覧を持たない)。戻り値 = { scripts: [src…], ids: Set }
 */
function buildFromHtml(doc, htmlText) {
  const body = doc.body;
  const VOID = new Set(["input", "img", "meta", "link", "br", "hr"]);
  const out = { scripts: [], ids: new Set() };
  const tail = htmlText.slice(htmlText.search(/<body[\s>]/));
  const stack = [body];
  const re = /<!--[\s\S]*?-->|<(\/?)([a-zA-Z][\w-]*)([^>]*)>|([^<]+)/g;
  let m;
  while ((m = re.exec(tail))) {
    if (m[0].startsWith("<!--")) continue;
    if (m[4] !== undefined) {
      const t = m[4].replace(/\s+/g, " ").trim();
      const top = stack[stack.length - 1];
      if (t && top.tagName !== "SCRIPT") top.textContent += t;
      continue;
    }
    const closing = m[1] === "/";
    const tag = m[2].toLowerCase();
    if (closing) {
      if (!VOID.has(tag) && stack.length > 1) stack.pop();
      continue;
    }
    const attrs = {};
    String(m[3]).replace(/([\w:-]+)(?:="([^"]*)")?/g, (_, k, v) => {
      attrs[k] = v === undefined ? "" : v;
      return "";
    });
    if (tag === "body") {
      body.className = attrs.class || "";
      continue;
    }
    const e = doc.createElement(tag);
    if (attrs.id) {
      e.id = attrs.id;
      out.ids.add(attrs.id);
    }
    if (attrs.class) e.className = attrs.class;
    if ("hidden" in attrs) e.hidden = true;
    if ("disabled" in attrs) e.disabled = true;
    if (attrs.type) e.type = attrs.type;
    if (attrs.placeholder) e.placeholder = attrs.placeholder;
    if (tag === "input" && attrs.type === "number") {
      // 本物の <input type=number>: value は有効な数値の文字列だけ・不正な入力は ""(数値の代入も文字列になる)
      let v = "";
      Object.defineProperty(e, "value", {
        get: () => v,
        set: (x) => {
          x = String(x);
          v = /^-?(\d+|\d*\.\d+)([eE][-+]?\d+)?$/.test(x) ? x : "";
        },
      });
    }
    if (tag === "script" && attrs.src) out.scripts.push(attrs.src);
    stack[stack.length - 1].appendChild(e);
    if (!VOID.has(tag)) stack.push(e);
  }
  return out;
}

// json() は呼ぶたびに新しい物(本物の Response と同じ。ページ JS が配列を書き換えても次の試験に漏れない)
const resp = (status, body) => Promise.resolve({ ok: status >= 200 && status < 300, status, json: () => Promise.resolve(JSON.parse(JSON.stringify(body))) });
function deferred() {
  let resolve;
  const promise = new Promise((r) => (resolve = r));
  return { promise, resolve };
}
const tick = () => new Promise((r) => setImmediate(r));

/**
 * ページ 1 枚を開く。page = cite | settings | notice。
 * opts.d = 初期データ・opts.search = クエリ全体（d より優先）・opts.routes = [[urlの前方一致, (url, opts) → Promise]]
 * 結果の POST（/word/v2/dialog/ab12/result）は opts.result（既定 200）に応じて返し、本文を env.sent に積む。
 */
async function open(page, opts) {
  opts = opts || {};
  const doc = mkEl("#document");
  doc.ownerDocument = doc;
  const body = mkEl("body");
  doc.appendChild(body);
  doc.body = body;
  const els = new Proxy({}, { get: (_, id) => doc.getElementById(String(id)) || undefined });
  doc.createElement = (t) => {
    const e = mkEl(t);
    e.ownerDocument = doc;
    return e;
  };
  doc.getElementById = function find(id, n) {
    n = n || body;
    if (n.id === id) return n;
    for (const c of n.children) {
      const r = find(id, c);
      if (r) return r;
    }
    return null;
  };
  doc.querySelector = () => null;
  doc.activeElement = body;
  body.ownerDocument = doc;
  body.classList.add("rd-body");
  const built = buildFromHtml(doc, read(PAGES[page].html));
  const timers = [];
  let tid = 0;
  const logs = { log: [], warn: [], error: [] };
  const sent = [];
  const calls = [];
  const copied = [];
  const win = {
    location: { search: opts.search !== undefined ? opts.search : "?host=dotm&sid=ab12" + (opts.tab ? "&tab=" + opts.tab : "") + (opts.d !== undefined ? "&d=" + encodeURIComponent(JSON.stringify(opts.d)) : ""), origin: "https://localhost:23120" },
    document: doc,
    URLSearchParams,
    encodeURIComponent,
    console: { log: (...a) => logs.log.push(a.join(" ")), warn: (...a) => logs.warn.push(a.join(" ")), error: (...a) => logs.error.push(a.join(" ")) },
    setTimeout: (fn, ms) => {
      timers.push({ id: ++tid, fn, ms });
      return tid;
    },
    clearTimeout: (id) => {
      const i = timers.findIndex((t) => t.id === id);
      if (i >= 0) timers.splice(i, 1);
    },
    navigator: opts.clipboard === undefined ? {} : { clipboard: { writeText: (t) => { copied.push(t); return opts.clipboard ? Promise.resolve() : Promise.reject(new Error("denied")); } } },
    getSelection: () => ({ selectAllChildren: (n) => copied.push("selected:" + n.id) }),
    fetch: (url, o) => {
      calls.push({ url, opts: o });
      if (/^\/word\/v2\/dialog\/[^/]+\/result$/.test(url)) {
        sent.push(JSON.parse(o.body));
        return resp(opts.result || 200, {});
      }
      for (const [prefix, fn] of opts.routes || []) if (url.startsWith(prefix)) return fn(url, o);
      return resp(404, {});
    },
  };
  doc.execCommand = () => false;
  win.window = win;
  const ctx = vm.createContext(win);
  // HTML の <script src> の順にそのまま読む(タグが欠けると動かない)
  for (const src of built.scripts) vm.runInContext(read(path.basename(src)), ctx);
  await tick();
  return {
    win, doc, els, built, sent, calls, logs, copied, timers,
    /** 遅延タイマー(ms 一致のもの)を走らせる */
    fire(ms) {
      timers.filter((t) => t.ms === ms).forEach((t) => {
        timers.splice(timers.indexOf(t), 1);
        t.fn();
      });
    },
    key(target, props) {
      return target.dispatch("keydown", Object.assign({ key: "", isComposing: false, keyCode: 0, metaKey: false, ctrlKey: false }, props));
    },
  };
}

// ---------- 共通の小道具 ----------
const ITEM_A = { key: "A2020", title: "Alpha <b>study</b>", first_author: "Kano", journal: "Radiology", year: 2020 };
const ITEM_B = { key: "B2021", title: "Beta trial", first_author: "Sato", journal: "Neuroradiology", year: 2021 };
const searchRoute = (items) => ["/api/search?q=", () => resp(200, { items })];

const chips = (env) => env.els["rd-search"].querySelectorAll(".rd-chip");
const chipLabels = (env) => chips(env).map((c) => c.children[0].textContent);
const rows = (env) => env.els["rd-results"].children;
function typeQ(env, text) {
  env.els["rd-q"].value = text;
  env.els["rd-q"].dispatch("input", {});
}

let failures = 0;
async function check(name, fn) {
  try {
    await fn();
    console.log("OK   " + name);
  } catch (e) {
    failures++;
    console.log("FAIL " + name + ": " + (e && e.stack ? e.stack.split("\n").slice(0, 3).join(" | ") : e));
  }
}

(async () => {
  // ================= 実 HTML との整合 =================
  const parsed = (page) => {
    const doc = mkEl("#document");
    doc.ownerDocument = doc;
    doc.body = mkEl("body");
    doc.createElement = (t) => mkEl(t);
    return Object.assign(buildFromHtml(doc, read(PAGES[page].html)), { doc });
  };
  await check("HTML: <script src> で dialog-common.js と自分のページ JS をこの順に読み込む", async () => {
    for (const page of Object.keys(PAGES)) {
      assert.deepStrictEqual(parsed(page).scripts, ["/word/dialog-common.js", "/word/" + PAGES[page].js], page);
    }
  });
  await check("HTML: ページ JS が $(\"…\") / getElementById で引く id は全部 HTML にある", async () => {
    for (const page of Object.keys(PAGES)) {
      const ids = parsed(page).ids;
      const used = [...read(PAGES[page].js).matchAll(/(?:\$|getElementById)\("([\w-]+)"\)/g)].map((m) => m[1]);
      assert.ok(used.length >= 5, page + ": 引いている id を拾えていない");
      used.forEach((id) => assert.ok(ids.has(id), page + ": " + id + " が " + PAGES[page].html + " に無い"));
    }
  });
  await check("HTML: cite は rd-q・rd-clear が rd-search の中にある(チップは rd-q の前に差し込まれる)", async () => {
    const env = await open("cite", { d: { mode: "add", items: [] } });
    assert.strictEqual(env.els["rd-q"].parentNode, env.els["rd-search"]);
    assert.strictEqual(env.els["rd-clear"].parentNode, env.els["rd-search"]);
    assert.strictEqual(env.els["rd-q"].tagName, "INPUT");
    assert.strictEqual(env.els["rd-ok"].parentNode, env.els["rd-cancel"].parentNode);
  });
  await check("HTML: 開いた直後の hidden / disabled は HTML のとおり(JS が勝手に出さない・隠さない)", async () => {
    const c = await open("cite", { d: { mode: "add", items: [] } });
    assert.strictEqual(c.els["rd-clear"].hidden, true);
    assert.strictEqual(c.els["rd-note"].hidden, true);
    assert.strictEqual(c.els["rd-results"].hidden, false);
    const s = await open("settings", { d: { style: "vancouver" }, routes: [["/api/styles", () => resp(200, { styles: [] })]] });
    assert.strictEqual(s.els["rd-pane-doc"].hidden, true);
    assert.strictEqual(s.els["rd-pane-style"].hidden, false);
    assert.strictEqual(s.els["rd-err"].hidden, true);
    const nErr = await open("notice", { d: { kind: "error", message: "m" } });
    assert.strictEqual(nErr.els["rd-warn"].hidden, false);
    assert.strictEqual(nErr.els["rd-cancel"].hidden, true);
    assert.strictEqual(nErr.els["rd-copy"].hidden, true);
    assert.strictEqual(nErr.els["rd-detail"].hidden, true, "detail が無ければ出さない");
    const nConf = await open("notice", { d: { kind: "confirm", message: "m" } });
    assert.strictEqual(nConf.els["rd-warn"].hidden, true);
    assert.strictEqual(nConf.els["rd-cancel"].hidden, false);
    assert.strictEqual(nConf.els["rd-copy"].hidden, true);
  });

  // ================= cite-dialog =================
  await check("cite: 初期化 — 追加モードは「挿入」が無効・フッタにスタイル名・ヒント表示", async () => {
    const env = await open("cite", { d: { mode: "add", items: [], style: { id: "vancouver", title: "Vancouver" } } });
    assert.strictEqual(env.els["rd-ok"].textContent, "挿入");
    assert.strictEqual(env.els["rd-ok"].disabled, true);
    assert.strictEqual(env.els["rd-style"].textContent, "スタイル: Vancouver");
    assert.ok(/Enter で追加/.test(rows(env)[0].textContent));
    assert.strictEqual(env.doc.activeElement, env.els["rd-q"], "検索欄にフォーカス");
    assert.strictEqual(env.calls.length, 0, "チップが無ければ /api/items は引かない");
  });

  await check("cite: 検索 → Enter でチップ → 空欄 Enter で {type:insert,keys}", async () => {
    const env = await open("cite", { d: { mode: "add", items: [] }, routes: [searchRoute([ITEM_A, ITEM_B])] });
    typeQ(env, " kano ");
    assert.strictEqual(env.timers.some((t) => t.ms === 250), true, "250ms のデバウンス");
    assert.strictEqual(env.calls.length, 0);
    env.fire(250);
    await tick();
    assert.strictEqual(env.calls[0].url, "/api/search?q=kano", "trim して encode");
    assert.strictEqual(rows(env).length, 2);
    assert.ok(rows(env)[0].classList.contains("rd-hi"), "先頭が強調行");
    assert.ok(rows(env)[0].innerHTML.includes("Alpha &lt;b&gt;study&lt;/b&gt;"), "タイトルは escape");
    assert.ok(rows(env)[0].innerHTML.includes("Kano · Radiology · 2020"));
    const e1 = env.key(env.els["rd-q"], { key: "Enter" });
    assert.ok(e1.defaultPrevented);
    assert.deepStrictEqual(chipLabels(env), ["Kano 2020"]);
    assert.strictEqual(env.els["rd-q"].value, "", "追加したら入力を空に");
    assert.strictEqual(env.els["rd-ok"].disabled, false);
    assert.strictEqual(env.sent.length, 0, "Enter で追加しただけでは送らない");
    // 空欄で Enter → 確定
    env.key(env.els["rd-q"], { key: "Enter" });
    await tick();
    assert.deepStrictEqual(env.sent, [{ type: "insert", keys: ["A2020"] }]);
    assert.strictEqual(env.doc.title, "openref:done");
    assert.strictEqual(env.els["rd-q"].disabled, true, "送信後は入力欄を閉じる");
    assert.strictEqual(env.els["rd-ok"].disabled, true);
    // 二重送信しない(ボタン・入力欄は disabled で届かない。document 側のキーは S.sent ガードで止まる)
    env.key(env.doc.body, { key: "Enter", metaKey: true });
    env.key(env.doc.body, { key: "Escape" });
    env.els["rd-ok"].dispatch("click", {});
    env.els["rd-cancel"].dispatch("click", {});
    assert.strictEqual(env.els["rd-cancel"].disabled, true, "送信後はキャンセルも無効");
    await tick();
    assert.strictEqual(env.sent.length, 1);
  });

  await check("cite: ↑↓ で強調行を動かし、2 件目をチップに → ⌘Enter で確定（複数 keys の順）", async () => {
    const env = await open("cite", { d: { mode: "add", items: [] }, routes: [searchRoute([ITEM_A, ITEM_B])] });
    typeQ(env, "x");
    env.fire(250);
    await tick();
    env.key(env.els["rd-q"], { key: "ArrowDown" });
    assert.ok(rows(env)[1].classList.contains("rd-hi") && !rows(env)[0].classList.contains("rd-hi"));
    env.key(env.els["rd-q"], { key: "ArrowDown" }); // 末尾で止まる
    assert.ok(rows(env)[1].classList.contains("rd-hi"));
    env.key(env.els["rd-q"], { key: "Enter" });
    typeQ(env, "y");
    env.fire(250);
    await tick();
    env.key(env.els["rd-q"], { key: "Enter" }); // 先頭 A を追加
    assert.deepStrictEqual(chipLabels(env), ["Sato 2021", "Kano 2020"]);
    env.key(env.els["rd-q"], { key: "Enter", ctrlKey: true });
    assert.deepStrictEqual(env.sent, [{ type: "insert", keys: ["B2021", "A2020"] }]);
  });

  await check("cite: 追加モードでチップ 0 件の ⌘Enter / 空 Enter / 挿入ボタンは何も送らない", async () => {
    const env = await open("cite", { d: { mode: "add", items: [] } });
    env.key(env.els["rd-q"], { key: "Enter", metaKey: true });
    env.key(env.els["rd-q"], { key: "Enter" });
    env.els["rd-ok"].dispatch("click", {});
    await tick();
    assert.strictEqual(env.sent.length, 0);
  });

  await check("cite: IME 変換中（isComposing / keyCode 229）の Enter・矢印・Esc は触らない", async () => {
    const env = await open("cite", { d: { mode: "add", items: [{ key: "K1", label: "Initial" }] }, routes: [searchRoute([ITEM_A])] });
    typeQ(env, "かの");
    env.fire(250);
    await tick();
    for (const props of [{ key: "Enter", isComposing: true }, { key: "Enter", keyCode: 229 }, { key: "Escape", isComposing: true }, { key: "ArrowDown", keyCode: 229 }]) {
      const ev = env.key(env.els["rd-q"], props);
      assert.ok(!ev.defaultPrevented, JSON.stringify(props) + " は preventDefault しない");
    }
    assert.deepStrictEqual(chipLabels(env), ["Initial"], "既存チップのまま増えない");
    assert.strictEqual(env.els["rd-q"].value, "かの");
    assert.strictEqual(env.sent.length, 0);
    // 変換が終われば Enter は効く
    env.key(env.els["rd-q"], { key: "Enter" });
    assert.deepStrictEqual(chipLabels(env), ["Initial", "Kano 2020"], "変換が終われば Enter で追加される");
    // document 側（検索欄以外）の Esc も composing は無視
    const ev = env.key(env.els["rd-ok"], { key: "Escape", isComposing: true });
    assert.ok(!ev.defaultPrevented);
    assert.strictEqual(env.sent.length, 0);
  });

  await check("cite: pendingEnter — 検索待ち(デバウンス中)の Enter は結果が来たら先頭を追加", async () => {
    const env = await open("cite", { d: { mode: "add", items: [] }, routes: [searchRoute([ITEM_A, ITEM_B])] });
    typeQ(env, "kano");
    env.key(env.els["rd-q"], { key: "Enter" });
    assert.strictEqual(chips(env).length, 0, "まだ結果が無い");
    env.fire(250);
    await tick();
    assert.deepStrictEqual(chipLabels(env), ["Kano 2020"]);
    assert.strictEqual(env.els["rd-q"].value, "");
    assert.strictEqual(env.sent.length, 0);
  });

  await check("cite: pendingEnter — 通信中の Enter も待つ・入力が変わったら取り消す", async () => {
    const d1 = deferred();
    const env = await open("cite", { d: { mode: "add", items: [] }, routes: [["/api/search?q=", () => d1.promise]] });
    typeQ(env, "kano");
    env.fire(250); // 通信開始（status = loading）
    env.key(env.els["rd-q"], { key: "Enter" });
    assert.strictEqual(chips(env).length, 0);
    d1.resolve({ ok: true, status: 200, json: () => Promise.resolve({ items: [ITEM_B] }) });
    await tick();
    assert.deepStrictEqual(chipLabels(env), ["Sato 2021"]);
    // 打ち直したら pendingEnter は捨てる
    const env2 = await open("cite", { d: { mode: "add", items: [] }, routes: [searchRoute([ITEM_A])] });
    typeQ(env2, "kan");
    env2.key(env2.els["rd-q"], { key: "Enter" });
    typeQ(env2, "kano");
    env2.fire(250);
    await tick();
    assert.strictEqual(chips(env2).length, 0, "Enter 後に入力を変えたら自動追加しない");
    assert.strictEqual(rows(env2).length, 1);
  });

  await check("cite: 古い検索の結果は捨てる（seq）・空にすると結果を消してヒントへ", async () => {
    const d1 = deferred();
    const d2 = deferred();
    let n = 0;
    const env = await open("cite", { d: { mode: "add", items: [] }, routes: [["/api/search?q=", () => (++n === 1 ? d1.promise : d2.promise)]] });
    typeQ(env, "a");
    env.fire(250);
    typeQ(env, "ab");
    env.fire(250);
    const mk = (items) => ({ ok: true, status: 200, json: () => Promise.resolve({ items }) });
    d2.resolve(mk([ITEM_B]));
    await tick();
    d1.resolve(mk([ITEM_A]));
    await tick();
    assert.strictEqual(rows(env).length, 1);
    assert.ok(rows(env)[0].innerHTML.includes("Beta trial"), "後の検索の結果が残る");
    typeQ(env, "");
    assert.ok(/Enter で追加/.test(rows(env)[0].textContent));
    assert.strictEqual(env.els["rd-clear"].hidden, true);
  });

  await check("cite: 検索が HTTP 500 → 「検索できません: HTTP 500」を赤字で出し、Enter は何もしない", async () => {
    const env = await open("cite", { d: { mode: "add", items: [] }, routes: [["/api/search?q=", () => resp(500, {})]] });
    typeQ(env, "kano");
    env.key(env.els["rd-q"], { key: "Enter" }); // pendingEnter
    env.fire(250);
    await tick();
    assert.strictEqual(rows(env).length, 1);
    assert.ok(rows(env)[0].className.includes("rd-err"));
    assert.ok(rows(env)[0].textContent.startsWith("検索できません: HTTP 500"), rows(env)[0].textContent);
    assert.strictEqual(chips(env).length, 0, "エラー時は pendingEnter を捨てる");
    assert.strictEqual(env.sent.length, 0);
  });

  await check("cite: 0 件 → 「該当なし」", async () => {
    const env = await open("cite", { d: { mode: "add", items: [] }, routes: [searchRoute([])] });
    typeQ(env, "zzz");
    env.fire(250);
    await tick();
    assert.strictEqual(rows(env)[0].textContent, "該当なし");
    env.key(env.els["rd-q"], { key: "Enter" });
    assert.strictEqual(chips(env).length, 0);
  });

  await check("cite: Backspace（入力が空）で最後のチップを外す・入力があるときは外さない・× で外す", async () => {
    const env = await open("cite", { d: { mode: "add", items: [{ key: "K1", label: "L1" }, { key: "K2", label: "L2" }] } });
    assert.deepStrictEqual(chipLabels(env), ["L1", "L2"]);
    env.els["rd-q"].value = "ab";
    const ev0 = env.key(env.els["rd-q"], { key: "Backspace" });
    assert.ok(!ev0.defaultPrevented);
    assert.strictEqual(chips(env).length, 2);
    env.els["rd-q"].value = "";
    env.key(env.els["rd-q"], { key: "Backspace" });
    assert.deepStrictEqual(chipLabels(env), ["L1"]);
    chips(env)[0].children[1].dispatch("click", {}); // ×
    assert.strictEqual(chips(env).length, 0);
    assert.strictEqual(env.els["rd-ok"].disabled, true, "追加モードは 0 件で挿入が無効");
  });

  await check("cite: items の重複 key・不正な行は初期チップに入らない", async () => {
    const env = await open("cite", { d: { mode: "add", items: [{ key: "K1" }, { key: "K1", label: "dup" }, { label: "nokey" }, null, { key: 5 }] } });
    assert.deepStrictEqual(chipLabels(env), ["K1"], "label が無ければ key を出す");
  });

  await check("cite: 編集モード — 「更新」→ {type:replace,keys}・0 件は Enter では削除せず赤ボタンで {type:delete}", async () => {
    const mk = () => open("cite", { d: { mode: "edit", items: [{ key: "K1", label: "L1" }] }, routes: [["/api/items", () => resp(200, { K1: { title: "Paper one" } })]] });
    const env = await mk();
    assert.strictEqual(env.els["rd-ok"].textContent, "更新");
    assert.ok(/引用を編集/.test(env.els["rd-style"].textContent));
    env.key(env.els["rd-q"], { key: "Enter" });
    await tick();
    assert.deepStrictEqual(env.sent, [{ type: "replace", keys: ["K1"] }]);

    const env2 = await mk();
    chips(env2)[0].children[1].dispatch("click", {});
    assert.strictEqual(env2.els["rd-ok"].textContent, "引用を削除");
    assert.ok(env2.els["rd-ok"].classList.contains("rd-danger") && !env2.els["rd-ok"].classList.contains("rd-primary"));
    assert.strictEqual(env2.els["rd-ok"].disabled, false);
    env2.key(env2.els["rd-q"], { key: "Enter" });
    env2.key(env2.els["rd-q"], { key: "Enter", metaKey: true });
    assert.strictEqual(env2.sent.length, 0, "キー操作では 0 件を確定しない");
    env2.els["rd-ok"].dispatch("click", {});
    await tick();
    assert.deepStrictEqual(env2.sent, [{ type: "delete" }]);
  });

  await check("cite: 編集 — チップの title に論文名・蔵書に無ければ注記・/api/items は keys を POST", async () => {
    const env = await open("cite", {
      d: { mode: "edit", items: [{ key: "K1", label: "L1" }, { key: "K2", label: "L2" }] },
      routes: [["/api/items", () => resp(200, { K1: { title: "Paper one" } })]],
    });
    await tick();
    const c = env.calls.find((x) => x.url === "/api/items");
    assert.strictEqual(c.opts.method, "POST");
    assert.deepStrictEqual(JSON.parse(c.opts.body), { keys: ["K1", "K2"] });
    assert.strictEqual(chips(env)[0].title, "Paper one");
    assert.ok(/蔵書に見つかりません/.test(chips(env)[1].title));
    // 取得失敗でもチップは残り警告だけ
    const env2 = await open("cite", { d: { mode: "edit", items: [{ key: "K1", label: "L1" }] }, routes: [["/api/items", () => resp(500, {})]] });
    await tick();
    assert.deepStrictEqual(chipLabels(env2), ["L1"]);
    assert.ok(env2.logs.warn.some((w) => /書誌を取得できません/.test(w)));
  });

  await check("cite: 編集で missing / d が壊れているときの注記", async () => {
    const env = await open("cite", { d: { mode: "edit", items: [{ key: "K1" }], missing: true }, routes: [["/api/items", () => resp(200, {})]] });
    assert.strictEqual(env.els["rd-note"].hidden, false);
    assert.ok(/データが文書にありません/.test(env.els["rd-note"].textContent));
    const env2 = await open("cite", { search: "?host=dotm&sid=ab12&d=%7Bbroken" });
    assert.strictEqual(env2.els["rd-note"].hidden, false);
    assert.ok(/初期データを読めません/.test(env2.els["rd-note"].textContent));
    assert.strictEqual(env2.els["rd-ok"].textContent, "挿入", "d が壊れていても追加モードで開く");
  });

  await check("cite: Esc / キャンセル → {type:cancel}・検索欄以外にフォーカスがあっても Esc / ⌘Enter が効く", async () => {
    const env = await open("cite", { d: { mode: "add", items: [{ key: "K1", label: "L1" }] } });
    const ev = env.key(env.els["rd-q"], { key: "Escape" });
    assert.ok(ev.defaultPrevented);
    await tick();
    assert.deepStrictEqual(env.sent, [{ type: "cancel" }], "検索欄の Esc が document へ伝わって二重に送らない");
    const env2 = await open("cite", { d: { mode: "add", items: [{ key: "K1", label: "L1" }] } });
    env2.key(env2.els["rd-ok"], { key: "Enter", metaKey: true });
    await tick();
    assert.deepStrictEqual(env2.sent, [{ type: "insert", keys: ["K1"] }]);
    const env3 = await open("cite", { d: { mode: "add", items: [] } });
    env3.els["rd-cancel"].dispatch("click", {});
    await tick();
    assert.deepStrictEqual(env3.sent, [{ type: "cancel" }]);
  });

  await check("cite: 行クリックで追加/外す・クリアボタン", async () => {
    const env = await open("cite", { d: { mode: "add", items: [] }, routes: [searchRoute([ITEM_A, ITEM_B])] });
    typeQ(env, "k");
    assert.strictEqual(env.els["rd-clear"].hidden, false);
    env.fire(250);
    await tick();
    rows(env)[1].dispatch("click", {});
    assert.deepStrictEqual(chipLabels(env), ["Sato 2021"]);
    assert.ok(rows(env)[1].classList.contains("rd-sel"), "選択済みの行");
    rows(env)[1].dispatch("click", {});
    assert.strictEqual(chips(env).length, 0, "もう一度押すと外れる");
    env.els["rd-clear"].dispatch("click", {});
    assert.strictEqual(env.els["rd-q"].value, "");
    assert.strictEqual(env.els["rd-clear"].hidden, true);
  });

  await check("cite: 結果の送信が HTTP 500 → 窓は閉じず、理由を画面に出す", async () => {
    const env = await open("cite", { d: { mode: "add", items: [{ key: "K1", label: "L1" }] }, result: 500 });
    env.key(env.els["rd-q"], { key: "Enter" });
    await tick();
    await tick();
    assert.deepStrictEqual(env.sent, [{ type: "insert", keys: ["K1"] }]);
    assert.notStrictEqual(env.doc.title, "openref:done");
    assert.ok(/結果を OpenRef（connectd）へ送れませんでした（HTTP 500）/.test(env.els["rd-debug"].textContent), env.els["rd-debug"] && env.els["rd-debug"].textContent);
  });

  await check("cite: プレビュー（host=dotm なし）— 送らず、1.5 秒後に操作を戻す", async () => {
    const env = await open("cite", { search: "?d=" + encodeURIComponent(JSON.stringify({ mode: "add", items: [{ key: "K1", label: "L1" }] })) });
    env.key(env.els["rd-q"], { key: "Enter" });
    assert.strictEqual(env.sent.length, 0);
    assert.strictEqual(env.els["rd-debug"].textContent, 'result: {"type":"insert","keys":["K1"]}');
    assert.strictEqual(env.els["rd-q"].disabled, true);
    env.fire(1500);
    assert.strictEqual(env.els["rd-q"].disabled, false);
    assert.strictEqual(env.els["rd-ok"].disabled, false);
  });

  // ================= settings-dialog =================
  const STYLES = { styles: [{ id: "vancouver", title: "Vancouver" }, { id: "apa", title: "APA 7th" }, { id: "jama", title: "Journal of the American Medical Association" }] };
  const D_SET = { style: "apa", doiAlways: true, fontName: "Times", fontSize: 11, cites: 3 };
  const stylesRoute = ["/api/styles", () => resp(200, STYLES)];
  const optRows = (env) => env.els["rd-styles"].children.filter((c) => c.className.includes("rd-opt"));

  await check("settings: 初期値の反映・スタイル一覧・選択行・フッタ", async () => {
    const env = await open("settings", { d: D_SET, routes: [stylesRoute] });
    assert.strictEqual(env.els["rd-doi"].checked, true);
    assert.strictEqual(env.els["rd-font"].value, "Times");
    assert.strictEqual(env.els["rd-size"].value, "11", "type=number の value は文字列");
    assert.deepStrictEqual(optRows(env).map((r) => r.className.includes("rd-on")), [false, true, false]);
    assert.ok(optRows(env)[1].innerHTML.includes("APA 7th"));
    assert.strictEqual(env.els["rd-foot"].textContent, "この文書の設定 · APA 7th · 適用すると引用 3 箇所と文献リストを再整形");
    assert.strictEqual(env.els["rd-pane-doc"].hidden, true);
    assert.strictEqual(env.els["rd-tab-style"].getAttribute("aria-selected"), "true");
    assert.strictEqual(env.doc.activeElement, env.els["rd-filter"]);
  });

  await check("settings: d なしの既定（vancouver・Century・10.5・doi なし）で適用 → {type:apply,…}", async () => {
    const env = await open("settings", { routes: [stylesRoute] });
    assert.strictEqual(env.els["rd-size"].value, "10.5");
    env.els["rd-ok"].dispatch("click", {});
    await tick();
    assert.deepStrictEqual(env.sent, [{ type: "apply", style: "vancouver", doiAlways: false, fontName: "Century", fontSize: 10.5 }]);
    assert.strictEqual(env.doc.title, "openref:done");
    assert.strictEqual(env.els["rd-ok"].disabled, true);
    env.els["rd-ok"].dispatch("click", {});
    env.key(env.doc.body, { key: "Enter", metaKey: true });
    env.key(env.doc.body, { key: "Escape" });
    assert.strictEqual(env.sent.length, 1, "二重送信しない");
  });

  await check("settings: tab=doc で文書の設定タブから開く", async () => {
    const env = await open("settings", { tab: "doc", d: D_SET, routes: [stylesRoute] });
    assert.strictEqual(env.els["rd-pane-style"].hidden, true);
    assert.strictEqual(env.els["rd-pane-doc"].hidden, false);
    assert.strictEqual(env.doc.activeElement, env.els["rd-font"]);
    env.els["rd-tab-style"].dispatch("click", {});
    assert.strictEqual(env.els["rd-pane-style"].hidden, false);
    assert.strictEqual(env.els["rd-tab-doc"].getAttribute("aria-selected"), "false");
  });

  await check("settings: サイズは 4〜72 の数値だけ（3.9 / 72.5 / 空 / 文字は弾く・4 / 72 / 10.5 は通す）", async () => {
    for (const bad of ["3.9", "72.5", "", "abc", "0", "-5", "Infinity"]) {
      const env = await open("settings", { d: D_SET, routes: [stylesRoute] });
      env.els["rd-size"].value = bad;
      env.els["rd-ok"].dispatch("click", {});
      await tick();
      assert.strictEqual(env.sent.length, 0, JSON.stringify(bad) + " は送らない");
      assert.strictEqual(env.els["rd-err"].hidden, false);
      assert.strictEqual(env.els["rd-err"].textContent, "サイズは 4〜72 pt の数値で入力してください。");
      assert.strictEqual(env.els["rd-pane-doc"].hidden, false, "文書の設定タブへ移す");
      assert.strictEqual(env.doc.activeElement, env.els["rd-size"]);
      assert.strictEqual(env.els["rd-ok"].disabled, false, "窓は操作できるまま");
    }
    for (const [good, num] of [["4", 4], ["72", 72], ["10.5", 10.5], ["12", 12]]) {
      const env = await open("settings", { d: D_SET, routes: [stylesRoute] });
      env.els["rd-size"].value = good;
      env.els["rd-ok"].dispatch("click", {});
      await tick();
      assert.strictEqual(env.sent.length, 1, good + " は通す");
      assert.strictEqual(env.sent[0].fontSize, num);
    }
  });

  await check("settings: 不正 → 修正して再適用でエラー表示が消える・フォント名は trim して空なら Century", async () => {
    const env = await open("settings", { d: D_SET, routes: [stylesRoute] });
    env.els["rd-size"].value = "1";
    env.els["rd-ok"].dispatch("click", {});
    assert.strictEqual(env.els["rd-err"].hidden, false);
    env.els["rd-size"].value = "9";
    env.els["rd-font"].value = "   ";
    env.els["rd-doi"].checked = false;
    env.els["rd-ok"].dispatch("click", {});
    await tick();
    assert.strictEqual(env.els["rd-err"].hidden, true);
    assert.deepStrictEqual(env.sent, [{ type: "apply", style: "apa", doiAlways: false, fontName: "Century", fontSize: 9 }]);
    const env2 = await open("settings", { d: D_SET, routes: [stylesRoute] });
    env2.els["rd-font"].value = "  Arial ";
    env2.els["rd-ok"].dispatch("click", {});
    await tick();
    assert.strictEqual(env2.sent[0].fontName, "Arial");
  });

  await check("settings: Enter — 入力欄(フォント/サイズ)と ⌘Enter は適用・絞り込み欄の Enter は適用しない・IME 中は無視・Esc はキャンセル", async () => {
    const env = await open("settings", { d: D_SET, routes: [stylesRoute] });
    env.key(env.els["rd-filter"], { key: "Enter", isComposing: true });
    env.key(env.els["rd-font"], { key: "Enter", keyCode: 229 });
    env.key(env.els["rd-filter"], { key: "Enter" });
    env.key(env.els["rd-ok"], { key: "Enter" });
    assert.strictEqual(env.sent.length, 0);
    env.key(env.els["rd-font"], { key: "Enter" });
    await tick();
    assert.strictEqual(env.sent.length, 1);
    assert.strictEqual(env.sent[0].type, "apply");
    const env2 = await open("settings", { d: D_SET, routes: [stylesRoute] });
    env2.key(env2.els["rd-filter"], { key: "Enter", metaKey: true });
    await tick();
    assert.strictEqual(env2.sent[0].type, "apply");
    const env3 = await open("settings", { d: D_SET, routes: [stylesRoute] });
    const ev = env3.key(env3.els["rd-filter"], { key: "Escape" });
    assert.ok(ev.defaultPrevented);
    await tick();
    assert.deepStrictEqual(env3.sent, [{ type: "cancel" }]);
    const env4 = await open("settings", { d: D_SET, routes: [stylesRoute] });
    env4.els["rd-cancel"].dispatch("click", {});
    await tick();
    assert.deepStrictEqual(env4.sent, [{ type: "cancel" }]);
  });

  await check("settings: 絞り込み（title/id）・↑↓・隠れた選択は Enter で先頭へ・クリック/ダブルクリック", async () => {
    const env = await open("settings", { d: D_SET, routes: [stylesRoute] });
    env.els["rd-filter"].value = "vanc";
    env.els["rd-filter"].dispatch("input", {});
    assert.strictEqual(optRows(env).length, 1);
    env.key(env.els["rd-filter"], { key: "Enter" }); // 選択 apa が隠れている → 先頭 vancouver
    assert.ok(optRows(env)[0].className.includes("rd-on"));
    assert.ok(/Vancouver/.test(env.els["rd-foot"].textContent));
    env.els["rd-filter"].value = "";
    env.els["rd-filter"].dispatch("input", {});
    env.key(env.els["rd-filter"], { key: "ArrowDown" });
    assert.ok(optRows(env)[1].className.includes("rd-on"), "vancouver の次 = apa");
    env.key(env.els["rd-filter"], { key: "ArrowUp" });
    env.key(env.els["rd-filter"], { key: "ArrowUp" });
    assert.ok(optRows(env)[0].className.includes("rd-on"), "先頭で止まる");
    env.els["rd-filter"].value = "zzz";
    env.els["rd-filter"].dispatch("input", {});
    assert.strictEqual(env.els["rd-styles"].children[0].textContent, "該当なし");
    env.els["rd-filter"].value = "apa";
    env.els["rd-filter"].dispatch("input", {});
    optRows(env)[0].dispatch("dblclick", {});
    await tick();
    assert.strictEqual(env.sent[0].style, "apa", "ダブルクリックで選んで適用");
  });

  await check("settings: スタイル一覧が HTTP 500 → 取得できません(connectd: HTTP 500)・文書のスタイルは「未導入」と言わずに出す", async () => {
    const env = await open("settings", { d: D_SET, routes: [["/api/styles", () => resp(500, {})]] });
    const msgs = env.els["rd-styles"].children.filter((c) => c.className.includes("rd-msg"));
    assert.strictEqual(msgs.length, 1);
    assert.ok(msgs[0].className.includes("rd-err"));
    assert.strictEqual(msgs[0].textContent, "スタイル一覧を取得できません（connectd: HTTP 500）");
    assert.strictEqual(optRows(env).length, 1);
    assert.ok(!optRows(env)[0].innerHTML.includes("未導入"));
    assert.ok(optRows(env)[0].innerHTML.includes("apa"));
    env.els["rd-ok"].dispatch("click", {});
    await tick();
    assert.strictEqual(env.sent[0].style, "apa", "一覧が取れなくても今のスタイルで適用できる");
  });

  await check("settings: 文書のスタイルが一覧に無い（取得は成功）→「未導入 — 適用時に取得」で先頭に出す", async () => {
    const env = await open("settings", { d: Object.assign({}, D_SET, { style: "nature" }), routes: [stylesRoute] });
    assert.strictEqual(optRows(env).length, 4);
    assert.ok(optRows(env)[0].innerHTML.includes("nature（この Mac に未導入"));
    assert.ok(optRows(env)[0].className.includes("rd-on"));
  });

  await check("settings: 引用 0 件のフッタ・読み込み前は「読み込み中…」", async () => {
    const dd = deferred();
    const env = await open("settings", { d: Object.assign({}, D_SET, { cites: 0 }), routes: [["/api/styles", () => dd.promise]] });
    assert.strictEqual(env.els["rd-styles"].children[0].textContent, "読み込み中…");
    dd.resolve({ ok: true, status: 200, json: () => Promise.resolve(STYLES) });
    await tick();
    assert.ok(/引用はまだありません/.test(env.els["rd-foot"].textContent));
  });

  await check("settings: 結果の送信が HTTP 500 → 閉じずに理由を出す", async () => {
    const env = await open("settings", { d: D_SET, routes: [stylesRoute], result: 500 });
    env.els["rd-ok"].dispatch("click", {});
    await tick();
    await tick();
    assert.notStrictEqual(env.doc.title, "openref:done");
    assert.ok(/HTTP 500/.test(env.els["rd-debug"].textContent));
  });

  // ================= notice =================
  await check("notice: error — 既定の見出し「エラー」・警告アイコン・閉じる → {type:close}・Esc も close", async () => {
    const env = await open("notice", { d: { kind: "error", message: "失敗しました", detail: "詳細" } });
    assert.strictEqual(env.els["rd-title"].textContent, "エラー");
    assert.strictEqual(env.els["rd-msg"].textContent, "失敗しました");
    assert.strictEqual(env.els["rd-detail"].textContent, "詳細");
    assert.strictEqual(env.els["rd-detail"].hidden, false);
    assert.strictEqual(env.els["rd-warn"].hidden, false);
    assert.strictEqual(env.els["rd-ok"].textContent, "閉じる");
    assert.ok(env.doc.body.classList.contains("rd-error"));
    assert.strictEqual(env.doc.activeElement, env.els["rd-ok"]);
    env.els["rd-ok"].dispatch("click", {});
    await tick();
    assert.deepStrictEqual(env.sent, [{ type: "close" }]);
    env.key(env.doc.body, { key: "Escape" });
    assert.strictEqual(env.sent.length, 1, "送信後の Esc は無視");
    const env2 = await open("notice", { d: { message: "x" } });
    env2.key(env2.doc.body, { key: "Escape" });
    await tick();
    assert.deepStrictEqual(env2.sent, [{ type: "close" }]);
  });

  await check("notice: 種別が不正なら error 扱い・d なしでも落ちない", async () => {
    const env = await open("notice", { d: { kind: "weird", message: "m" } });
    assert.ok(env.doc.body.classList.contains("rd-error"));
    const env2 = await open("notice", {});
    assert.strictEqual(env2.els["rd-title"].textContent, "エラー");
    assert.strictEqual(env2.els["rd-copy"].hidden, true, "error で copy 指定が無ければコピーは出さない");
  });

  await check("notice: confirm — OK → {type:ok}・キャンセル → {type:cancel}・Esc → cancel・OK ラベル", async () => {
    const mk = () => open("notice", { d: { kind: "confirm", message: "消しますか", ok: "削除" } });
    const env = await mk();
    assert.strictEqual(env.els["rd-title"].textContent, "確認");
    assert.strictEqual(env.els["rd-ok"].textContent, "削除");
    assert.strictEqual(env.els["rd-cancel"].hidden, false);
    assert.strictEqual(env.els["rd-warn"].hidden, true);
    assert.strictEqual(env.doc.activeElement, env.els["rd-ok"], "通常の確認は OK が既定");
    env.els["rd-ok"].dispatch("click", {});
    await tick();
    assert.deepStrictEqual(env.sent, [{ type: "ok" }]);
    const env2 = await mk();
    env2.els["rd-cancel"].dispatch("click", {});
    await tick();
    assert.deepStrictEqual(env2.sent, [{ type: "cancel" }]);
    const env3 = await mk();
    env3.key(env3.doc.body, { key: "Escape" });
    await tick();
    assert.deepStrictEqual(env3.sent, [{ type: "cancel" }]);
  });

  await check("notice: 破壊的な confirm は既定フォーカスがキャンセル・OK は赤", async () => {
    const env = await open("notice", { d: { kind: "confirm", message: "削除", danger: true } });
    assert.strictEqual(env.els["rd-ok"].textContent, "OK");
    assert.ok(env.els["rd-ok"].classList.contains("rd-danger") && !env.els["rd-ok"].classList.contains("rd-primary"));
    assert.strictEqual(env.doc.activeElement, env.els["rd-cancel"]);
  });

  await check("notice: info — コピーボタン・本文は title/message/detail を改行でつなぐ・成功の表示", async () => {
    const env = await open("notice", { d: { kind: "info", title: "診断", message: "本文", detail: "細部" }, clipboard: true });
    assert.strictEqual(env.els["rd-title"].textContent, "診断");
    assert.strictEqual(env.els["rd-copy"].hidden, false);
    env.els["rd-copy"].dispatch("click", {});
    await tick();
    assert.deepStrictEqual(env.copied, ["診断\n本文\n細部"]);
    assert.strictEqual(env.els["rd-copied"].textContent, "コピーしました");
    env.els["rd-ok"].dispatch("click", {});
    await tick();
    assert.deepStrictEqual(env.sent, [{ type: "close" }]);
  });

  await check("notice: コピー不可（clipboard 拒否 → execCommand 失敗）→ 本文を選択して ⌘C を案内", async () => {
    const env = await open("notice", { d: { kind: "error", message: "m", copy: true }, clipboard: false });
    assert.strictEqual(env.els["rd-copy"].hidden, false);
    env.els["rd-copy"].dispatch("click", {});
    await tick();
    await tick();
    assert.ok(env.copied.includes("selected:rd-msg"));
    assert.ok(/⌘C でコピーしてください/.test(env.els["rd-copied"].textContent));
  });

  await check("notice: 結果の送信が HTTP 500 → 閉じずに理由・プレビューは送らず 1.5 秒後に戻す", async () => {
    const env = await open("notice", { d: { kind: "error", message: "m" }, result: 500 });
    env.els["rd-ok"].dispatch("click", {});
    await tick();
    await tick();
    assert.notStrictEqual(env.doc.title, "openref:done");
    assert.ok(/HTTP 500/.test(env.els["rd-debug"].textContent));
    const env2 = await open("notice", { search: "?d=" + encodeURIComponent(JSON.stringify({ kind: "confirm", message: "m" })) });
    env2.els["rd-ok"].dispatch("click", {});
    assert.strictEqual(env2.sent.length, 0);
    assert.strictEqual(env2.els["rd-debug"].textContent, 'result: {"type":"ok"}');
    assert.strictEqual(env2.els["rd-ok"].disabled, true);
    env2.fire(1500);
    assert.strictEqual(env2.els["rd-ok"].disabled, false);
    assert.strictEqual(env2.els["rd-cancel"].disabled, false);
  });

  // ================= 細部(cite / settings / notice) =================
  await check("cite: ↑ で戻る・先頭で止まる・結果が無いときの矢印は何もしない", async () => {
    const env = await open("cite", { d: { mode: "add", items: [] }, routes: [searchRoute([ITEM_A, ITEM_B])] });
    const ev0 = env.key(env.els["rd-q"], { key: "ArrowUp" });
    assert.ok(ev0.defaultPrevented);
    typeQ(env, "x");
    env.fire(250);
    await tick();
    env.key(env.els["rd-q"], { key: "ArrowDown" });
    env.key(env.els["rd-q"], { key: "ArrowUp" });
    assert.ok(rows(env)[0].classList.contains("rd-hi"));
    env.key(env.els["rd-q"], { key: "ArrowUp" });
    assert.ok(rows(env)[0].classList.contains("rd-hi"), "先頭で止まる");
    assert.strictEqual(rows(env)[0].getAttribute("aria-selected"), "true");
  });

  await check("cite: デバウンス — 続けて打つと前のタイマーを捨て、検索は最後の 1 回だけ", async () => {
    const env = await open("cite", { d: { mode: "add", items: [] }, routes: [searchRoute([ITEM_A])] });
    typeQ(env, "k");
    typeQ(env, "ka");
    typeQ(env, "kan");
    assert.strictEqual(env.timers.filter((t) => t.ms === 250).length, 1, "250ms のタイマーは 1 本だけ残る");
    env.fire(250);
    await tick();
    assert.deepStrictEqual(env.calls.filter((c) => c.url.startsWith("/api/search")).map((c) => c.url), ["/api/search?q=kan"]);
  });

  await check("cite: 入力を空にすると走行中の検索の結果は捨てる（S.seq++）", async () => {
    const d1 = deferred();
    const env = await open("cite", { d: { mode: "add", items: [] }, routes: [["/api/search?q=", () => d1.promise]] });
    typeQ(env, "kano");
    env.fire(250);
    typeQ(env, "");
    d1.resolve({ ok: true, status: 200, json: () => Promise.resolve({ items: [ITEM_A] }) });
    await tick();
    assert.ok(/Enter で追加/.test(rows(env)[0].textContent), "ヒントに戻ったまま");
    assert.strictEqual(rows(env).length, 1);
  });

  await check("cite: 追加済みの文献は Enter で二重に入らない(add の重複ガード)", async () => {
    const env = await open("cite", { d: { mode: "add", items: [] }, routes: [searchRoute([ITEM_A])] });
    for (let i = 0; i < 2; i++) {
      typeQ(env, "kano");
      env.fire(250);
      await tick();
      env.key(env.els["rd-q"], { key: "Enter" });
    }
    assert.deepStrictEqual(chipLabels(env), ["Kano 2020"]);
  });

  await check("cite: 行の title / meta の escape(& < > \")・年なしの label・著者なしは ?", async () => {
    const odd = { key: "O1", title: 'A & B "quoted" <i>', first_author: "Lee", journal: "J", year: null };
    const anon = { key: "O2", title: "", first_author: "", journal: "", year: 2019 };
    const env = await open("cite", { d: { mode: "add", items: [] }, routes: [searchRoute([odd, anon])] });
    typeQ(env, "x");
    env.fire(250);
    await tick();
    assert.ok(rows(env)[0].innerHTML.includes("A &amp; B &quot;quoted&quot; &lt;i&gt;"), rows(env)[0].innerHTML);
    assert.ok(rows(env)[1].innerHTML.includes("（無題）"));
    env.key(env.els["rd-q"], { key: "Enter" });
    typeQ(env, "y");
    env.fire(250);
    await tick();
    env.key(env.els["rd-q"], { key: "ArrowDown" });
    env.key(env.els["rd-q"], { key: "Enter" });
    assert.deepStrictEqual(chipLabels(env), ["Lee", "? 2019"]);
  });

  await check("settings: 絞り込みは title と id の両方に効く（大文字小文字は無視）", async () => {
    const env = await open("settings", { d: D_SET, routes: [stylesRoute] });
    const ids = () => optRows(env).map((r) => r.innerHTML.match(/rd-opt-id">([^<]*)</)[1]);
    const filter = (v) => {
      env.els["rd-filter"].value = v;
      env.els["rd-filter"].dispatch("input", {});
    };
    filter("MEDICAL"); // title だけに含まれる
    assert.deepStrictEqual(ids(), ["jama"]);
    filter("jama"); // id だけに含まれる
    assert.deepStrictEqual(ids(), ["jama"]);
    filter("APA"); // 両方
    assert.deepStrictEqual(ids(), ["apa"]);
    filter("");
    assert.strictEqual(ids().length, 3);
  });

  await check("settings: 一覧の行クリックでフッタの選択が変わる・適用にその id が入る", async () => {
    const env = await open("settings", { d: D_SET, routes: [stylesRoute] });
    optRows(env)[0].dispatch("click", {});
    assert.ok(/Vancouver/.test(env.els["rd-foot"].textContent));
    env.els["rd-ok"].dispatch("click", {});
    await tick();
    assert.strictEqual(env.sent[0].style, "vancouver");
  });

  await check("notice: detail 無しなら detail 欄は出さない・message 無しは空", async () => {
    const env = await open("notice", { d: { kind: "info" } });
    assert.strictEqual(env.els["rd-detail"].hidden, true);
    assert.strictEqual(env.els["rd-msg"].textContent, "");
    assert.strictEqual(env.els["rd-title"].textContent, "情報");
    assert.strictEqual(env.els["rd-copy"].hidden, false);
  });

  await check("disabled の要素はクリックもキーも受けない(偽 DOM が本物と同じ前提)", async () => {
    const env = await open("cite", { d: { mode: "add", items: [] } });
    let hit = 0;
    env.els["rd-ok"].addEventListener("click", () => hit++);
    assert.strictEqual(env.els["rd-ok"].disabled, true);
    env.els["rd-ok"].dispatch("click", {});
    assert.strictEqual(hit, 0);
    env.els["rd-ok"].disabled = false;
    env.els["rd-ok"].dispatch("click", {});
    assert.strictEqual(hit, 1);
  });

  console.log(failures ? `\n${failures} FAILED` : "\nALL PASS");
  process.exit(failures ? 1 : 0);
})();
