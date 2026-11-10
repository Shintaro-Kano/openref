/* 読み手の PDF の読み方の試験（node app/src/lib/shelf/range.test.mjs）。実測で効いた制約を機械で守る:
   1 区間 ≤ asset_reply の上限（Rust の MAX_LEN と結ぶ）・素の GET を出さない・no-store・同時 fetch の上限・onDataRange は繋いでから 1 回・閉じた後は何も呼ばない。
   pdf.js は最小の偽物（requestDataRange を呼ぶ側 = 試験。onDataRange の受け口だけ）。fetch は差し替え */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { installLoader } from "../wiki/test-harness.mjs";

installLoader({
  "pdfjs-dist": "export class PDFDataRangeTransport { constructor(length, initial, done) { this.length = length; this.initial = initial; this.done = done; this.got = []; } onDataRange(begin, chunk) { this.got.push([begin, chunk]); } }",
});
const { AssetRangeTransport, CHUNK, probeLength } = await import("./rangeTransport.ts");
const { ASSET_MAX } = await import("./tree.ts");

const sleep = (ms = 0) => new Promise((r) => setTimeout(r, ms));
let tests = 0;

// asset_reply の上限と同じ値（ずれると pdf.js の chunk の境目が合わず Bad end offset になる）
{
  const rust = readFileSync(new URL("../../../src-tauri/src/wiki_asset.rs", import.meta.url), "utf8");
  const m = /const MAX_LEN[^=]*=\s*([\d_]+)(?:\s*\*\s*([\d_]+))?\s*;/.exec(rust);
  assert.ok(m, "wiki_asset.rs の MAX_LEN が読めない");
  assert.equal(ASSET_MAX, Number(m[1].replaceAll("_", "")) * Number((m[2] ?? "1").replaceAll("_", "")), "ASSET_MAX が Rust の MAX_LEN と違う");
  assert.equal(CHUNK, 64 * 1024, "chunk は 64 KiB（開くだけで読む量 = ページ数 × chunk）");
  tests++;
}

/** 偽の asset_reply: Range のとおりの byte を返す。上限を超える区間・Range の無い要求は失敗にする（本物は切る / 全量を読む） */
function fakeAsset(size, { hold = false } = {}) {
  const log = [];
  const waiting = [];
  let live = 0;
  let peak = 0;
  const reply = (b, e) => {
    const body = new Uint8Array(e - b + 1);
    for (let i = 0; i < body.length; i++) body[i] = (b + i) % 251;
    return new Response(body, { status: 206, headers: { "content-range": `bytes ${b}-${e}/${size}` } });
  };
  globalThis.fetch = async (url, init) => {
    const range = new Headers(init?.headers).get("range");
    log.push({ url, range, cache: init?.cache, signal: init?.signal });
    live++;
    peak = Math.max(peak, live);
    try {
      if (hold) await new Promise((r) => waiting.push(r));
      init?.signal?.throwIfAborted();
      const m = /^bytes=(\d+)-(\d+)$/.exec(range ?? "");
      if (!m) throw new Error("素の GET を出した");
      const [b, e] = [Number(m[1]), Number(m[2])];
      assert.ok(e - b + 1 <= ASSET_MAX, `1 区間が上限を超えた: ${e - b + 1}`);
      return reply(b, e);
    } finally {
      live--;
    }
  };
  return { log, waiting, peak: () => peak };
}

// 長さ = bytes=0-0 の分母（素の GET / HEAD は全量を読む）・206 でなければ失敗
{
  const a = fakeAsset(1500000000);
  assert.equal(await probeLength("asset://x", new AbortController().signal), 1500000000);
  assert.deepEqual(a.log.map((c) => [c.range, c.cache]), [["bytes=0-0", "no-store"]]);
  globalThis.fetch = async () => new Response("", { status: 200 });
  await assert.rejects(probeLength("asset://x", new AbortController().signal), /status 200/);
  globalThis.fetch = async () => new Response("", { status: 206, headers: { "content-range": "bytes */*" } });
  await assert.rejects(probeLength("asset://x", new AbortController().signal), /content-range/);
  tests++;
}

