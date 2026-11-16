/* OpenNote の「書く」道具の部品テスト（node app/src/lib/wiki/write.test.mjs・Svelte 5 の実部品 + 実 CodeMirror + jsdom・invoke は差し替え）。
   確かめること:
   ① 文献リンクの形（editor/cite.ts）: `[選択](openref://open?key=KEY)`・選択なし `[](…)`・`\` `[` `]` の escape・ビルドの正規表現に合う・英数字でない key は入れない
   ② 貼り付けの判定の順（cm.ts）: 選択の上の URL = 普通のリンク（lang-markdown の pasteURLAsLink を壊さない・DOI の URL も）→ 画像 → 選択なしの DOI / PubMed の URL = メニュー → 既定
      置けない所（citeBlocked）: 塊の行（端と caret 0 を含む）・行の中の構文の中・`[[…]]`・脚注 / callout・表のセルの外 / 区切りの行 / セルをまたぐ選択。置ける所の確かめも並べる
   ③ 貼った URL のメニュー: 蔵書にあれば置き換え・無ければ登録して置き換え・待つ間の編集に追随・URL を書き換えたら置き換えない・打ち続け / Esc / [URL のまま] で消える・撤回の印を知らせる
   ④ 蔵書の検索（cite.svelte.ts）: 語 = list_items（search + metadataOnly・上位 20）・4 桁の年は語から外して year で絞る・DOI / PMID / URL は wiki_cite_lookup の 1 件・識別子でなければ語として
   ⑤ 引用の窓（実部品 WikiCite + WikiPage）: [文献] / ⌘⌥K（e.code）/「/」で開く・↑↓ Enter で入れる・Esc で閉じる・IME の Enter で入れない・編集から離れたら入れない・登録は離れたら本文に入れない
   ⑥ ⑦ 字数（count.ts・実部品 WikiPage）: 本文 = 見える文字・選択 = そのまま（空白と改行を除く）・表示中も編集中も・ページの外の選択は数えない
   ⑧ 見出しで畳む（foldGutter + foldKeymap）: 溝・鍵で節が畳まれ開く・本文は変わらない
   ⑨ 表の中の Tab / Shift+Tab（editor/table.ts）: 次 / 前のセル・空のセル・区切りの行・行を足す・表の外は字下げ・IME の Tab・snippet の Tab が先
   ⑩ 覆いの下でページのキーが裏に効かない（実部品 WikiPage・ページを開いた状態）: 参照エディタ・取込・チャットの同意・救出・本体の窓 ×
      ⌘E / ⌘F / ⌘⌥K / 検索中の Esc = 何も起きない（キーも奪わない）・⌘S は覆いの下でも保存する（今のまま）・覆いが無ければ全部が今までどおり効く
   ⑪ 同じキーの処理の途中で、先に走る受け手（App の Esc の連鎖 = closeImport・窓自身の Esc）が覆いを閉じても、裏のページは動かない
      （キーが来た時点の覆いを capture の段で覚える = ShelfViewer の coveredAtKey と同じ型・受け手の順序ごと再現）
   出力 = `wiki write: ok (N tests)`（N = 列 0 のブロックの数） */
import assert from "node:assert/strict";
import { setupHarness } from "./test-harness.mjs";

const { calls, handlers, sleep, waitFor } = setupHarness();
let tests = 0;

const { mount, unmount, flushSync } = await import("svelte");
const E = await import("./edit.svelte.ts");
const { createEditor } = await import("./editor/cm.ts");
const C = await import("./editor/cite.ts");
const CS = await import("./cite.svelte.ts");
const S = await import("../state.svelte.ts");
const W = await import("./wiki.svelte.ts");
const notices = [];
E.setNotice((m) => notices.push(m));
E.cfg.delay = 40;
E.cfg.previewDelay = 5;
CS.citeCfg.delay = 5;

const BUILD = /^openref:\/\/open\?key=([A-Za-z0-9]+)$/; // 文献リンクの形 = openref://open?key=<英数字>
const page = (path, content, sha) => ({ path, abs_path: "/v/" + path, kind: "md", title: path, size: content.length, mtime: 0, content, sha256: sha, truncated: false, links: [] });
const sel = (h, from, to = from) => h.view.dispatch({ selection: { anchor: from, head: to } });
const text = (h) => h.view.state.doc.toString();
/** 貼り付け（jsdom に ClipboardEvent の clipboardData は無い = 差し込む）。files = 画像 */
function paste(h, plain, files = []) {
  const items = files.map((f) => ({ kind: "file", type: f.type, getAsFile: () => f }));
  const dt = { getData: (t) => (t === "text/plain" ? plain : ""), items, files, types: ["text/plain"] };
  const e = new Event("paste", { bubbles: true, cancelable: true });
  Object.defineProperty(e, "clipboardData", { value: dt });
  h.view.contentDOM.dispatchEvent(e);
  return e;
}
const pop = (h) => h.view.dom.querySelector(".wk-cite-pop");
const popButtons = (h) => [...(pop(h)?.querySelectorAll("button") ?? [])].map((b) => b.textContent);
const click = (h, label) => [...pop(h).querySelectorAll("button")].find((b) => b.textContent === label).click();

// ---- ① 文献リンクの形 ----
{
  assert.equal(C.citeLink("ABCD1234"), "[](openref://open?key=ABCD1234)");
  assert.equal(C.citeLink("K1", "a[1]\\b"), "[a\\[1\\]\\\\b](openref://open?key=K1)");
  assert.match(C.citeLink("K1").match(/\]\((.*)\)$/)[1], BUILD);
  assert.throws(() => C.citeLink("../x"), /bad item key/);
  assert.throws(() => C.citeLink(""), /bad item key/);
  const h = createEditor(document.createElement("div"), { doc: "肝硬変は[既存]です", mode: "md", onChange: () => {} });
  sel(h, 0, 3);
  h.cite("K1");
  assert.equal(text(h), "[肝硬変](openref://open?key=K1)は[既存]です", "選択がリンクの文字にならない");
  assert.equal(h.view.state.selection.main.head, "[肝硬変](openref://open?key=K1)".length, "caret がリンクの後ろに無い");
  sel(h, text(h).length);
  h.cite("K2");
  assert.ok(text(h).endsWith("です[](openref://open?key=K2)"), "選択なしで番号だけのリンクにならない");
  sel(h, text(h).indexOf("[既存]"), text(h).indexOf("[既存]") + 4);
  assert.equal(h.selection(), "[既存]");
  h.cite("K3");
  assert.ok(text(h).includes("[\\[既存\\]](openref://open?key=K3)"), "括弧を escape しない");
  h.destroy();
  tests++;
}

