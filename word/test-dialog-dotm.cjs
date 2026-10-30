// dialog-common.js の送信先の切り替え（?host=dotm = OpenRef Dialog.app）の node テスト。
// dotm: 結果を /word/v2/dialog/<sid>/result へ POST（X-OpenRef ヘッダ）→ 届いたら document.title = "openref:done"・ready は即時。
// host=dotm なし（プレビュー）: 送らずに debug 行へ出す（send は false = 呼び手が操作を戻す）。
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
  await check("dotm: ready は即時・結果を POST して届いたら openref:done", async () => {
    const env = load("?host=dotm&sid=ab12&d=%7B%7D", { fetch: () => Promise.resolve({ ok: true, status: 200 }) });
    let ran = false;
    env.RD.ready(() => (ran = true));
    assert.ok(ran, "ready は同期で呼ぶ");
    assert.strictEqual(env.timers.length, 0);
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

  await check("プレビュー（host=dotm なし）: ready は即時・送らずに debug 行へ・send は false", async () => {
    const env = load("?d=%7B%7D", { fetch: () => assert.fail("fetch しない") });
    let ran = false;
    env.RD.ready(() => (ran = true));
    assert.ok(ran);
    assert.strictEqual(env.RD.send({ type: "cancel" }), false);
    assert.strictEqual(env.calls.length, 0);
    assert.strictEqual(env.els["rd-debug"].textContent, 'result: {"type":"cancel"}');
    assert.notStrictEqual(env.document.title, "openref:done");
  });

  console.log(failures ? `\n${failures} FAILED` : "\nALL PASS");
  process.exit(failures ? 1 : 0);
})();
