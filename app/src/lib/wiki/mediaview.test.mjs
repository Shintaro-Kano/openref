/* OpenNote: MediaView の PDF 判定の回帰（node app/src/lib/wiki/mediaview.test.mjs・Svelte 5 の実部品 + jsdom）。
   同じ部品の page だけを差し替えたとき、確かめていない src で <iframe> が一瞬でも作られないこと（回帰:
   pdfOk が確かめた src を持たないと、差し替えの flush で前の結果のまま新しい src の iframe が作られる）・読めなかった（403）ときの案内。
   .svelte / .svelte.ts は svelte/compiler で読む（node の load hook・拡張子なしの相対 import は .ts を補う） */
import assert from "node:assert/strict";
import { setupHarness } from "./test-harness.mjs";

// 土台（test-harness.mjs）: loader・jsdom の global 化・Tauri の偽物
setupHarness();

// fetch の差し替え: src ごとに応答を手で返す（isPdf は Range で先頭を読む）
const pending = new Map(); // url → resolve(body, status)
globalThis.fetch = (url) =>
  new Promise((res) => {
    pending.set(String(url), (text, status = 206) => res(new Response(text, { status })));
  });
const answer = async (url, text, status) => {
  for (let i = 0; i < 50 && !pending.has(url); i++) await new Promise((r) => setTimeout(r, 0));
  assert.ok(pending.has(url), `fetch が来ない: ${url}`);
  pending.get(url)(text, status);
  pending.delete(url);
  for (let i = 0; i < 10; i++) await new Promise((r) => setTimeout(r, 0));
};

const { mount, unmount, flushSync } = await import("svelte");
const { proxy } = await import("svelte/internal/client");
const { default: MediaView } = await import("./MediaView.svelte");
const { assetUrl } = await import("./render/asset.ts");

const page = (path) => ({ path, abs_path: "/v/" + path, kind: "media", media: "pdf", title: path, size: 1, mtime: 0 });
const target = document.createElement("div");
document.body.append(target);
// 作られた iframe を全部記録（直後に外されても残る）
const seen = [];
new MutationObserver((recs) => {
  for (const r of recs) for (const n of r.addedNodes) {
    if (n.nodeType !== 1) continue;
    for (const f of n.matches?.("iframe") ? [n] : n.querySelectorAll("iframe")) seen.push(f.getAttribute("src"));
  }
}).observe(target, { childList: true, subtree: true });
const settle = () => new Promise((r) => setTimeout(r, 0));
let tests = 0;

const props = proxy({ page: page("a.pdf"), root: "/v", onNavigate: () => {}, readPage: async () => page("x") });
const cmp = mount(MediaView, { target, props });
flushSync();
const A = assetUrl("/v/a.pdf");
const B = assetUrl("/v/b.pdf");
const C = assetUrl("/v/c.pdf");
assert.equal(target.querySelector("iframe"), null, "確認前に iframe が作られた");
await answer(A, "%PDF-1.7\n");
flushSync();
assert.equal(target.querySelector("iframe")?.getAttribute("src"), A, "本物の PDF は iframe で表示");
tests++;

// 同じ部品で page を中身 HTML の b.pdf へ差し替え: 確認が返る前も後も b の iframe は作られない
props.page = page("b.pdf");
flushSync();
await settle();
assert.equal(target.querySelector("iframe"), null, "差し替え直後に前の結果のまま iframe が残った");
await answer(B, "<!doctype html><script>fetch('http://e')</script>");
flushSync();
await settle();
assert.equal(target.querySelector("iframe"), null, "偽 pdf で iframe が作られた");
assert.ok(target.textContent.includes("PDF として表示できません"), "偽 pdf は外部で開く案内");
tests++;

// 偽 → 本物（c.pdf）でも、確認が返るまで c の iframe は作られない（false の結果が c に持ち越されて案内が出るのも防ぐ）
props.page = page("c.pdf");
flushSync();
assert.equal(target.querySelector("iframe"), null);
assert.ok(!target.textContent.includes("PDF として表示できません"), "前の src の false が持ち越された");
await answer(C, "%PDF-1.4\n");
flushSync();
assert.equal(target.querySelector("iframe")?.getAttribute("src"), C);
tests++;

await settle();
assert.ok(!seen.includes(B), `確かめていない（偽の）src で iframe が作られた: ${JSON.stringify(seen)}`);
assert.deepEqual(seen, [A, C], `作られた iframe: ${JSON.stringify(seen)}`);
tests++;

// 読めなかった（403）PDF は「PDF として表示できません」でなく「読めませんでした（理由）」の案内・iframe は作らない
props.page = page("d.pdf");
flushSync();
const D = assetUrl("/v/d.pdf");
await answer(D, "%PDF-1.4\n", 403);
flushSync();
await settle();
assert.equal(target.querySelector("iframe"), null, "読めなかった pdf で iframe が作られた");
assert.ok(target.textContent.includes("このファイルを読めませんでした") && target.textContent.includes("HTTP 403"), target.textContent);
assert.ok(!target.textContent.includes("PDF として表示できません"), "読めなかったのに PDF でない案内が出た");
assert.ok(target.querySelector(".wk-mv-other button"), "外部で開くボタンが無い");
assert.deepEqual(seen, [A, C]);
tests++;
unmount(cmp);
console.log(`mediaview: ok (${tests} tests)`);