// ---- ② 貼り付けの判定の順 ----
{
  const images = [];
  const mk = (doc) => createEditor(document.createElement("div"), {
    doc, mode: "md", onChange: () => {}, onPasteImages: (f) => images.push(...f),
    cite: { lookup: async () => null, register: async () => null },
  });
  // 選択の上の普通の URL = 今どおり [選択](URL)（lang-markdown の pasteURLAsLink）
  let h = mk("see here");
  sel(h, 4, 8);
  assert.equal(paste(h, "https://example.com/a").defaultPrevented, true);
  assert.equal(text(h), "see [here](https://example.com/a)", "選択の上に URL を貼ってもリンクにならない（既存の動きの回帰）");
  assert.equal(pop(h), null);
  // 選択の上の DOI の URL = 普通のリンク（引用への自動の変換はしない）
  h.setText("see here");
  sel(h, 4, 8);
  paste(h, "https://doi.org/10.1000/abc");
  assert.equal(text(h), "see [here](https://doi.org/10.1000/abc)", "選択の上の DOI の URL が普通のリンクにならない");
  assert.equal(pop(h), null, "選択の上の DOI の URL でメニューを出した");
  // 画像は DOI の URL より先
  h.setText("x");
  sel(h, 1);
  const img = new File([new Uint8Array([1])], "image.png", { type: "image/png" });
  paste(h, "https://doi.org/10.1000/abc", [img]);
  assert.equal(images.length, 1, "画像を先に扱わない");
  assert.equal(text(h), "x");
  // 選択なしの DOI の URL = URL が入り、メニュー
  sel(h, 1);
  assert.equal(paste(h, " https://doi.org/10.1000/abc \n").defaultPrevented, true);
  assert.equal(text(h), "xhttps://doi.org/10.1000/abc");
  assert.deepEqual(popButtons(h), ["文献リンクにする", "URL のまま"], "メニューが出ない");
  // PubMed の URL も
  h.setText("");
  paste(h, "https://pubmed.ncbi.nlm.nih.gov/123456/");
  assert.ok(pop(h), "PubMed の URL でメニューが出ない");
  // DOI でも PubMed でもない URL・URL と文の混ざり・コードの中・frontmatter の中 = 既定（メニューなし）
  for (const [doc, at, clip] of [
    ["", 0, "https://example.com/10.1000/abc"],
    ["", 0, "see https://doi.org/10.1000/abc"],
    ["```\n\n```\n", 4, "https://doi.org/10.1000/abc"],
    ["---\ntitle: a\n\n---\n", 15, "https://doi.org/10.1000/abc"],
  ]) {
    h.setText(doc);
    sel(h, at);
    paste(h, clip);
    assert.equal(pop(h), null, `メニューを出した: ${JSON.stringify([doc, clip])}`);
  }
  // html のエディタ・cite を渡さないエディタ = メニューなし
  const html = createEditor(document.createElement("div"), { doc: "", mode: "html", onChange: () => {}, cite: { lookup: async () => null, register: async () => null } });
  paste(html, "https://doi.org/10.1000/abc");
  assert.equal(pop(html), null);
  const bare = createEditor(document.createElement("div"), { doc: "", mode: "md", onChange: () => {} });
  paste(bare, "https://doi.org/10.1000/abc");
  assert.equal(pop(bare), null);
  assert.equal(C.citeUrlOf("https://dx.doi.org/10.1000/a?x=1"), "https://dx.doi.org/10.1000/a?x=1");
  assert.equal(C.citeUrlOf("http://pubmed.ncbi.nlm.nih.gov/1/?a#b"), "http://pubmed.ncbi.nlm.nih.gov/1/?a#b");
  assert.equal(C.citeUrlOf("https://doi.org/abc"), null);
  assert.equal(C.citeUrlOf("https://pubmed.ncbi.nlm.nih.gov/?term=x"), null);
  // リンク先の位置・リンクの文字の中 = メニューを出さない（押すと `[t]([](…))` になる）。リンクの直後は出す
  for (const [doc, at, want] of [
    ["[text](", 7, false],
    ["[text]()", 7, false],
    ["[fo o](https://x.org)", 3, false],
    ["<", 1, false],
    ["[a](https://x.org)", 18, true],
    ["`c` ", 4, true],
    // 塊の端（閉じの ``` の直後・字下げのコードの最終行の末尾・HTML の塊の末尾・frontmatter の閉じの直後・caret 0）
    ["```\nx\n```\n後", 9, false],
    ["    code\n\nend", 8, false],
    ["<div>\nx\n</div>\n\n本文", 14, false],
    ["---\ntitle: a\n---\n本文", 16, false],
    ["---\ntitle: a\n---\n本文", 0, false],
    ["---\ntitle: a\n---\n本文", 17, true],
    // `[[…]]` の中・脚注・callout
    ["a [[ページ]] b", 3, false],
    ["a [[ページ]] b", 9, true],
    // 表の本文の行の末尾（`|` の後）・見出しの行の末尾 = 出さない。最後のセルの終わり = 出す
    ["| シナリオ | 判定者 A |\n|---|---|\n| 悲観 | 12% |\n| 楽観 | 23% |\n", 39, false],
    ["| シナリオ | 判定者 A |\n|---|---|\n| 悲観 | 12% |\n| 楽観 | 23% |\n", 16, false],
    ["| シナリオ | 判定者 A |\n|---|---|\n| 悲観 | 12% |\n| 楽観 | 23% |\n", 37, true],
  ]) {
    h.setText(doc);
    sel(h, at);
    paste(h, "https://doi.org/10.1000/abc");
    assert.equal(!!pop(h), want, `メニュー ${want ? "が出ない" : "を出した"}: ${JSON.stringify([doc, at])}`);
  }
  // 置けない所・包めない選択には入れずに理由（本文は変えない）
  for (const [doc, a, b, why] of [
    ["段落 1\n\n段落 2", 2, 9, "行をまたぐ"],
    ["行 1\n行 2", 0, 5, "行をまたぐ"],
    ["```\nx\n```\n", 4, 4, "コード"],
    ["`code` x", 2, 4, "コード"],
    ["---\ntitle: a\n---\n本文", 6, 6, "frontmatter"],
    ["[既存のリンク](https://x.org)", 1, 3, "リンクの中"],
    ["[既存のリンク](https://x.org)", 0, 23, "リンクの中"],
    // 塊の端と caret 0（端を「外」と数えない）
    ["```\nx\n```\n後", 9, 9, "コード"],
    ["```\nx\n```", 0, 0, "コード"],
    ["    code\n\nend", 8, 8, "コード"],
    ["    code\n\nend", 0, 0, "コード"],
    ["<div>\nx\n</div>\n\n本文", 14, 14, "コード"],
    ["---\ntitle: a\n---\n本文", 0, 0, "frontmatter"],
    ["---\ntitle: a\n---\n本文", 16, 16, "frontmatter"],
    // `[[…]]`（[[ の間・中・含む選択）・埋め込み・脚注・callout・定義のある [foo]
    ["a [[ページ]] b", 3, 3, "リンクの中"],
    ["a [[ページ]] b", 5, 5, "リンクの中"],
    ["a [[ページ]] b", 0, 11, "リンクの中"],
    ["x ![[img.png]] y", 3, 3, "リンクの中"],
    ["本文[^1] 続き", 3, 3, "リンクの中"],
    ["> [!note] x", 4, 4, "リンクの中"],
    ["[foo] と\n\n[foo]: https://x.org", 2, 2, "リンクの中"],
    // 閉じていない自動リンクの中
    ["<https://x.org/a", 16, 16, "リンクの中"],
    // 表の行の最初と最後の `|` の外（本文の行の末尾・見出しの行の末尾・行の頭）・区切りの行・セルをまたぐ選択・`|` を含む選択
    ["T", "12% |", 5, 0, "セルの外"],
    ["T", "判定者 A |", 7, 0, "セルの外"],
    ["T", "| 悲観", 0, 0, "セルの外"],
    ["T", "|---|", 2, 0, "セルの外"],
    ["T", "悲観", 0, 7, "セルをまたぐ"],
    ["> | a |\n> | - |\n> | 1 |\n", "> | 1", 1, 0, "セルの外"],
    ["| a \\| b |\n| - |\n", "a", 0, 4, "「|」"],
    // 脚注の定義の名前の中
    ["本文[^1]\n\n[^1]: 注の本文\n", "[^1]:", 2, 0, "リンクの中"],
  ].map((x) => (x.length === 5 ? (() => {
    const doc = x[0] === "T" ? "| シナリオ | 判定者 A |\n|---|---|\n| 悲観 | 12% |\n| 楽観 | 23% |\n" : x[0];
    const at = doc.indexOf(x[1]) + x[2];
    return [doc, at, at + x[3], x[4]];
  })() : x))) {
    h.setText(doc);
    sel(h, a, b);
    const r = h.cite("K9");
    assert.ok(r && r.includes(why), `入れてしまう / 理由が違う: ${JSON.stringify([doc, a, b, r])}`);
    assert.equal(text(h), doc, "置けないのに本文を変えた");
    assert.equal(h.citeCheck(), r);
  }
  // 置ける所: 塊の次の行・`[[…]]` の直後・定義の無い [foo]・`p<` の直後（窓）・脚注の定義の本文・表のセルの中（最後のセルの終わり・セルの中の 1 語）
  for (const [doc, a, b, want] of [
    ["```\nx\n```\n後", 10, 10, "```\nx\n```\n[](openref://open?key=K9)後"],
    ["---\ntitle: a\n---\n本文", 17, 17, "---\ntitle: a\n---\n[](openref://open?key=K9)本文"],
    ["a [[ページ]] b", 9, 9, "a [[ページ]][](openref://open?key=K9) b"],
    ["[foo] と", 0, 5, "[\\[foo\\]](openref://open?key=K9) と"],
    ["p< q", 2, 2, "p<[](openref://open?key=K9) q"],
    // インラインのコードと素の URL を含む選択は包める（コードの中は escape しない）
    ["a `code[0]` b", 0, 13, "[a `code[0]` b](openref://open?key=K9)"],
    ["see https://x.org/a here", 0, 24, "[see https://x.org/a here](openref://open?key=K9)"],
    ["本文[^1]\n\n[^1]: 注の本文\n", 16, 16, "本文[^1]\n\n[^1]: 注の[](openref://open?key=K9)本文\n"],
    ["| シナリオ | 判定者 A |\n|---|---|\n| 悲観 | 12% |\n| 楽観 | 23% |\n", 37, 37, "| シナリオ | 判定者 A |\n|---|---|\n| 悲観 | 12%[](openref://open?key=K9) |\n| 楽観 | 23% |\n"],
    ["| シナリオ | 判定者 A |\n|---|---|\n| 悲観 | 12% |\n| 楽観 | 23% |\n", 29, 31, "| シナリオ | 判定者 A |\n|---|---|\n| [悲観](openref://open?key=K9) | 12% |\n| 楽観 | 23% |\n"],
  ]) {
    h.setText(doc);
    sel(h, a, b);
    assert.equal(h.citeCheck(), null, `置けるのに拒んだ: ${JSON.stringify([doc, a, b, h.citeCheck()])}`);
    assert.equal(h.cite("K9"), null);
    assert.equal(text(h), want);
  }
  h.setText("[a](https://x.org) と `c`");
  sel(h, 18);
  assert.equal(h.citeCheck(), null, "リンクの直後に置けない");
  assert.equal(h.cite("K9"), null);
  assert.ok(text(h).startsWith("[a](https://x.org)[](openref://open?key=K9)"));
  for (const x of [h, html, bare]) x.destroy();
  tests++;
}

