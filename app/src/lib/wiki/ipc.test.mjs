/* OpenNote のエラーの契約の部品テスト（node app/src/lib/wiki/ipc.test.mjs・実部品 wiki.svelte.ts + jsdom）。
   wiki_gui.rs の wiki_run は CoreError::Wiki を `{"code", "message"}` の JSON 文字列で返す → ipc.ts が WikiError に。画面は code で分岐し、
   core の文言（改名・英訳で変わる）を照合しない = ここの偽物の message は wiki の文言と無関係な文にしてある。確かめること:
   ⓪ WIKI_CODES = core の wiki::codes（crates/openref-core/src/wiki/mod.rs を読んで値を固定 = code の取り違えも黙って壊れる）
   ① parseWikiError: JSON（code + message）・素の文・`{` で始まるだけの文・WikiError（String(e) = message）
   ② rebuilding: 木は状態を読み直す（toast なし）・Tags / Ingest / Chat log の箱は toast しない・欄に出る文は message（JSON ではない）
   ③ bad_query: searchError = null（0 件）・他の code / 素の文は searchError に message
   ④ code の無いエラー（素の文）は従来どおり toast（文はそのまま）
   ⑤ 参照フォルダの追加 / 外す（refdirs.svelte.ts）: 呼ぶ command と引数・wk.refDirs・toast（成功 / 退避 / 取り消し / 失敗。dirs の欠けは []）
   出力 = `wiki ipc: ok (N tests)`（N = 列 0 のブロックの数） */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { setupHarness } from "./test-harness.mjs";

const { calls, handlers, waitFor, sleep } = setupHarness();
let tests = 0;

const { parseWikiError, WikiError, WIKI_CODES } = await import("./ipc.ts");
const S = await import("../state.svelte.ts");
const W = await import("./wiki.svelte.ts");
const R = await import("./refdirs.svelte.ts");

const coded = (code, message) => JSON.stringify({ code, message });
const reject = (e) => () => {
  throw e;
};
const LIST = { pages: [], count: 0 };

