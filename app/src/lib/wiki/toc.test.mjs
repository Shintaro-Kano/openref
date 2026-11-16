/* 右ペインの目次の開閉を覚える試験（node app/src/lib/wiki/toc.test.mjs・実部品 wiki.svelte.ts / WikiInfo + jsdom・invoke は差し替え）。
   確かめること: 既定は開いている（鍵が無い = 前の版が書いた設定）・畳むと ui_wiki_toc = "0" を書き、今ある鍵（ui_wiki_info・ui_wiki_heading_colors）は書かない・
   起動し直しても畳んだまま（子のプロセス = 設定を読む関数の 1 回きりの門ごと新しい）・前の版の読み方との往復（前の版は ui_wiki_toc を知らない =
   今ある鍵の値が同じなら前の版の画面は同じ）・壊れた値は開いている。
   起動し直しは子のプロセス（OPENREF_TOC_PHASE = 2・設定は OPENREF_TOC_PREFS の JSON）で見る */
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { setupHarness } from "./test-harness.mjs";

const { calls, handlers, sleep, waitFor } = setupHarness();
const phase = process.env.OPENREF_TOC_PHASE ?? "1";
const prefs = JSON.parse(process.env.OPENREF_TOC_PREFS ?? "{}");
handlers.get_ui_pref = (a) => prefs[a.key] ?? null;
handlers.set_ui_pref = (a) => {
  prefs[a.key] = a.value;
  return null;
};
handlers.wiki_settings = () => ({ enabled: true, root: "/v" });
handlers.wiki_status = () => ({ enabled: true, root: "/v" });
handlers.wiki_backlinks = () => ({ total: 0, pages: [] });

const { mount, flushSync } = await import("svelte");
const S = await import("../state.svelte.ts");
const W = await import("./wiki.svelte.ts");
const { default: WikiInfo } = await import("./WikiInfo.svelte");
const settle = async () => {
  for (let i = 0; i < 4; i++) {
    await sleep(0);
    flushSync();
  }
};

/** 前の版（main = 48d3272 まで）の loadWikiPrefs の読み方（そのままの式）。前の版は ui_wiki_toc を読まない */
const oldRead = (p) => ({ infoOpen: p.ui_wiki_info !== "0", headColors: p.ui_wiki_heading_colors !== "0" });

await W.loadWikiEnabled();
assert.ok(await waitFor(() => W.wk.prefsReady, 2000), "表示設定を読み終えない");
W.wk.page = { path: "a.md", abs_path: "/v/a.md", kind: "md", title: "a", size: 1, mtime: 0, content: "", sha256: "A", truncated: false, links: [] };
W.wk.headings = [
  { level: 1, text: "一", id: "一" },
  { level: 2, text: "二", id: "二" },
  { level: 3, text: "三", id: "三" },
];
const target = document.createElement("div");
document.body.append(target);
mount(WikiInfo, { target });
await settle();
const head = () => target.querySelector(".wk-sec-toggle");
const rows = () => target.querySelectorAll(".wk-toc").length;
const wrote = (k) => calls.filter((c) => c.name === "set_ui_pref" && c.args.key === k).length;