// ---- ③ 貼った URL のメニュー ----
{
  const said = [];
  let lookup = async () => null;
  let register = async () => null;
  // 文書に付けた親（付いていないと jsdom では view.focus() が何もしない = focus を奪う変異を見逃す）
  const host3 = document.createElement("div");
  document.body.append(host3);
  const h = createEditor(host3, {
    doc: "本文。", mode: "md", onChange: () => {},
    cite: { lookup: (u) => lookup(u), register: (u) => register(u), notice: (m) => said.push(m) },
  });
  const URL1 = "https://doi.org/10.1000/abc";
  const start = (doc = "本文。") => {
    h.setText(doc);
    sel(h, doc.length);
    paste(h, URL1);
  };
  // 蔵書にある = 置き換え（撤回の印は知らせる）
  let asked = null;
  lookup = async (u) => ((asked = u), { key: "HIT1", retracted: true });
  start();
  click(h, "文献リンクにする");
  assert.ok(await waitFor(() => text(h) === "本文。[](openref://open?key=HIT1)"), `置き換えない: ${text(h)}`);
  assert.equal(asked, URL1);
  assert.equal(pop(h), null, "置き換えた後もメニューが残る");
  assert.ok(said.some((m) => m.includes("撤回")), "撤回の印を知らせない");
  // 蔵書に無い = 登録のボタン → 登録して置き換え。待つ間の前の編集に追随
  lookup = async () => null;
  let release;
  register = (u) => new Promise((r) => (release = () => r({ key: "NEW1", retracted: false }), asked = u));
  start("A");
  click(h, "文献リンクにする");
  assert.ok(await waitFor(() => popButtons(h).includes("LIBRARY に登録して文献リンクに")), "蔵書に無いのに登録のボタンが出ない");
  assert.ok(pop(h).textContent.includes("蔵書にありません"));
  click(h, "LIBRARY に登録して文献リンクに");
  assert.ok(await waitFor(() => !!release), "登録を呼ばない");
  assert.ok(pop(h).textContent.includes("登録中"), "待つ間の表示が無い");
  h.view.dispatch({ changes: { from: 0, insert: "前に打った。" }, userEvent: "input" }); // 待つ間の編集（URL の前）
  h.view.dispatch({ changes: { from: h.view.state.doc.length, insert: "後ろ" }, userEvent: "input" });
  release();
  assert.ok(await waitFor(() => text(h) === "前に打った。A[](openref://open?key=NEW1)後ろ"), `待つ間の編集に追随しない: ${text(h)}`);
  // 待つ間に URL を書き換えた = 置き換えない（登録はできた = 知らせる）
  release = null;
  start("B");
  click(h, "文献リンクにする");
  await waitFor(() => popButtons(h).includes("LIBRARY に登録して文献リンクに"));
  click(h, "LIBRARY に登録して文献リンクに");
  await waitFor(() => !!release);
  const at = text(h).indexOf("abc");
  h.view.dispatch({ changes: { from: at, to: at + 3, insert: "xyz" }, userEvent: "input" });
  said.length = 0;
  release();
  await sleep(20);
  assert.equal(text(h), "Bhttps://doi.org/10.1000/xyz", "書き換えた URL を置き換えた");
  assert.equal(pop(h), null);
  assert.ok(said.some((m) => m.includes("置き換えませんでした")), "置き換えなかったことを知らせない");
  // 登録が null（呼び出し側が諦めて知らせた）= 何もしない
  register = async () => null;
  start("C");
  click(h, "文献リンクにする");
  await waitFor(() => popButtons(h).includes("LIBRARY に登録して文献リンクに"));
  click(h, "LIBRARY に登録して文献リンクに");
  await waitFor(() => !pop(h));
  assert.equal(text(h), "C" + URL1);
  // 失敗 = 理由と [閉じる]
  lookup = async () => {
    throw new Error("DOI / PMID として読めません");
  };
  start("D");
  click(h, "文献リンクにする");
  assert.ok(await waitFor(() => popButtons(h).includes("閉じる")), "失敗の表示が無い");
  assert.ok(pop(h).textContent.includes("読めません"));
  assert.ok(!pop(h).textContent.includes("Error"), `"Error:" の前置きを出した: ${pop(h).textContent}`);
  click(h, "閉じる");
  assert.equal(pop(h), null);
  assert.equal(text(h), "D" + URL1);
  // [URL のまま]・打ち続け・Esc = メニューが消える（本文は URL のまま）
  start("E");
  click(h, "URL のまま");
  assert.equal(pop(h), null);
  start("F");
  h.view.dispatch({ changes: { from: h.view.state.doc.length, insert: " 続き" }, userEvent: "input" });
  assert.equal(pop(h), null, "打ち続けてもメニューが残る");
  start("G");
  h.view.contentDOM.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", keyCode: 27, bubbles: true, cancelable: true }));
  assert.equal(pop(h), null, "Esc で消えない");
  assert.equal(text(h), "G" + URL1);
  // 待つ間に別の所で打っていた = 答えが来ても caret を飛ばさない・focus を奪わない。URL の直後に居たなら caret はリンクの後ろへ
  lookup = () => new Promise((r) => (release = () => r({ key: "MAP1", retracted: false })));
  release = null;
  start("I");
  click(h, "文献リンクにする");
  await waitFor(() => !!release);
  sel(h, 0);
  h.view.dispatch({ changes: { from: 0, insert: "前" }, selection: { anchor: 1 }, userEvent: "input" });
  const other = document.createElement("input");
  document.body.append(other);
  other.focus();
  // focus を奪う変異は、jsdom では CodeMirror の測定が回り続けて落ちる（失敗の文が出ない）= 呼んだ回数で先に止める
  let focusCalls = 0;
  const realFocus = h.view.focus;
  h.view.focus = () => void focusCalls++;
  release();
  assert.ok(await waitFor(() => text(h).includes("openref://open?key=MAP1")), "置き換えない");
  assert.equal(focusCalls, 0, "答えが来たときに view.focus() を呼んだ（focus を奪う）");
  h.view.focus = realFocus;
  assert.equal(h.view.state.selection.main.head, 1, "caret が飛んだ");
  assert.equal(document.activeElement, other, "focus を奪った");
  other.remove();
  release = null;
  start("J");
  click(h, "文献リンクにする");
  await waitFor(() => !!release);
  release();
  assert.ok(await waitFor(() => text(h) === "J[](openref://open?key=MAP1)"));
  assert.equal(h.view.state.selection.main.head, text(h).length, "URL の直後の caret がリンクの後ろへ来ない");
  // エディタを閉じた後に答えが来ても書かない
  lookup = () => new Promise((r) => (release = () => r({ key: "LATE", retracted: false })));
  start("H");
  click(h, "文献リンクにする");
  await sleep(5);
  h.destroy();
  release();
  await sleep(10);
  assert.ok(!h.view.state.doc.toString().includes("LATE"), "閉じたエディタに書いた");
  host3.remove();
  tests++;
}