// ---- ⓪ 画面の code = core の code（Rust のソースを読んで固定。edit.test ⑱ と同じ「共有の規則を両側で読む」型）----
{
  const rs = readFileSync(new URL("../../../../crates/openref-core/src/wiki/mod.rs", import.meta.url), "utf8");
  const mod = rs.match(/pub mod codes \{[\s\S]*?\n\}/)?.[0];
  assert.ok(mod, "wiki/mod.rs に pub mod codes が無い（場所か名前が変わった = このテストを追随させる）");
  const core = Object.fromEntries([...mod.matchAll(/pub const (\w+): &str = "([^"]*)";/g)].map((m) => [m[1], m[2]]));
  assert.ok(Object.keys(core).length >= 2, `codes を読めない: ${JSON.stringify(core)}`);
  assert.equal(WIKI_CODES.rebuilding, core.REBUILDING, "rebuilding の code が core と違う");
  assert.equal(WIKI_CODES.badQuery, core.BAD_QUERY, "bad_query の code が core と違う");
  for (const v of Object.values(WIKI_CODES)) assert.ok(Object.values(core).includes(v), `core に無い code: ${v}`);
  tests++;
}

// ---- ① parseWikiError ----
{
  assert.deepEqual(parseWikiError(coded("rebuilding", "M 42")), { code: "rebuilding", message: "M 42" });
  assert.deepEqual(parseWikiError("plain"), { code: null, message: "plain" });
  assert.deepEqual(parseWikiError("{not json"), { code: null, message: "{not json" });
  assert.deepEqual(parseWikiError('{"x":1}'), { code: null, message: '{"x":1}' }, "code / message の無い JSON は素の文");
  const e = new WikiError("bad_query", "M");
  assert.equal(String(e), "M", "WikiError の表示は message だけ");
  assert.deepEqual(parseWikiError(e), { code: "bad_query", message: "M" });
  assert.deepEqual(parseWikiError(new Error("x")), { code: null, message: "Error: x" });
  tests++;
}

S.app.wikiOn = true;
const toasts = () => S.app.toast;

// ---- ② rebuilding ----
{
  // 木: 状態を読み直す（wiki_status）・欄の文は message
  S.app.toast = "";
  calls.length = 0;
  handlers.wiki_status = () => new Promise(() => {}); // 読み直しが走ったことだけを見る（先へは進めない）
  handlers.wiki_list = reject(coded("rebuilding", "R 42"));
  await W.loadTree();
  assert.equal(W.wk.treeError, "R 42", "欄に JSON が出た");
  assert.ok(await waitFor(() => calls.some((c) => c.name === "wiki_status")), "rebuilding で状態を読み直さない");
  assert.equal(toasts(), "", `rebuilding で toast が出た: ${toasts()}`);
  // 箱: 木と一緒に読む Tags / Ingest / Chat log（loadAll）が rebuilding = toast なし
  handlers.wiki_list = () => LIST;
  handlers.wiki_tags = reject(coded("rebuilding", "R"));
  handlers.wiki_inbox = reject(coded("rebuilding", "R"));
  calls.length = 0;
  await W.loadAll();
  assert.ok(await waitFor(() => calls.filter((c) => c.name === "wiki_inbox").length === 2 && calls.some((c) => c.name === "wiki_tags")));
  await sleep(20);
  assert.equal(W.wk.treeError, null);
  assert.equal(toasts(), "", `rebuilding の箱で toast が出た: ${toasts()}`);
  tests++;
}

// ---- ③ bad_query ----
{
  const search = async (q, h) => {
    handlers.wiki_search = h;
    W.wk.q = q;
    W.searchSoon(0);
    await waitFor(() => calls.some((c) => c.name === "wiki_search"));
    await waitFor(() => !W.wk.searching);
    await sleep(5);
  };
  W.wk.searchError = "前の値";
  calls.length = 0;
  await search("- \"", reject(coded("bad_query", "Q")));
  assert.equal(W.wk.searchError, null, "bad_query は 0 件（searchError なし）");
  assert.deepEqual(W.wk.results, []);
  calls.length = 0;
  await search("x", reject(coded("rebuilding", "R 7")));
  assert.equal(W.wk.searchError, "R 7", "他の code は message を欄に");
  calls.length = 0;
  await search("y", reject("plain search error"));
  assert.equal(W.wk.searchError, "plain search error", "素の文はそのまま欄に");
  W.wk.q = "";
  W.searchSoon(0);
  tests++;
}

// ---- ④ code の無いエラーは従来どおり toast ----
{
  S.app.toast = "";
  handlers.wiki_list = () => LIST;
  handlers.wiki_tags = reject("plain tags error");
  handlers.wiki_inbox = () => ({ files: [], total: 0 });
  calls.length = 0;
  await W.loadAll();
  assert.ok(await waitFor(() => toasts().includes("plain tags error")), `素の文の失敗が toast に出ない: ${toasts()}`);
  // code 付きでも rebuilding 以外は toast（文は message = JSON ではない）
  S.app.toast = "";
  handlers.wiki_tags = reject(coded("unavailable", "U msg"));
  await W.loadAll();
  assert.ok(await waitFor(() => toasts().includes("U msg")), `unavailable が toast に出ない: ${toasts()}`);
  assert.ok(!toasts().includes('"code"'), `toast に JSON が出た: ${toasts()}`);
  tests++;
}

// ---- ⑤ 参照フォルダの追加 / 外す（refdirs.svelte.ts）----
{
  // 外す: 成功して restart あり = 一覧を置き換え・再起動の案内
  W.wk.refDirs = ["/a", "/b"];
  S.app.toast = "";
  calls.length = 0;
  handlers.wiki_ref_dir_remove = () => ({ dirs: ["/b"], restart: true });
  await R.removeRefDir("/a");
  assert.deepEqual(calls, [{ name: "wiki_ref_dir_remove", args: { path: "/a" } }]);
  assert.deepEqual(W.wk.refDirs, ["/b"]);
  assert.ok(toasts().includes("参照フォルダを外しました") && toasts().includes("再起動"), `restart の案内が出ない: ${toasts()}`);
  // 外す: dirs が欠けた応答 = 空の一覧（restart なし = 案内なし）
  S.app.toast = "";
  handlers.wiki_ref_dir_remove = () => ({ restart: false });
  await R.removeRefDir("/b");
  assert.deepEqual(W.wk.refDirs, []);
  assert.equal(toasts(), "参照フォルダを外しました");
  // 外す: 失敗 = 一覧はそのまま・toast に文
  W.wk.refDirs = ["/c"];
  S.app.toast = "";
  handlers.wiki_ref_dir_remove = reject("remove failed");
  await R.removeRefDir("/c");
  assert.deepEqual(W.wk.refDirs, ["/c"]);
  assert.ok(toasts().startsWith("✗ 参照フォルダを外せません") && toasts().includes("remove failed"), toasts());
  // 追加: 元の設定を退避した（set_aside）= 一覧を置き換え・⚠ の toast 1 つに退避先も
  S.app.toast = "";
  calls.length = 0;
  handlers.wiki_ref_dir_add = () => ({ dirs: ["/c", "/d"], added: "/d", set_aside: "wiki_ref_dirs.bak" });
  await R.addRefDir();
  assert.deepEqual(calls.map((c) => c.name), ["wiki_ref_dir_add"]);
  assert.deepEqual(W.wk.refDirs, ["/c", "/d"]);
  assert.ok(toasts().startsWith("⚠ 参照フォルダを追加しました: /d") && toasts().includes("wiki_ref_dirs.bak"), toasts());
  // 追加: 取り消し（dirs = null・added = null）= 何もしない
  S.app.toast = "";
  handlers.wiki_ref_dir_add = () => ({ dirs: null, added: null });
  await R.addRefDir();
  assert.deepEqual(W.wk.refDirs, ["/c", "/d"]);
  assert.equal(toasts(), "");
  // 追加: 失敗 = 一覧はそのまま・toast に文
  handlers.wiki_ref_dir_add = reject("add failed");
  await R.addRefDir();
  assert.deepEqual(W.wk.refDirs, ["/c", "/d"]);
  assert.ok(toasts().startsWith("✗ 参照フォルダを追加できません") && toasts().includes("add failed"), toasts());
  tests++;
}

console.log(`wiki ipc: ok (${tests} tests)`);
process.exit(0);
