/* OpenNote の画面側の読み直しの計測（test-all には入れない・手で走らせる）。
   使い方: node app/src/lib/wiki/bench-reload.mjs [<checkout>]   （checkout = 比べたい別の作業木。省略 = この repo）
   例: 新旧の比較 = `node app/src/lib/wiki/bench-reload.mjs` と `node app/src/lib/wiki/bench-reload.mjs /path/to/old-checkout`（node_modules は app/ に要る）
   古い checkout = この bench で読むと chat.ts の import で落ちる → それぞれの checkout の bench を引数なしで走らせて比べる
   計るもの（偽のワークスペース = 自分で生成した約 1 万ページ・wiki_list は偽物 = IPC の時間は含まない）:
   1) loadTree 1 回（wiki_list の応答 → wk.pages の置換 → 木の派生（WikiSidebar の tree / rows）の組み直しまで）
   2) wk.pages を 1 回読み切る（深い proxy 越し / raw）
   3) wikiTabTitle ×200（タブの描画ごとの題名）
   4) chatLinks ×50 ブロック（応答のブロックごとの [[…]] の解決）
   5) wiki-changed 1 回あたりの IPC（ページ 1 件の上書き・ログの追記）。古い checkout（payload が string[]）にも対応
   ShadowPage の描画の時間（performance.measure "wk-render <path>"）は実機の Inspector で `window.__wkMeasure = true` を立ててから読む。 */
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url)); // <checkout>/app/src/lib/wiki
const CK = process.argv[2] ? resolve(process.argv[2]) : resolve(HERE, "../../../..");
const { setupHarness } = await import(`${CK}/app/src/lib/wiki/test-harness.mjs`);
const { calls, handlers, emit, sleep } = setupHarness();
const { mount, flushSync } = await import("svelte");
const S = await import(`${CK}/app/src/lib/state.svelte.ts`);
const W = await import(`${CK}/app/src/lib/wiki/wiki.svelte.ts`);
const C = await import(`${CK}/app/src/lib/chat/chat.ts`);
const { default: WikiSidebar } = await import(`${CK}/app/src/lib/wiki/WikiSidebar.svelte`);

// 偽のワークスペース: 11 フォルダ × 2 段 × ページ（md 6 割・jpg 3.5 割・他）≈ 10,900 件（大きめの実用のワークスペースに近い大きさ）
const base = [];
{
  const dirs = ["papers", "cases", "notes", "journals", "reports", "neuro", "web-clips", "records", "books", "inbox", "chat-log"];
  let n = 0;
  for (const d of dirs) {
    for (let sub = 0; sub < 9; sub++) {
      for (let i = 0; i < 110; i++) {
        n++;
        const ext = n % 20 < 12 ? "md" : n % 20 < 19 ? "jpg" : "json";
        const kind = ext === "md" ? "md" : ext === "jpg" ? "media" : "other";
        base.push({ path: `${d}/sub-${sub}/page-${i}.${ext}`, kind, title: kind === "md" ? `Page ${d} ${sub}-${i}` : "", tags: [], items: [], mtime: 1_700_000_000 + n, size: 1000 + n });
      }
    }
  }
  base.push({ path: "index.md", kind: "md", title: "Index", tags: [], items: [], mtime: 1, size: 1 });
  base.sort((a, b) => (a.path < b.path ? -1 : 1));
}
const fresh = () => base.map((p) => ({ ...p, tags: [...p.tags], items: [...p.items] })); // wiki_list は毎回新しい配列 / object
console.log(`checkout=${CK}\npages=${base.length}`);

handlers.wiki_list = () => ({ total: base.length, count: base.length, pages: fresh() });
handlers.wiki_tags = () => ({ tags: [], total: 0 });
handlers.wiki_inbox = () => ({ files: [], total: 0 });
handlers.wiki_backlinks = () => ({ total: 0, pages: [] });
S.app.wikiOn = true;
W.wk.status = { enabled: true, root: "/v" };
const target = document.createElement("div");
document.body.append(target);
mount(WikiSidebar, { target });
flushSync();

const med = (a) => [...a].sort((x, y) => x - y)[Math.floor(a.length / 2)];
const tl = [];
for (let i = 0; i < 7; i++) {
  const t0 = performance.now();
  await W.loadTree();
  flushSync();
  tl.push(performance.now() - t0);
}
console.log(`1) loadTree + tree derived: median ${med(tl).toFixed(1)} ms (runs: ${tl.map((x) => x.toFixed(0)).join(" ")})`);
const rd = [];
for (let i = 0; i < 7; i++) {
  await W.loadTree();
  const t0 = performance.now();
  let n = 0;
  for (const p of W.wk.pages) n += p.path.length + p.title.length;
  rd.push(performance.now() - t0);
}
console.log(`2) read all pages once: median ${med(rd).toFixed(2)} ms`);
await W.loadTree();
const tabs = base.filter((p) => p.kind === "md").filter((_, i) => i % 60 === 0).slice(0, 100);
const tt = [];
for (let i = 0; i < 5; i++) {
  const t0 = performance.now();
  for (const p of tabs) for (let k = 0; k < 2; k++) W.wikiTabTitle({ path: p.path, scrollTop: 0, editing: false });
  tt.push(performance.now() - t0);
}
console.log(`3) wikiTabTitle x200: median ${med(tt).toFixed(2)} ms`);
const names = tabs.slice(0, 10).map((p) => `[[${p.title}]]`).join(" ");
const cl = [];
for (let i = 0; i < 5; i++) {
  const t0 = performance.now();
  for (let k = 0; k < 50; k++) C.chatLinks(`本文 ${names}`, W.wk.pages);
  cl.push(performance.now() - t0);
}
console.log(`4) chatLinks x50 blocks: median ${med(cl).toFixed(2)} ms`);
const off = W.initWiki();
await sleep(10);
const typed = typeof W.reloadPlan === "function";
const known = base[0].path; // 既知のページ（新しい形では modified = 木を読まない）
const ipc = async (payload) => {
  calls.length = 0;
  emit("wiki-changed", payload);
  await sleep(1200);
  return `IPC ${calls.length}: ${calls.map((c) => c.name + (c.args?.dir ? `(${c.args.dir})` : "")).join(", ")}`;
};
console.log(`5) wiki-changed (modified 1 known page) → ${await ipc(typed ? { all: false, added: [], removed: [], modified: [known] } : [known])}`);
console.log(`   wiki-changed (chat-log append, known log) → ${await ipc(typed ? { all: false, added: [], removed: [], modified: ["chat-log/sub-0/page-0.md"] } : ["chat-log/sub-0/page-0.md"])}`);
console.log(`   wiki-changed (new page) → ${await ipc(typed ? { all: false, added: ["new.md"], removed: [], modified: [] } : ["new.md"])}`);
off();
process.exit(0);