// ---- ④ 蔵書の検索（cite.svelte.ts）----
{
  const row = (key, year, extra = {}) => ({ key, title: "T " + key, firstAuthor: "Sato", firstAuthorGiven: null, year, container: "J", starred: false, hasPdf: false, retracted: false, hasNote: false, fulltextSnippet: null, dateAdded: "", tags: [], ...extra });
  let rows = [];
  handlers.list_items = (a) => rows.slice(0, a.filter.limit || 200);
  handlers.wiki_cite_lookup = (a) => (a.input.includes("10.9999/none") ? { item: null } : /^(10\.|https:\/\/doi|pmid|\d{5})/i.test(a.input) ? { item: row("ID1", 2020) } : null);
  assert.deepEqual(CS.citeQuery(" liver 2020 cirrhosis 2021 "), { id: null, terms: "liver cirrhosis 2021", year: 2020 });
  assert.deepEqual(CS.citeQuery("胆道 1999"), { id: null, terms: "胆道", year: 1999 });
  assert.deepEqual(CS.citeQuery("12345"), { id: "12345", terms: "12345", year: null }, "5 桁以上の数字は PMID");
  assert.equal(CS.citeQuery("2020").year, 2020, "4 桁の年は PMID にしない");
  assert.equal(CS.citeQuery("10.1000/x").id, "10.1000/x");
  assert.equal(CS.citeQuery("doi: 10.1000/x").id, "doi: 10.1000/x");
  // 語 = list_items（search + metadataOnly・上位 20）
  rows = Array.from({ length: 30 }, (_, i) => row("K" + i, 2000 + (i % 3)));
  calls.length = 0;
  let r = await CS.searchCite("liver");
  const f = calls.at(-1).args.filter;
  assert.deepEqual({ search: f.search, metadataOnly: f.metadataOnly, limit: f.limit }, { search: "liver", metadataOnly: true, limit: 20 });
  assert.equal(r.items.length, 20);
  // 年 = 語から外して多めに取って絞る
  r = await CS.searchCite("liver 2001");
  assert.equal(calls.at(-1).args.filter.search, "liver", "年を語に残した");
  assert.equal(calls.at(-1).args.filter.limit, 200);
  assert.ok(r.items.length === 10 && r.items.every((x) => x.year === 2001), "年で絞らない");
  r = await CS.searchCite("2002");
  assert.equal(calls.at(-1).args.filter.search, null);
  assert.ok(r.items.every((x) => x.year === 2002));
  // DOI / PMID / URL = 照合の 1 件（list_items を呼ばない）
  calls.length = 0;
  r = await CS.searchCite("https://doi.org/10.1000/abc");
  assert.deepEqual(r.items.map((x) => x.key), ["ID1"]);
  assert.deepEqual(calls.map((c) => c.name), ["wiki_cite_lookup"]);
  r = await CS.searchCite("10.9999/none");
  assert.deepEqual(r, { items: [], note: "missing" });
  // 識別子として読めない URL = 語として
  calls.length = 0;
  r = await CS.searchCite("https://example.com/x");
  assert.deepEqual(calls.map((c) => c.name), ["wiki_cite_lookup", "list_items"]);
  // 空 = 何も呼ばない
  calls.length = 0;
  assert.deepEqual(await CS.searchCite("   "), { items: [], note: "" });
  assert.equal(calls.length, 0);
  tests++;
}

