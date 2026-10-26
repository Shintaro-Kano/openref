// dialog-common.js の送信先の切り替え（?host=dotm = Startup テンプレート版の OpenRef Dialog.app）の node テスト。
// dotm: 結果を /word/v2/dialog/<sid>/result へ POST（X-OpenRef ヘッダ）→ 届いたら document.title = "openref:done"・
// ready は即時・5 分制限の予告帯なし。Office: 従来どおり messageParent・onReady 待ち・予告帯あり。
// 実行: node word/test-dialog-dotm.cjs
"use strict";
const fs = require("fs");
const path = require("path");
const vm = require("vm");
const assert = require("assert");

const SRC = fs.readFileSync(path.join(__dirname, "dialog-common.js"), "utf8");

function fakeEl() {
  return { id: "", className: "", textContent: "", children: [], appendChild(c) { this.children.push(c); }, insertBefore(c) { this.children.unshift(c); } };
}
/** 窓 1 枚分の偽の環境。fetchImpl = (url, opts) → Promise<{ok, status}> */
function load(search, extra) {
  const els = {};
  const timers = [];
  const calls = [];
  const document = {
    title: "OpenRef — 引用",
    body: fakeEl(),
    getElementById: (id) => els[id] || null,
    createElement: () => fakeEl(),
    querySelector: () => null,
  };
  document.body.appendChild = function (c) {
    if (c.id) els[c.id] = c;
    this.children.push(c);
  };
  const win = {
    location: { search: search, origin: "https://localhost:23120" },
    document: document,
    URLSearchParams: URLSearchParams,
    console: { log() {}, warn() {}, error() {} },
    setTimeout: (fn, ms) => timers.push({ fn, ms }),
    fetch: (url, opts) => {
      calls.push({ url, opts });
      return extra.fetch(url, opts);
    },
  };
  if (extra.Office) win.Office = extra.Office;
  win.window = win;
  const ctx = vm.createContext(win);
  vm.runInContext(SRC, ctx);
  return { RD: win.OpenRefDialog, document, els, timers, calls };
}
const tick = () => new Promise((r) => setImmediate(r));

let failures = 0;
async function check(name, fn) {
  try {
    await fn();
    console.log("OK   " + name);
  } catch (e) {
    failures++;
    console.log("FAIL " + name + ": " + e.message);
  }
}

(async () => {
  await check("dotm: ready は即時・予告帯なし・結果を POST して届いたら openref:done", async () => {
    const env = load("?host=dotm&sid=ab12&d=%7B%7D", { fetch: () => Promise.resolve({ ok: true, status: 200 }) });
    let ran = false;
    env.RD.ready(() => (ran = true));
    assert.ok(ran, "ready は同期で呼ぶ（Office を待たない）");
    env.RD.limitBanner();
    assert.strictEqual(env.timers.length, 0, "5 分制限の予告帯は出さない");
    assert.strictEqual(env.RD.params().sid, "ab12");
    assert.strictEqual(env.RD.send({ type: "insert", keys: ["K1"] }), true);
    assert.strictEqual(env.calls.length, 1);
    const c = env.calls[0];
    assert.strictEqual(c.url, "/word/v2/dialog/ab12/result");
    assert.strictEqual(c.opts.method, "POST");
    assert.strictEqual(c.opts.headers["X-OpenRef"], "word");
    assert.deepStrictEqual(JSON.parse(c.opts.body), { type: "insert", keys: ["K1"] });
    assert.notStrictEqual(env.document.title, "openref:done", "届く前は閉じない");
    await tick();
    assert.strictEqual(env.document.title, "openref:done");
  });

  await check("dotm: 送れなければ窓は閉じず理由を出す", async () => {
    const env = load("?host=dotm&sid=zz", { fetch: () => Promise.resolve({ ok: false, status: 404 }) });
    env.RD.send({ type: "cancel" });
    await tick();
    await tick();
    assert.notStrictEqual(env.document.title, "openref:done");
    assert.ok(/HTTP 404/.test(env.els["rd-debug"].textContent), env.els["rd-debug"] && env.els["rd-debug"].textContent);
  });

  await check("Office: 従来どおり onReady を待って messageParent・予告帯は 4:15", async () => {
    const sent = [];
    let onReady = null;
    const Office = {
      onReady: (cb) => (onReady = cb),
      context: { ui: { messageParent: (s) => sent.push(s) } },
    };
    const env = load("?d=%7B%7D", { Office, fetch: () => assert.fail("fetch しない") });
    let ran = false;
    env.RD.ready(() => (ran = true));
    assert.ok(!ran, "onReady を待つ");
    assert.ok(env.timers.some((t) => t.ms === 3000), "3 秒で見切る");
    onReady({ host: "Word" });
    assert.ok(ran);
    assert.strictEqual(env.RD.send({ type: "cancel" }), true);
    assert.deepStrictEqual(sent, ['{"type":"cancel"}']);
    assert.strictEqual(env.calls.length, 0);
    env.RD.limitBanner();
    assert.ok(env.timers.some((t) => t.ms === (4 * 60 + 15) * 1000), "予告帯");
  });

  console.log(failures ? `\n${failures} FAILED` : "\nALL PASS");
  process.exit(failures ? 1 : 0);
})();