// 区間が上限を超える = 分けて fetch し、繋いでから onDataRange を 1 回（pdf.js は 1 回で reader が閉じる）
{
  const size = ASSET_MAX * 3;
  const a = fakeAsset(size);
  const errors = [];
  const tr = new AssetRangeTransport("asset://x", size, (e) => errors.push(e));
  assert.equal(tr.length, size);
  assert.equal(tr.initial, null, "全量の順読みをしない");
  assert.equal(tr.done, true, "progressiveDone でない");
  const begin = 100;
  const end = begin + ASSET_MAX * 2 + 7;
  tr.requestDataRange(begin, end);
  for (let i = 0; i < 50 && !tr.got.length; i++) await sleep();
  assert.deepEqual(errors, []);
  assert.equal(tr.got.length, 1, "onDataRange が 1 回でない");
  const [b, chunk] = tr.got[0];
  assert.equal(b, begin);
  assert.equal(chunk.byteLength, end - begin);
  for (const i of [0, ASSET_MAX - 1, ASSET_MAX, ASSET_MAX * 2, end - begin - 1]) assert.equal(chunk[i], (begin + i) % 251, `繋ぎ目の byte が違う: ${i}`);
  assert.equal(a.log.length, 3, "分けていない");
  assert.ok(a.log.every((c) => c.cache === "no-store"), "no-store でない fetch がある");
  tests++;
}

// 上限以下の区間は fetch 1 本
{
  const a = fakeAsset(1 << 20);
  const tr = new AssetRangeTransport("asset://x", 1 << 20, () => {});
  tr.requestDataRange(0, CHUNK);
  for (let i = 0; i < 50 && !tr.got.length; i++) await sleep();
  assert.deepEqual([a.log.length, a.log[0].range, tr.got[0][1].byteLength], [1, `bytes=0-${CHUNK - 1}`, CHUNK]);
  tests++;
}

// 同時 fetch は文書単位で 16 まで（1,000 ページの平らな木では開くだけで 1,000 本の range が来る）
{
  const a = fakeAsset(1 << 30, { hold: true });
  const tr = new AssetRangeTransport("asset://x", 1 << 30, () => {});
  for (let i = 0; i < 100; i++) tr.requestDataRange(i * CHUNK, (i + 1) * CHUNK);
  await sleep(5);
  assert.equal(a.peak(), 16, "同時 fetch が 16 でない");
  for (let i = 0; i < 400 && tr.got.length < 100; i++) {
    a.waiting.splice(0).forEach((r) => r());
    await sleep();
  }
  assert.equal(tr.got.length, 100, "待たせた要求が流れない");
  assert.equal(a.peak(), 16, "同時 fetch が 16 を超えた");
  tests++;
}

// 閉じた後は onDataRange / onError を呼ばない（読み手はもう居ない）
{
  const a = fakeAsset(1 << 20, { hold: true });
  const errors = [];
  const tr = new AssetRangeTransport("asset://x", 1 << 20, (e) => errors.push(e));
  tr.requestDataRange(0, CHUNK);
  await sleep(2);
  tr.abort();
  assert.ok(a.log[0].signal.aborted, "fetch を止めない");
  a.waiting.splice(0).forEach((r) => r());
  await sleep(5);
  assert.deepEqual([tr.got.length, errors.length], [0, 0], "閉じた後に呼んだ");
  // 閉じたのが本文を読んでいる最中 = fetch は返っている（閉じる競合）。届いた分を渡さない
  let body;
  const held = new Promise((r) => (body = r));
  globalThis.fetch = async () => ({ status: 206, arrayBuffer: () => held });
  const tr2 = new AssetRangeTransport("asset://x", 1 << 20, (e) => errors.push(e));
  tr2.requestDataRange(0, CHUNK);
  await sleep(2);
  tr2.abort();
  body(new ArrayBuffer(CHUNK));
  await sleep(5);
  assert.deepEqual([tr2.got.length, errors.length], [0, 0], "閉じた後に届いた分を渡した");
  tests++;
}

// 読めない = 失敗を読み手へ（pdf.js の transport には失敗を伝える口が無い = 待ちが残り続ける）
{
  for (const [label, res] of [
    ["206 でない", () => new Response(new Uint8Array(CHUNK), { status: 200 })],
    ["長さが足りない", () => new Response(new Uint8Array(10), { status: 206 })],
  ]) {
    globalThis.fetch = async () => res();
    const errors = [];
    const tr = new AssetRangeTransport("asset://x", 1 << 20, (e) => errors.push(e));
    tr.requestDataRange(0, CHUNK);
    for (let i = 0; i < 50 && !errors.length; i++) await sleep();
    assert.equal(errors.length, 1, `${label}: 失敗を伝えない`);
    assert.equal(tr.got.length, 0, `${label}: 壊れた区間を渡した`);
  }
  tests++;
}

console.log(`shelf range: ok (${tests} tests)`);