// ---- ⑤ 引用の窓（実部品 WikiCite + WikiPage）----
{
  const { default: WikiPage } = await import("./WikiPage.svelte");
  const { EditorView } = await import("@codemirror/view");
  const row = (key, extra = {}) => ({ key, title: "Title " + key, firstAuthor: "Sato", firstAuthorGiven: null, year: 2020, container: "Radiology", starred: false, hasPdf: false, retracted: false, hasNote: false, fulltextSnippet: null, dateAdded: "", tags: [], ...extra });
  handlers.list_items = () => [row("AAA1"), row("BBB2", { retracted: true })];
  handlers.wiki_save = (a) => ({ ok: true, path: a.path, sha256: "S1", size: a.content.length, mtime: 7, created: false });
  const files = { "b.md": "# 記事\n\n肝硬変の所見\n", "c.md": "# 別\n" };
  handlers.wiki_read_gui = (a) => page(a.path, files[a.path], "B0");
  handlers.wiki_backlinks = () => ({ total: 0, pages: [] });
  S.app.wikiOn = true;
  W.wk.status = { enabled: true, root: "/v" };
  const tab = S.app.tabs[0];
  tab.wiki = { path: "b.md", root: "/v", scrollTop: 0, editing: true };
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(WikiPage, { target });
  const has = (s) => !!document.querySelector(s);
  const view = () => EditorView.findFromDOM(target.querySelector(".cm-editor"));
  assert.ok(await waitFor(() => has(".cm-editor"), 3000), "エディタが開かない");
  const btn = () => [...target.querySelectorAll(".wk-bar button")].find((b) => b.textContent.trim() === "文献");
  assert.ok(btn(), "[文献] ボタンが無い");
  const input = () => document.querySelector(".wk-cite-input");
  const typeQuery = (q) => {
    input().value = q;
    input().dispatchEvent(new Event("input", { bubbles: true }));
  };
  const keyIn = (k, init = {}) => input().dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...init }));
  // 選択して [文献] → 案内に選択・語で探す → ↓ Enter = 2 件目（撤回）を入れる
  const v = view();
  const at = v.state.doc.toString().indexOf("肝硬変");
  v.dispatch({ selection: { anchor: at, head: at + 3 } });
  btn().click();
  flushSync();
  assert.ok(has(".wk-cite"), "[文献] で窓が開かない");
  assert.equal(document.activeElement, input(), "検索欄に focus が無い");
  assert.ok(document.querySelector(".wk-cite-hint").textContent.includes("肝硬変"), "選択の案内が無い");
  typeQuery("cirrhosis");
  assert.ok(await waitFor(() => document.querySelectorAll(".wk-cite-row").length === 2), "候補が出ない");
  assert.ok(document.querySelector(".wk-cite-retracted"), "撤回の印が出ない");
  assert.ok(document.querySelector(".wk-cite-meta").textContent.includes("Sato · 2020 · Radiology"));
  // 窓はモーダル = タブの操作と ⌘⌥I / ⌘⌥C を受けない（state の modalOpen）・窓の下で ⌘F / ⌘E が効かない
  assert.equal(S.modalOpen(), true, "窓がモーダルとして数えられない");
  assert.equal(S.tabOpsAllowed(), false, "窓を開いたままタブを操作できる");
  input().dispatchEvent(new KeyboardEvent("keydown", { key: "f", metaKey: true, bubbles: true, cancelable: true }));
  input().dispatchEvent(new KeyboardEvent("keydown", { key: "e", metaKey: true, bubbles: true, cancelable: true }));
  flushSync();
  await sleep(20);
  assert.equal(document.querySelector(".cm-search"), null, "窓の下で ⌘F が CodeMirror の検索を開いた");
  assert.ok(has(".cm-editor") && has(".wk-cite"), "窓の下で ⌘E が効いた");
  assert.equal(document.activeElement, input(), "窓の下で focus が移った");
  // IME の確定 Enter では入れない
  keyIn("Enter", { keyCode: 229 });
  flushSync();
  assert.ok(has(".wk-cite"), "IME の Enter で入れた");
  notices.length = 0;
  keyIn("ArrowDown");
  keyIn("Enter");
  flushSync();
  assert.ok(!has(".wk-cite"), "Enter で閉じない");
  assert.ok(view().state.doc.toString().includes("[肝硬変](openref://open?key=BBB2)の所見"), `入らない: ${view().state.doc.toString()}`);
  assert.ok(notices.some((m) => m.includes("撤回")), "撤回の印を知らせない");
  // ⌘⌥K（e.code = KeyK・e.key は ˚）= 選択なし → 番号だけ。Esc は閉じるだけ
  const cmdOptK = () => window.dispatchEvent(new KeyboardEvent("keydown", { key: "˚", code: "KeyK", metaKey: true, altKey: true, bubbles: true, cancelable: true }));
  cmdOptK();
  flushSync();
  assert.ok(has(".wk-cite"), "⌘⌥K で窓が開かない");
  assert.ok(document.querySelector(".wk-cite-hint").textContent.includes("選択なし"));
  keyIn("Escape");
  flushSync();
  assert.ok(!has(".wk-cite"), "Esc で閉じない");
  const before = view().state.doc.toString();
  cmdOptK();
  flushSync();
  typeQuery("x");
  await waitFor(() => document.querySelectorAll(".wk-cite-row").length === 2);
  document.querySelector(".wk-cite-row").click();
  flushSync();
  assert.ok(view().state.doc.toString().includes("[](openref://open?key=AAA1)"), "選択なしで番号だけのリンクにならない");
  assert.notEqual(view().state.doc.toString(), before);
  // 「/」の「文献を引用…」が窓を開く（行の頭の /）
  const { slashItems } = await import("./editor/slash.ts");
  let opened = 0;
  const items = slashItems({ onCite: () => opened++ });
  const it = items.find((x) => x.label === "文献を引用…");
  assert.ok(it && it.keys.includes("cite"), "「/」に文献を引用が無い");
  it.action();
  assert.equal(opened, 1);
  assert.equal(slashItems({}).some((x) => x.label === "文献を引用…"), false, "onCite が無いのに出す");
  // 窓を開いたまま別のページへ移った = 閉じる（入れない）
  cmdOptK();
  flushSync();
  typeQuery("x");
  await waitFor(() => document.querySelectorAll(".wk-cite-row").length === 2);
  const row0 = CS.cite.results[0];
  tab.wiki = { path: "c.md", root: "/v", scrollTop: 0, editing: true };
  assert.ok(await waitFor(() => E.ed.path === "c.md" && !has(".wk-cite"), 3000), "別のページへ移っても窓が残る");
  CS.cite.path = "b.md"; // 古い窓の約束で入れようとしても入れない
  notices.length = 0;
  assert.equal(E.citeInsert(row0.key, false, "b.md", "/v"), false, "離れた編集に入れた");
  assert.ok(notices.some((m) => m.includes("入れませんでした")), "入れなかったことを知らせない");
  // 置けない選択（行をまたぐ）では窓を開かずに知らせる
  const v2 = view();
  v2.dispatch({ selection: { anchor: 0, head: v2.state.doc.length } });
  notices.length = 0;
  cmdOptK();
  flushSync();
  assert.ok(!has(".wk-cite"), "行をまたぐ選択で窓を開いた");
  assert.ok(notices.some((m) => m.includes("行をまたぐ")), "開かない理由を知らせない");
  v2.dispatch({ selection: { anchor: 0 } });
  // 表示中（編集していない）は [文献] も ⌘⌥K も無い
  tab.wiki = { path: "c.md", root: "/v", scrollTop: 0, editing: false };
  assert.ok(await waitFor(() => !has(".cm-editor")));
  assert.equal(btn(), undefined, "表示中に [文献] を出した");
  cmdOptK();
  flushSync();
  assert.ok(!has(".wk-cite"), "表示中に ⌘⌥K で開いた");
  unmount(cmp);
  target.remove();
  tests++;
}

// ---- ⑤' 登録の呼び口（edit.svelte の citeRegister）: 待つ間に編集を離れたら本文に入れず知らせる・警告と新規の登録を知らせる ----
{
  let release;
  handlers.wiki_cite_register = (a) => new Promise((r) => (release = () => r({ key: "R1", title: "Paper", retracted: false, created: true, warnings: ["w1"], input: a.input })));
  E.startEdit(page("p.md", "x", "P0"), "/v");
  notices.length = 0;
  let p = E.citeRegister("https://doi.org/10.1/a");
  await waitFor(() => !!release);
  assert.equal(calls.at(-1).args.input, "https://doi.org/10.1/a");
  release();
  assert.deepEqual(await p, { key: "R1", retracted: false });
  assert.ok(notices.includes("⚠ w1") && notices.some((m) => m.startsWith("✓") && m.includes("Paper")), notices.join(" / "));
  release = null;
  p = E.citeRegister("https://doi.org/10.1/a");
  await waitFor(() => !!release);
  E.startEdit(page("q.md", "y", "Q0"), "/v"); // 別のページへ
  notices.length = 0;
  release();
  assert.equal(await p, null, "離れたのに key を返した");
  assert.ok(notices.some((m) => m.includes("本文に入れませんでした")), "離れたことを知らせない");
  E.discardEdit();
  tests++;
}

// ---- ⑥ 字数（count.ts）: 本文 = 見える文字（frontmatter・URL・記号・画像・埋め込み・タグ・空白を数えない）・選択 = そのまま ----
{
  const { countBody, countChars } = await import("./count.ts");
  const md = [
    "---", "title: x", "summary: あいう", "---",
    "# 見出し", "",
    "本文 [リンク](https://example.com/very/long) と **太字** と `code`。", "",
    "![alt 文字](a.png) ![[img.png]] [[ページ#節|別名]] [[メモ]] [](openref://open?key=K) <span class=\"x\">html</span><!-- c -->", "",
    "| a | b |", "|---|---|", "| 1 |  |", "",
    "```js", "let x", "```", "",
    "&amp; &lt;", "",
  ].join("\n");
  assert.equal(countBody(md), 3 + 14 + 8 + 3 + 4 + 2, `本文の字数: ${countBody(md)}`);
  const para = "あ".repeat(500) + "\n" + "い　".repeat(500); // 和文 1,000 字（全角の空白・改行は数えない）
  assert.equal(countBody(para), 1000);
  assert.equal(countBody(""), 0);
  // 地の文の素の URL・`<URL>` は数えない（本文の字ではない）。リンクの文字は数える
  assert.equal(countBody("本文 https://doi.org/10.1000/abc"), 2, "素の URL を数えた");
  assert.equal(countBody("本文 <https://doi.org/10.1000/abc> と <mailto:a@b.jp>"), 3, "<URL> を数えた");
  assert.equal(countBody("[文字](https://x.org) https://y.org/z 後"), 3);
  assert.equal(countBody("---\nsummary: 長い要約\n---\n"), 0, "frontmatter を数えた");
  assert.equal(countChars("summary: 六十字の要約\n"), 14, "選択はそのまま（空白と改行だけ除く）");
  assert.equal(countChars("👍 a"), 2, "コードポイントで数える");
  tests++;
}