if (phase === "1") {
  // 前の版が書いた設定（ui_wiki_toc が無い）= 開いている・今ある鍵は前の版と同じに読む
  assert.equal(W.wk.tocOpen, true, "既定（鍵が無い）が開いていない");
  assert.equal(head().getAttribute("aria-expanded"), "true");
  assert.equal(rows(), 3, "開いているのに行が無い");
  // 畳む = ui_wiki_toc = "0" だけを書く（今ある鍵の値を変えない）
  prefs.ui_wiki_info = "0"; // 前の版が書いた値（右ペインを隠した）= 新しい版の目次の開閉はこれを触らない
  prefs.ui_wiki_heading_colors = "1";
  const before = { ...prefs };
  head().click();
  await settle();
  assert.equal(W.wk.tocOpen, false, "畳めない");
  assert.equal(prefs.ui_wiki_toc, "0", "畳んだ状態を ui_wiki_toc に書かない");
  assert.equal(wrote("ui_wiki_info") + wrote("ui_wiki_heading_colors"), 0, "目次の開閉で今ある鍵を書いた");
  const { ui_wiki_toc: _, ...rest } = prefs;
  assert.deepEqual(rest, before, "目次の開閉で ui_wiki_toc 以外の設定が変わった");
  // 往復: 前の版がこの設定を読んでも、前の版の画面は畳む前と同じ（知らない鍵は読まない・今ある鍵の値は同じ）
  assert.deepEqual(oldRead(prefs), oldRead(before), "前の版の読み方で、目次の開閉の前後に差が出た");
  // 起動し直す（子のプロセス）= 畳んだまま
  const child = spawnSync(process.execPath, [fileURLToPath(import.meta.url)], {
    env: { ...process.env, OPENREF_TOC_PHASE: "2", OPENREF_TOC_PREFS: JSON.stringify(prefs), OPENREF_TOC_WANT: "closed" },
    encoding: "utf8",
  });
  assert.equal(child.status, 0, `起動し直した後の試験が落ちた:\n${child.stdout}\n${child.stderr}`);
  // 開き直して起動し直す = 開いている
  head().click();
  await settle();
  assert.equal(prefs.ui_wiki_toc, "1");
  const child2 = spawnSync(process.execPath, [fileURLToPath(import.meta.url)], {
    env: { ...process.env, OPENREF_TOC_PHASE: "2", OPENREF_TOC_PREFS: JSON.stringify(prefs), OPENREF_TOC_WANT: "open" },
    encoding: "utf8",
  });
  assert.equal(child2.status, 0, `開き直して起動し直した後の試験が落ちた:\n${child2.stdout}\n${child2.stderr}`);
  // 前の版が書いた設定だけ（ui_wiki_toc なし）で起動 = 開いている・前の版の鍵は前の版と同じ意味
  const child3 = spawnSync(process.execPath, [fileURLToPath(import.meta.url)], {
    env: { ...process.env, OPENREF_TOC_PHASE: "2", OPENREF_TOC_PREFS: JSON.stringify({ ui_wiki_info: "0", ui_wiki_heading_colors: "0" }), OPENREF_TOC_WANT: "open-old" },
    encoding: "utf8",
  });
  assert.equal(child3.status, 0, `前の版の設定で起動した後の試験が落ちた:\n${child3.stdout}\n${child3.stderr}`);
  // 壊れた値 = 開いている（"0" だけが畳む = ui_wiki_info と同じ読み方）
  const child4 = spawnSync(process.execPath, [fileURLToPath(import.meta.url)], {
    env: { ...process.env, OPENREF_TOC_PHASE: "2", OPENREF_TOC_PREFS: JSON.stringify({ ui_wiki_toc: "{broken" }), OPENREF_TOC_WANT: "open" },
    encoding: "utf8",
  });
  assert.equal(child4.status, 0, `壊れた値で起動した後の試験が落ちた:\n${child4.stdout}\n${child4.stderr}`);
  console.log("wiki toc: ok (1 test)");
} else {
  const want = process.env.OPENREF_TOC_WANT;
  if (want === "closed") {
    assert.equal(W.wk.tocOpen, false, "起動し直したら畳んだ状態を忘れた");
    assert.equal(head().getAttribute("aria-expanded"), "false");
    assert.equal(rows(), 0, "起動し直した後、畳んでいるのに行を描いた");
    assert.match(head().textContent, /目次\s*3/, "畳んでいる間に見出しの件数が無い");
  } else {
    assert.equal(W.wk.tocOpen, true, `起動し直したら開いていない（${want}）`);
    assert.equal(rows(), 3);
  }
  if (want === "open-old") assert.deepEqual([W.wk.infoOpen, W.wk.headColors], [false, false], "前の版の鍵の意味が変わった");
  assert.equal(calls.filter((c) => c.name === "set_ui_pref").length, 0, "起動しただけで設定を書いた");
  void S;
}
process.exit(0);