// ---- ⑦ 字数の表示（実部品 WikiPage）: 表示中も編集中も・打鍵で変わる・選択 m 字・ページの外の選択は出さない・html は出さない ----
{
  const { default: WikiPage } = await import("./WikiPage.svelte");
  const { EditorView } = await import("@codemirror/view");
  const files = { "n.md": "---\nsummary: 六十字\n---\n# 題\n\n本文です\n", "h.html": "<p>x</p>" };
  handlers.wiki_read_gui = (a) => ({ ...page(a.path, files[a.path], "N0"), kind: a.path.endsWith(".html") ? "html" : "md" });
  handlers.wiki_backlinks = () => ({ total: 0, pages: [] });
  handlers.wiki_save = (a) => ({ ok: true, path: a.path, sha256: "S1", size: a.content.length, mtime: 7, created: false });
  S.app.wikiOn = true;
  W.wk.status = { enabled: true, root: "/v" };
  const tab = S.app.tabs[0];
  tab.wiki = { path: "n.md", root: "/v", scrollTop: 0, editing: false };
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(WikiPage, { target });
  const count = () => target.querySelector(".wk-count")?.textContent.replace(/\s+/g, " ").trim();
  assert.ok(await waitFor(() => count() === "5 字"), `表示中の字数: ${count()}`);
  // ページの中の選択（表示）= 選択 m 字・外の選択 = 出さない
  const selectNode = (n) => {
    const r = document.createRange();
    r.selectNodeContents(n);
    const s = document.getSelection();
    s.removeAllRanges();
    s.addRange(r);
    document.dispatchEvent(new Event("selectionchange"));
  };
  selectNode(target.querySelector(".wk-bar-name"));
  assert.ok(await waitFor(() => count() === "選択 4 字 5 字"), `ページの中の選択: ${count()}`);
  const outside = document.createElement("p");
  outside.textContent = "外の文字";
  document.body.append(outside);
  selectNode(outside);
  assert.ok(await waitFor(() => count() === "5 字"), `ページの外の選択を数えた: ${count()}`);
  // 編集中: 打鍵で変わる・エディタの選択（frontmatter の中も）
  tab.wiki = { path: "n.md", root: "/v", scrollTop: 0, editing: true };
  assert.ok(await waitFor(() => !!target.querySelector(".cm-editor"), 3000));
  const view = () => EditorView.findFromDOM(target.querySelector(".cm-editor"));
  view().dispatch({ changes: { from: view().state.doc.length, insert: "追記した" }, userEvent: "input" });
  assert.ok(await waitFor(() => count() === "9 字"), `打鍵で字数が変わらない: ${count()}`);
  // DOM の選択を "summary: 六十字" に（CodeMirror は DOM の選択を自分の選択に取り込む = 実機と同じ道）
  const walker = document.createTreeWalker(view().contentDOM, NodeFilter.SHOW_TEXT);
  let tn;
  while ((tn = walker.nextNode()) && !tn.data.includes("summary"));
  const r = document.createRange();
  r.setStart(tn, tn.data.indexOf("summary"));
  r.setEnd(tn, tn.data.indexOf("六十字") + 3);
  document.getSelection().removeAllRanges();
  document.getSelection().addRange(r);
  document.dispatchEvent(new Event("selectionchange"));
  assert.ok(await waitFor(() => count() === "選択 11 字 9 字"), `エディタの選択: ${count()}`);
  // html のページは出さない
  tab.wiki = { path: "h.html", root: "/v", scrollTop: 0, editing: false };
  assert.ok(await waitFor(() => W.wk.page?.path === "h.html"));
  await sleep(20);
  assert.equal(target.querySelector(".wk-count"), null, "html のページに字数を出した");
  unmount(cmp);
  target.remove();
  outside.remove();
  tests++;
}

// ---- ⑧ 見出しで畳む（foldGutter + foldKeymap）: 畳む印がある・鍵で見出しの節が畳まれる・本文は変わらない ----
{
  const { foldedRanges } = await import("@codemirror/language");
  const doc = "# 記事\n\n前書き\n\n## 節 A\n\n本文 A\n\n### 小節\n\nA の続き\n\n## 節 B\n\n本文 B\n";
  const h = createEditor(document.createElement("div"), { doc, mode: "md", onChange: () => {} });
  assert.ok(h.view.dom.querySelector(".cm-foldGutter"), "畳む印の溝が無い");
  const mac = /Mac|iP(hone|[oa]d)/.test(navigator.platform);
  const foldKey = (close) =>
    h.view.contentDOM.dispatchEvent(new KeyboardEvent("keydown", mac
      ? { key: close ? "[" : "]", keyCode: close ? 219 : 221, metaKey: true, altKey: true, bubbles: true, cancelable: true }
      : { key: close ? "{" : "}", keyCode: close ? 219 : 221, ctrlKey: true, shiftKey: true, bubbles: true, cancelable: true }));
  const at = doc.indexOf("## 節 A");
  sel(h, at + 3);
  foldKey(true);
  const ranges = [];
  foldedRanges(h.view.state).between(0, doc.length, (from, to) => ranges.push(h.view.state.sliceDoc(from, to)));
  assert.equal(ranges.length, 1, "鍵で畳まない");
  assert.ok(ranges[0].includes("本文 A") && ranges[0].includes("A の続き") && !ranges[0].includes("節 B"), `節の範囲が違う: ${JSON.stringify(ranges[0])}`);
  assert.equal(h.getText(), doc, "畳んで本文が変わった");
  foldKey(false);
  let n = 0;
  foldedRanges(h.view.state).between(0, doc.length, () => void n++);
  assert.equal(n, 0, "鍵で開かない");
  h.destroy();
  tests++;
}

// ---- ⑧b 言語の切替: setLocale で文言を差し替える・本文の丸ごとの置き換え（setText = 新しい state）の後も今の言語の文言 ----
{
  const h = createEditor(document.createElement("div"), { doc: "a\n", mode: "md", onChange: () => {}, phrases: { Find: "検索" }, placeholder: "ここに書く" });
  assert.equal(h.view.state.phrase("Find"), "検索");
  h.setLocale({ Find: "Search" }, "Write here");
  assert.equal(h.view.state.phrase("Find"), "Search", "setLocale で文言が替わらない");
  h.setText("b\n");
  assert.equal(h.view.state.phrase("Find"), "Search", "本文の丸ごとの置き換えの後に前の言語の文言へ戻った");
  h.setText("");
  await sleep(5);
  assert.ok(h.view.dom.querySelector(".cm-placeholder")?.textContent.includes("Write here"), `空の本文の placeholder が今の言語でない: ${h.view.dom.querySelector(".cm-placeholder")?.textContent}`);
  h.destroy();
  tests++;
}

// ---- ⑨ 表の中の Tab / Shift+Tab（editor/table.ts）----
{
  const tab = (h, init = {}) => {
    const e = new KeyboardEvent("keydown", { key: "Tab", keyCode: 9, bubbles: true, cancelable: true, ...init });
    h.view.contentDOM.dispatchEvent(e);
    return e;
  };
  const caret = (h) => h.view.state.selection.main.head;
  const doc = "前\n\n| 項目 | 値 |\n| --- | --- |\n| CT |  |\n|  | 2 |\n\n後";
  const h = createEditor(document.createElement("div"), { doc, mode: "md", onChange: () => {} });
  const p = (s, d = 0) => text(h).indexOf(s) + d;
  sel(h, p("項目", 1)); // 見出しの 1 つ目のセルの中
  tab(h);
  assert.equal(caret(h), p("値", 1), "次のセルへ移らない");
  tab(h); // 行の最後 → 次の行の最初のセル（区切りの行は飛ばす）
  assert.equal(caret(h), p("CT", 2), "次の行の最初のセルへ移らない");
  tab(h); // 空のセルも飛ばさない（TableCell が無い）
  assert.equal(caret(h), p("CT |  |", 5), `空のセルを飛ばした: ${caret(h)}`);
  tab(h);
  assert.equal(caret(h), p("|  | 2", 2), "次の行の空のセルへ移らない");
  tab(h);
  assert.equal(caret(h), p("| 2 |", 3));
  // Shift+Tab = 前のセル（行の頭 → 前の行の最後）
  tab(h, { shiftKey: true });
  assert.equal(caret(h), p("|  | 2", 2));
  tab(h, { shiftKey: true });
  assert.equal(caret(h), p("CT |  |", 5));
  // 最後のセルで Tab = 行を足す（列の数は見出しと同じ）
  sel(h, p("| 2 |", 3));
  tab(h);
  assert.equal(text(h), "前\n\n| 項目 | 値 |\n| --- | --- |\n| CT |  |\n|  | 2 |\n|  |  |\n\n後", "行を足さない");
  assert.equal(caret(h), p("|  |  |\n\n後", 2), "足した行の最初のセルへ移らない");
  // 区切りの行の上の Tab = 本文の最初のセル・最初のセルの Shift+Tab = 留まる（字下げに流さない）
  sel(h, p("| --- |", 3));
  tab(h);
  assert.equal(caret(h), p("CT", 2));
  sel(h, p("項目", 0));
  const before = text(h);
  assert.equal(tab(h, { shiftKey: true }).defaultPrevented, true);
  assert.equal(text(h), before, "最初のセルの Shift+Tab で本文が変わった");
  // 表の外 = 今どおり字下げ（indentWithTab）
  sel(h, p("後", 0));
  tab(h);
  assert.ok(text(h).endsWith("\n  後") || text(h).endsWith("\n\t後"), `表の外の Tab が字下げにならない: ${JSON.stringify(text(h).slice(-6))}`);
  // IME の変換中の Tab は奪わない（表の鍵は動かない = caret はそのまま）
  for (const init of [{ isComposing: true }, { keyCode: 229 }]) {
    sel(h, p("項目", 1));
    const c0 = caret(h);
    tab(h, init);
    assert.ok(caret(h) !== p("値", 1), `IME の Tab（${JSON.stringify(init)}）でセルを移った`);
    assert.ok(caret(h) >= c0);
  }
  // 頭の | の無い表
  h.setText("a | b\n--- | ---\n1 | 2\n");
  sel(h, 0);
  tab(h);
  assert.equal(caret(h), text(h).indexOf("b") + 1);
  sel(h, text(h).indexOf("2") + 1);
  tab(h);
  assert.equal(text(h), "a | b\n--- | ---\n1 | 2\n | \n", "頭の | の無い表に合わせて行を足さない");
  h.destroy();
  // 本文の行が無い表 = 区切りの行の後ろに足す（見出しと区切りの間に入れない）・引用 / リストの中の表 = 行の頭を写す
  const { syntaxTree, ensureSyntaxTree } = await import("@codemirror/language");
  const rowsIn = (ed) => {
    const t = ensureSyntaxTree(ed.view.state, ed.view.state.doc.length, 2000) ?? syntaxTree(ed.view.state);
    const out = { tables: 0, rows: 0 };
    t.iterate({ enter: (n) => void (n.name === "Table" ? out.tables++ : n.name === "TableRow" ? out.rows++ : 0) });
    return out;
  };
  for (const [doc, at, want] of [
    ["| a | b |\n| --- | --- |\n\n後", "| b", "| a | b |\n| --- | --- |\n|  |  |\n\n後"],
    ["| a | b |\n| --- | --- |", "---", "| a | b |\n| --- | --- |\n|  |  |"],
    ["> | a | b |\n> | --- | --- |\n> | 1 | 2 |\n", "| 2", "> | a | b |\n> | --- | --- |\n> | 1 | 2 |\n> |  |  |\n"],
    ["- 項目\n\n  | a |\n  | --- |\n  | 1 |\n", "| 1", "- 項目\n\n  | a |\n  | --- |\n  | 1 |\n  |  |\n"],
  ]) {
    const e2 = createEditor(document.createElement("div"), { doc, mode: "md", onChange: () => {} });
    const p0 = doc.indexOf(at) + at.length - (at === "---" ? 1 : 0);
    e2.view.dispatch({ selection: { anchor: p0 } });
    // 最後のセルまで進めて Tab（本文の行が無い表では見出しの最後のセル・区切りの行から 1 回で足す）
    tab(e2);
    if (text(e2) === doc) tab(e2);
    assert.equal(text(e2), want, `行を足す所 / 行の頭が違う: ${JSON.stringify(doc)}`);
    assert.equal(rowsIn(e2).tables, 1, "足した行で表が割れた");
    assert.ok(rowsIn(e2).rows >= 1, `足した行が表の行にならない: ${JSON.stringify(text(e2))}`);
    const ln = e2.view.state.doc.lineAt(e2.view.state.selection.main.head).text;
    assert.ok(ln.trimStart().replace(/^>\s*/, "").startsWith("|"), "caret が足した行に無い");
    e2.destroy();
  }
  // 1 行の中の選択（セルの中の 1 語）は caret と同じくセルを移る（字下げしない = 表から落ちない）
  const e4 = createEditor(document.createElement("div"), { doc: "| a | b |\n| --- | --- |\n| CT | MR |\n", mode: "md", onChange: () => {} });
  const ct = text(e4).indexOf("CT");
  e4.view.dispatch({ selection: { anchor: ct, head: ct + 2 } });
  tab(e4);
  assert.equal(text(e4), "| a | b |\n| --- | --- |\n| CT | MR |\n", "1 語の選択の Tab で字下げした");
  assert.equal(caret(e4), text(e4).indexOf("MR") + 2, "1 語の選択の Tab で次のセルへ移らない");
  e4.destroy();
  // 複数行の選択は字下げ（選択の頭が表の中でもセルを移らない）
  const e3 = createEditor(document.createElement("div"), { doc: "| a | b |\n| --- | --- |\n| 1 | 2 |\n後の行\n", mode: "md", onChange: () => {} });
  e3.view.dispatch({ selection: { anchor: 2, head: e3.view.state.doc.length - 1 } });
  tab(e3);
  assert.ok(text(e3).startsWith("  | a |") || text(e3).startsWith("\t| a |"), `選択の Tab が字下げにならない: ${JSON.stringify(text(e3))}`);
  e3.destroy();
  // 「/」の表を入れた直後の Tab は snippet の欄の移動が先（表の鍵より前）
  const { slashSource, slashItems } = await import("./editor/slash.ts");
  const g = createEditor(document.createElement("div"), { doc: "/table", mode: "md", onChange: () => {} });
  const table = slashItems({}).find((x) => x.keys.includes("table"));
  const res = slashSource({})({ state: g.view.state, pos: 6, explicit: true, matchBefore: () => null, aborted: false });
  res.options.find((o) => o.label === table.label).apply(g.view, null, res.from, res.to);
  const s0 = g.view.state.selection.main;
  assert.equal(g.view.state.sliceDoc(s0.from, s0.to), "列 1", "雛形の最初の欄が選ばれていない");
  tab(g);
  const s1 = g.view.state.selection.main;
  assert.equal(g.view.state.sliceDoc(s1.from, s1.to), "列 2", "snippet の Tab より表の鍵が先に効いた");
  g.destroy();
  tests++;
}

// ---- ⑩ 覆いの下でページのキーが裏に効かない ----
{
  const { default: WikiPage } = await import("./WikiPage.svelte");
  const { EditorView } = await import("@codemirror/view");
  const CH = await import("../chat/chat.svelte.ts");
  let saves = 0;
  handlers.wiki_save = (a) => (saves++, { ok: true, path: a.path, sha256: "S" + saves, size: a.content.length, mtime: 7, created: false });
  handlers.wiki_read_gui = (a) => page(a.path, "# 題\n\n本文の行\n", "K0");
  handlers.wiki_backlinks = () => ({ total: 0, pages: [] });
  S.app.wikiOn = true;
  W.wk.status = { enabled: true, root: "/v" };
  const tab = S.app.tabs[0];
  tab.wiki = { path: "k.md", root: "/v", scrollTop: 0, editing: false };
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(WikiPage, { target });
  const has = (sel) => !!target.querySelector(sel);
  assert.ok(await waitFor(() => W.wk.page?.path === "k.md", 3000), "ページが開かない");
  assert.ok(S.frontWiki()?.path === "k.md", "OpenNote が前面でない");
  const key = (init) => {
    const ev = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
    window.dispatchEvent(ev);
    flushSync();
    return ev;
  };
  const cmdE = () => key({ key: "e", metaKey: true });
  const cmdF = () => key({ key: "f", metaKey: true });
  const cmdS = () => key({ key: "s", metaKey: true });
  const cmdOptK = () => key({ key: "˚", code: "KeyK", metaKey: true, altKey: true });
  const editing = () => has(".cm-editor");
  const startSearch = () => {
    W.wk.q = "所見";
    W.wk.showResults = true;
    flushSync();
  };
  const endSearch = () => {
    W.wk.q = "";
    W.wk.showResults = false;
    flushSync();
  };
  const COVERS = [
    ["参照エディタ", () => (S.app.editorOpen = true), () => (S.app.editorOpen = false)],
    ["取込", () => (S.app.importJob = { files: [], done: false }), () => (S.app.importJob = null)],
    ["チャットの同意", () => (CH.chat.consent = true), () => (CH.chat.consent = false)],
    ["救出", () => E.ed.stranded.push({ path: "z.md", content: "x", reason: "test" }), () => E.ed.stranded.splice(0)],
    ["本体の窓", () => (S.app.modal = "manageLabels"), () => (S.app.modal = null)],
  ];
  // 表示中: ⌘E・⌘F・検索中の Esc
  for (const [name, on, off] of COVERS) {
    on();
    flushSync();
    assert.ok(S.covered(), `${name}: 覆いとして数えられない`);
    const f0 = W.wk.findRequest;
    const evE = cmdE();
    const evF = cmdF();
    await sleep(60);
    assert.equal(editing(), false, `${name}: 覆いの下で ⌘E が裏のページを編集にした`);
    assert.equal(W.wk.findRequest, f0, `${name}: 覆いの下で ⌘F が裏のページ内検索を開いた`);
    assert.deepEqual([evE.defaultPrevented, evF.defaultPrevented], [false, false], `${name}: 覆いの下で ⌘E / ⌘F を奪った（窓の側へ素通しにする）`);
    startSearch();
    key({ key: "Escape" });
    assert.equal(W.wk.q, "所見", `${name}: 覆いの下で Esc が裏の検索を消した`);
    endSearch();
    off();
    flushSync();
  }
  // 覆いが無ければ今までどおり: ⌘F = ページ内検索・検索中の Esc = 検索を消す・⌘E = 編集へ
  const f1 = W.wk.findRequest;
  assert.equal(cmdF().defaultPrevented, true, "⌘F を奪わない");
  assert.equal(W.wk.findRequest, f1 + 1, "覆いが無いのに ⌘F が効かない");
  startSearch();
  key({ key: "Escape" });
  assert.equal(W.wk.q, "", "覆いが無いのに検索中の Esc が検索を消さない");
  endSearch();
  cmdE();
  assert.ok(await waitFor(editing, 3000), "覆いが無いのに ⌘E が編集にしない");
  // 編集中: ⌘⌥K（引用の窓）・⌘S（保存）
  const view = () => EditorView.findFromDOM(target.querySelector(".cm-editor"));
  const at = view().state.doc.toString().indexOf("本文");
  view().dispatch({ selection: { anchor: at + 1 } });
  const delay0 = E.cfg.delay;
  E.cfg.delay = 60000; // 自動保存を待たない = ⌘S だけが書く
  for (const [name, on, off] of COVERS) {
    on();
    flushSync();
    const evK = cmdOptK();
    await sleep(20);
    assert.equal(CS.cite.open, false, `${name}: 覆いの下で ⌘⌥K が引用の窓を重ねた`);
    assert.equal(evK.defaultPrevented, false, `${name}: 覆いの下で ⌘⌥K を奪った`);
    const evE = cmdE();
    await sleep(40);
    assert.ok(editing(), `${name}: 覆いの下で ⌘E が裏のページを表示に戻した`);
    // ⌘S = 覆いの下でも保存する（今のまま）
    view().dispatch({ changes: { from: at, insert: "あ" } });
    const n = saves;
    const evS = cmdS();
    assert.ok(await waitFor(() => saves === n + 1, 2000), `${name}: 覆いの下の ⌘S で保存しない`);
    assert.equal(evS.defaultPrevented, true, `${name}: ⌘S を奪わない（ページの保存の既定へ流れる）`);
    off();
    flushSync();
    void evE;
  }
  E.cfg.delay = delay0;
  // 覆いが無ければ ⌘⌥K で引用の窓・⌘E で表示へ
  cmdOptK();
  assert.equal(CS.cite.open, true, "覆いが無いのに ⌘⌥K で引用の窓が開かない");
  CS.closeCite();
  flushSync();
  cmdE();
  assert.ok(await waitFor(() => !editing(), 3000), "覆いが無いのに ⌘E で表示へ戻らない");
  unmount(cmp);
  target.remove();
  tab.wiki = null;
  tests++;
}

// ---- ⑪ 先に走る受け手が同じキーで覆いを閉じても、裏のページは動かない（受け手の順序ごと）----
{
  const { default: WikiPage } = await import("./WikiPage.svelte");
  const CH = await import("../chat/chat.svelte.ts");
  handlers.wiki_read_gui = (a) => page(a.path, "# 題\n\n本文の行\n", "M0");
  handlers.wiki_backlinks = () => ({ total: 0, pages: [] });
  S.app.wikiOn = true;
  W.wk.status = { enabled: true, root: "/v" };
  const tab = S.app.tabs[0];
  tab.wiki = { path: "m.md", root: "/v", scrollTop: 0, editing: false };
  // 実機の順: App の window の keydown（Esc の連鎖 = 取込の窓を閉じる closeImport）・窓自身の Esc の受け手は、ページ（WikiPage）より先に登録されて先に走る
  // = WikiPage より先に、同じ Esc で覆いを閉じる受け手を window に置く
  let closer = null;
  const early = (e) => {
    if (e.key === "Escape" && closer) closer();
  };
  window.addEventListener("keydown", early);
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(WikiPage, { target });
  assert.ok(await waitFor(() => W.wk.page?.path === "m.md", 3000), "ページが開かない");
  // 実機のキーの標的 = 焦点の要素（ここでは body）= window の capture の段が先に走る
  const esc = () => {
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    flushSync();
  };
  const COVERS = [
    ["取込の窓", () => (S.app.importJob = { files: [], done: false }), () => (S.app.importJob = null)],
    ["本体の窓", () => (S.app.modal = "manageLabels"), () => (S.app.modal = null)],
    ["参照エディタ", () => (S.app.editorOpen = true), () => (S.app.editorOpen = false)],
    ["チャットの同意", () => (CH.chat.consent = true), () => (CH.chat.consent = false)],
    ["救出", () => E.ed.stranded.push({ path: "z.md", content: "x", reason: "test" }), () => E.ed.stranded.splice(0)],
  ];
  for (const [name, on, off] of COVERS) {
    W.wk.q = "所見";
    W.wk.showResults = true;
    on();
    closer = off;
    flushSync();
    esc();
    assert.equal(S.covered(), false, `${name}: 先の受け手が窓を閉じていない（試験の前提）`);
    assert.equal(W.wk.q, "所見", `${name}: 窓を閉じた同じ Esc が裏の検索を消した`);
    closer = null;
    // 覆いが無い次の Esc は今までどおり検索を消す
    esc();
    assert.equal(W.wk.q, "", `${name}: 覆いが無いのに Esc が検索を消さない`);
    W.wk.showResults = false;
    flushSync();
  }
  window.removeEventListener("keydown", early);
  unmount(cmp);
  target.remove();
  tab.wiki = null;
  tests++;
}

console.log(`wiki write: ok (${tests} tests)`);
process.exit(0);
