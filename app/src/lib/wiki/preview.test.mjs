/* OpenNote のリンクのプレビューの部品テスト（node app/src/lib/wiki/preview.test.mjs・実部品 preview.svelte.ts / WikiPreview /
   ShadowPage / WikiResults / WikiInfo / ChatMd / WikiLinkMenu + jsdom・invoke は差し替え）。確かめること:
   ① 本文の `[[ページ]]` にしばらく乗せると浮き窓（App に 1 つ = 乗せた ShadowPage の外の要素）・中身は同じ描画（shadow の中）・読みはそのページ 1 回
   ② 待ち時間の前に離れたら出さない・離れたら閉じる・浮き窓の上へ移れば保つ・浮き窓から離れたら閉じる
   ③ Esc で閉じて背後へ流さない・外側の押下・乗せたリンクを含む箱のスクロール（別の箱のスクロールでは閉じない）・乗せたリンクが DOM から消えた・
      タブの切替・右クリックのメニューで閉じる・元のリンクのクリックで閉じる
   ④ まだ無いページ = 案内だけ（読まない）・md 以外（PDF）と参照フォルダのページ（root が画面の root でない）は出さない・押しながら（選択中）は出さない
   ⑤ 浮き窓の中のリンク = 乗せた側の onNavigate（⌘クリック = 新しいタブ）で開いて閉じる・題名のクリックも同じ・中で乗せても次のプレビューは出さない
   ⑥ `[[ページ#見出し]]` = 浮き窓の中の箱だけをその見出しへ動かす
   ⑦ 右ペインのバックリンク・検索の結果の行・チャットの回答の `[[ ]]` でも出る（md 以外の行は出さない）
   ⑧ 大きなページ（約 400KB・画像 300 枚）: 浮き窓の中の画像はすべて loading="lazy"・描画の時間を出す（jsdom = 実機より遅い上限の目安）
   ⑨ 浮き窓の中のリンクの右クリックのメニューの項目（開く / 新しいタブで開く）が乗せた側の onNavigate で動く・浮き窓の中の画像 = 覆い
      （TypeError にならない・覆いの [新しいタブで開く] も動く）・⌘クリック = 画像を新しいタブ
   ⑩ 浮き窓が開いている間の時計（乗せた要素の見張り）は、閉じるたびに止まる
   ⑪ 乗せた要素が見えない（祖先に visibility: hidden / display: none = 集中モードで隠した右ペイン・ヘッダー）なら開かない・開いている間に見えなくなったら閉じる
      （shadow の中のリンクは host を辿る）
   ⑫ アプリのモーダル（引用の窓 等 = modalOpen）が開いたら閉じる・開いている間は出さない（窓の下に隠れた浮き窓が Esc を奪わない）
   出力 = `wiki preview: ok (N tests)` */
import assert from "node:assert/strict";
import { setupHarness } from "./test-harness.mjs";

const { calls, handlers, sleep, waitFor } = setupHarness();
let tests = 0;

const { mount, unmount, flushSync } = await import("svelte");
const S = await import("../state.svelte.ts");
const W = await import("./wiki.svelte.ts");
const P = await import("./preview.svelte.ts");
const LM = await import("./linkmenu.svelte.ts");
const { default: ShadowPage } = await import("./render/ShadowPage.svelte");
const { default: Preview } = await import("./WikiPreview.svelte");
const LB = await import("./lightbox.svelte.ts");
// 浮き窓の見張りの時計（300ms）の作成と停止を数える（⑩）
const ticking = new Set();
{
  const si = globalThis.setInterval;
  const ci = globalThis.clearInterval;
  globalThis.setInterval = (f, ms, ...a) => {
    const id = si(f, ms, ...a);
    if (ms === 300) ticking.add(id);
    return id;
  };
  globalThis.clearInterval = (id) => {
    ticking.delete(id);
    return ci(id);
  };
}
P.previewCfg.openDelay = 30;
P.previewCfg.closeDelay = 30;

const page = (path, content) => ({ path, abs_path: "/v/" + path, kind: "md", title: path.replace(/\.md$/, ""), size: content.length, mtime: 0, content, sha256: path, truncated: false, links: [] });
S.app.wikiOn = true;
W.wk.status = { enabled: true, root: "/v" };
handlers.get_ui_pref = () => null;
handlers.set_ui_pref = () => null;
handlers.wiki_backlinks = () => ({ total: 0, pages: [] });

const reads = [];
const pages = {
  "notes/x.md": page("notes/x.md", "# X\n\nxの本文 [[y]] [外](https://example.com/)\n\n## A\n\naaa\n\n![[a.png]]\n\n## B\n\nbbb\n"),
  "notes/y.md": page("notes/y.md", "yの本文"),
};
pages["notes/x.md"].links = [
  { kind: "wiki", target: "y", path: "notes/y.md", ambiguous: false },
  { kind: "embed", target: "a.png", path: "img/a.png", ambiguous: false },
];
const readPage = async (p) => {
  reads.push(p);
  if (!pages[p]) throw new Error("no such page: " + p);
  return pages[p];
};
const links = [
  { kind: "wiki", target: "x", path: "notes/x.md", ambiguous: false },
  { kind: "wiki", target: "x#B", path: "notes/x.md", ambiguous: false },
  { kind: "wiki", target: "y", path: "notes/y.md", ambiguous: false },
  { kind: "wiki", target: "doc.pdf", path: "docs/doc.pdf", ambiguous: false },
  { kind: "wiki", target: "gone", path: null, ambiguous: false },
];
const navs = [];
const host = document.createElement("div");
document.body.append(host);
const view = mount(ShadowPage, {
  target: host,
  props: { mode: "md", page: { ...page("p.md", "[[x]] [[x#B]] [[doc.pdf]] [[gone]] 地の文"), links }, root: "/v", refs: [], onNavigate: (n, nt) => navs.push([n, !!nt]), readPage },
});
const ptarget = document.createElement("div");
document.body.append(ptarget);
const pv = mount(Preview, { target: ptarget });
flushSync();
const sr = () => host.querySelector(".wk-md")?.shadowRoot;
assert.ok(await waitFor(() => sr()?.querySelectorAll("a").length >= 4), "リンクを描かない");
const link = (text) => [...sr().querySelectorAll("a")].find((x) => x.textContent.trim() === text) ?? assert.fail(`リンクが無い: ${text} / ${[...sr().querySelectorAll("a")].map((x) => x.textContent).join(" , ")}`);
const over = (el, extra = {}) => {
  el.dispatchEvent(new MouseEvent("mouseover", { bubbles: true, composed: true, clientX: 40, clientY: 10, ...extra }));
};
const out = (el, to = document.body) => {
  el.dispatchEvent(new MouseEvent("mouseout", { bubbles: true, composed: true, relatedTarget: to }));
};
const box = () => ptarget.querySelector(".wk-pv");
const inner = () => box()?.querySelector(".wk-md")?.shadowRoot;
const settle = async () => {
  for (let i = 0; i < 5; i++) {
    await sleep(0);
    flushSync();
  }
};
const reset = async () => {
  P.closePreview();
  await settle();
  reads.length = 0;
  navs.length = 0;
};

// ---------- ① 乗せると浮き窓 ----------
{
  const a = link("x");
  over(a);
  await sleep(5);
  flushSync();
  assert.equal(box(), null, "待ち時間の前に出した");
  assert.ok(await waitFor(() => P.preview.cur), "乗せても出ない");
  await settle();
  assert.ok(box(), "浮き窓を描かない");
  assert.ok(!host.contains(box()), "浮き窓が乗せた描画（contain の中）にある");
  assert.equal(P.preview.cur.path, "notes/x.md");
  assert.ok(await waitFor(() => inner()?.textContent.includes("xの本文")), "中身を描かない");
  assert.deepEqual(reads, ["notes/x.md"], "読みが 1 回でない");
  // 同じリンクの中で動いても出し直さない
  const seq = P.preview.cur.seq;
  over(a);
  await sleep(40);
  assert.equal(P.preview.cur.seq, seq, "同じリンクで出し直した");
  await reset();
  tests++;
}

// ---------- ② 離れる・浮き窓の上へ移る ----------
{
  const a = link("x");
  over(a);
  await sleep(5);
  out(a);
  await sleep(60);
  assert.equal(P.preview.cur, null, "待ち時間の前に離れたのに出した");
  over(a);
  assert.ok(await waitFor(() => P.preview.cur));
  await settle();
  out(a);
  await sleep(5);
  box().dispatchEvent(new MouseEvent("mouseenter"));
  await sleep(60);
  assert.ok(P.preview.cur, "浮き窓の上へ移ったのに閉じた");
  box().dispatchEvent(new MouseEvent("mouseleave"));
  assert.ok(await waitFor(() => !P.preview.cur), "浮き窓から離れても閉じない");
  // 別のリンクへ移る = 前の浮き窓は離れたときの時計で閉じ、次は待ち時間の後に出る（古い物を残さない）
  P.previewCfg.openDelay = 120;
  over(a);
  assert.ok(await waitFor(() => P.preview.cur?.path === "notes/x.md"));
  out(a);
  over(link("gone"));
  await sleep(70);
  assert.equal(P.preview.cur, null, "別のリンクへ移っても前の浮き窓が残る");
  assert.ok(await waitFor(() => P.preview.cur?.path === null), "移った先の浮き窓が出ない");
  P.previewCfg.openDelay = 30;
  out(link("gone"));
  assert.ok(await waitFor(() => !P.preview.cur));
  // リンクから離れて何もしなければ閉じる
  over(a);
  assert.ok(await waitFor(() => P.preview.cur));
  out(a);
  assert.ok(await waitFor(() => !P.preview.cur), "リンクから離れても閉じない");
  await reset();
  tests++;
}

// ---------- ③ Esc・外側の押下・スクロール・タブ・右クリックのメニュー・元のリンクのクリック ----------
{
  const a = link("x");
  const open = async () => {
    over(a);
    assert.ok(await waitFor(() => P.preview.cur));
    await settle();
  };
  await open();
  let behind = 0;
  const bubble = (e) => e.key === "Escape" && behind++;
  window.addEventListener("keydown", bubble);
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
  await settle();
  window.removeEventListener("keydown", bubble);
  assert.equal(P.preview.cur, null, "Esc で閉じない");
  assert.equal(behind, 0, "Esc が背後へ流れた");
  // IME の変換中の Esc では閉じない
  await open();
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", isComposing: true, bubbles: true }));
  await settle();
  assert.ok(P.preview.cur, "変換中の Esc で閉じた");
  // 浮き窓の中の押下では閉じない・外側の押下で閉じる
  box().dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
  await settle();
  assert.ok(P.preview.cur, "浮き窓の中の押下で閉じた");
  document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
  await settle();
  assert.equal(P.preview.cur, null, "外側の押下で閉じない");
  // 浮き窓の中のスクロールでは閉じない・外のスクロールで閉じる
  await open();
  box().querySelector(".wk-pv-body").dispatchEvent(new Event("scroll"));
  await settle();
  assert.ok(P.preview.cur, "浮き窓の中のスクロールで閉じた");
  // 乗せたリンクを含まない箱（チャットの自動追従など）のスクロールでは閉じない
  const other = document.createElement("div");
  document.body.append(other);
  other.dispatchEvent(new Event("scroll"));
  await settle();
  assert.ok(P.preview.cur, "別の箱のスクロールで閉じた");
  other.remove();
  host.dispatchEvent(new Event("scroll"));
  await settle();
  assert.equal(P.preview.cur, null, "乗せたリンクを含む箱のスクロールで閉じない");
  // 乗せたリンクが DOM から消えた = 閉じる（ページの描き直し・遷移）
  const fake = document.createElement("a");
  document.body.append(fake);
  P.hoverStart(fake, { path: "notes/y.md", heading: null, root: "/v", onNavigate: () => {}, readPage });
  assert.ok(await waitFor(() => P.preview.cur?.anchor === fake));
  await settle();
  fake.remove();
  assert.ok(await waitFor(() => !P.preview.cur, 1500), "乗せたリンクが消えても閉じない");
  // タブの切替
  await open();
  const tab0 = S.app.tab;
  S.app.tab = tab0 + 1000;
  await settle();
  assert.equal(P.preview.cur, null, "タブを切り替えても閉じない");
  S.app.tab = tab0;
  await settle();
  // 右クリックのメニュー
  await open();
  a.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, composed: true, clientX: 1, clientY: 1 }));
  await settle();
  assert.ok(LM.linkMenu.cur, "メニューが出ない");
  assert.equal(P.preview.cur, null, "右クリックのメニューで閉じない");
  LM.closeLinkMenu();
  // メニューが出ている間は出さない
  LM.openLinkMenu({ x: 0, y: 0, head: "", items: [{ label: "x", run() {} }] });
  over(link("x"));
  await sleep(60);
  assert.equal(P.preview.cur, null, "右クリックのメニューの上に出した");
  LM.closeLinkMenu();
  // 元のリンクのクリック = 遷移して閉じる
  await open();
  a.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, composed: true }));
  await settle();
  assert.equal(P.preview.cur, null, "リンクで移っても閉じない");
  assert.deepEqual(navs.at(-1), [{ kind: "page", path: "notes/x.md" }, false]);
  await reset();
  tests++;
}

// ---------- ④ まだ無いページ・md 以外・参照フォルダ・押しながら ----------
{
  over(link("gone"));
  assert.ok(await waitFor(() => P.preview.cur), "まだ無いページで出ない");
  await settle();
  assert.equal(P.preview.cur.path, null);
  assert.match(box().textContent, /このページはまだありません/);
  assert.match(box().textContent, /\[\[gone\]\]/, "名前を出さない");
  assert.equal(box().querySelector(".wk-md"), null, "まだ無いページを描いた");
  assert.deepEqual(reads, [], "まだ無いページを読んだ");
  await reset();
  over(link("doc.pdf"));
  await sleep(60);
  assert.equal(P.preview.cur, null, "PDF で出した");
  over(link("x"), { buttons: 1 });
  await sleep(60);
  assert.equal(P.preview.cur, null, "押しながら（選択中）出した");
  // 参照フォルダのページ（root が画面の root と違う）= 出さない
  const rhost = document.createElement("div");
  document.body.append(rhost);
  const rv = mount(ShadowPage, { target: rhost, props: { mode: "md", page: { ...page("r.md", "[[x]]"), links }, root: "/refs", refs: [], onNavigate: () => {}, readPage } });
  flushSync();
  const rsr = () => rhost.querySelector(".wk-md")?.shadowRoot;
  assert.ok(await waitFor(() => rsr()?.querySelector("a")));
  over(rsr().querySelector("a"));
  await sleep(60);
  assert.equal(P.preview.cur, null, "参照フォルダのページで出した");
  unmount(rv);
  rhost.remove();
  await reset();
  tests++;
}

// ---------- ⑤ 浮き窓の中のリンク・題名・中では次のプレビューを出さない ----------
{
  over(link("x"));
  assert.ok(await waitFor(() => inner()?.querySelector("a")), "中のリンクを描かない");
  const y = [...inner().querySelectorAll("a")].find((x) => x.textContent.trim() === "y");
  over(y);
  await sleep(60);
  assert.equal(P.preview.cur.path, "notes/x.md", "浮き窓の中で次のプレビューを出した");
  y.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, composed: true, metaKey: true }));
  await settle();
  assert.deepEqual(navs, [[{ kind: "page", path: "notes/y.md" }, true]], "中のリンクを乗せた側の規則で開かない（⌘ = 新しいタブ）");
  assert.equal(P.preview.cur, null, "中のリンクで移っても閉じない");
  navs.length = 0;
  over(link("x > B"));
  assert.ok(await waitFor(() => box()?.querySelector(".wk-pv-head")));
  box().querySelector(".wk-pv-head").dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 }));
  await settle();
  assert.deepEqual(navs, [[{ kind: "page", path: "notes/x.md", heading: "B" }, false]], "題名のクリックで見出しつきで開かない");
  assert.equal(P.preview.cur, null);
  await reset();
  tests++;
}

// ---------- ⑥ `#見出し` = 浮き窓の中の箱だけを動かす ----------
{
  const proto = HTMLElement.prototype;
  const st = Object.getOwnPropertyDescriptor(Element.prototype, "scrollTop");
  const rect = Element.prototype.getBoundingClientRect;
  Object.defineProperty(proto, "scrollTop", { configurable: true, get() { return this.__st ?? 0; }, set(v) { this.__st = v; } });
  Element.prototype.getBoundingClientRect = function () {
    const top = this.id === "h-B" ? 300 : this.classList?.contains("wk-pv-body") ? 100 : 0;
    return { left: 0, right: 10, top, bottom: top + 10, width: 10, height: 10, x: 0, y: top };
  };
  try {
    over(link("x > B"));
    assert.ok(await waitFor(() => inner()?.getElementById("h-B")), "見出しを描かない");
    const body = box().querySelector(".wk-pv-body");
    assert.ok(await waitFor(() => body.scrollTop === 200), `見出しへ動かない（${body.scrollTop}）`);
    assert.equal(host.scrollTop, 0, "外の箱を動かした");
  } finally {
    delete proto.scrollTop;
    if (st) Object.defineProperty(Element.prototype, "scrollTop", st);
    Element.prototype.getBoundingClientRect = rect;
  }
  await reset();
  tests++;
}

// ---------- ⑦ バックリンク・検索の結果の行・チャットの回答 ----------
{
  W.wk.pages = [
    { path: "notes/x.md", kind: "md", title: "x", tags: [], items: [], mtime: 0, size: 1 },
    { path: "notes/y.md", kind: "md", title: "y", tags: [], items: [], mtime: 0, size: 1 },
  ];
  handlers.wiki_read_gui = (a) => {
    reads.push(a.path);
    return pages[a.path];
  };
  // 検索の結果の行（md の行だけ）
  const { default: Results } = await import("./WikiResults.svelte");
  W.wk.results = [
    { path: "notes/y.md", kind: "md", title: "y", snippet: null, tags: [] },
    { path: "img/a.png", kind: "media", title: "a.png", snippet: null, tags: [] },
  ];
  W.wk.resultsTotal = 2;
  const rt = document.createElement("div");
  document.body.append(rt);
  const res = mount(Results, { target: rt });
  flushSync();
  const rows = rt.querySelectorAll(".wk-hit");
  assert.equal(rows.length, 2);
  rows[1].dispatchEvent(new MouseEvent("mouseenter", { clientX: 5 }));
  await sleep(60);
  assert.equal(P.preview.cur, null, "画像の行で出した");
  rows[1].dispatchEvent(new MouseEvent("mouseleave"));
  rows[0].dispatchEvent(new MouseEvent("mouseenter", { clientX: 5 }));
  assert.ok(await waitFor(() => P.preview.cur?.path === "notes/y.md"), "検索の結果の行で出ない");
  assert.ok(await waitFor(() => inner()?.textContent.includes("yの本文")), "検索の結果の行の浮き窓を描かない");
  rows[0].dispatchEvent(new MouseEvent("mouseleave"));
  assert.ok(await waitFor(() => !P.preview.cur), "行から離れても閉じない");
  unmount(res);
  rt.remove();
  await reset();

  // バックリンク
  const { default: Info } = await import("./WikiInfo.svelte");
  handlers.wiki_backlinks = () => ({ total: 1, pages: [{ path: "notes/y.md", title: "y", excerpt: "" }] });
  W.wk.page = pages["notes/x.md"];
  const it = document.createElement("div");
  document.body.append(it);
  const info = mount(Info, { target: it });
  flushSync();
  assert.ok(await waitFor(() => [...it.querySelectorAll(".wk-link")].some((b) => b.title === "notes/y.md")), "バックリンクを描かない");
  const bl = [...it.querySelectorAll(".wk-link")].find((b) => b.title === "notes/y.md");
  bl.dispatchEvent(new MouseEvent("mouseenter", { clientX: 5 }));
  assert.ok(await waitFor(() => P.preview.cur?.path === "notes/y.md"), "バックリンクで出ない");
  bl.dispatchEvent(new MouseEvent("mouseleave"));
  assert.ok(await waitFor(() => !P.preview.cur));
  unmount(info);
  it.remove();
  W.wk.page = null;
  await reset();

  // チャットの回答の `[[ ]]`（ChatMd = ShadowPage・解決は木）
  const { default: ChatMd } = await import("../chat/ChatMd.svelte");
  const ct = document.createElement("div");
  document.body.append(ct);
  const cm = mount(ChatMd, { target: ct, props: { text: "答え: [[y]] を見てください" } });
  flushSync();
  const csr = () => ct.querySelector(".wk-md")?.shadowRoot;
  assert.ok(await waitFor(() => csr()?.querySelector("a[data-wk-path]")), "チャットの回答のリンクを解かない");
  over(csr().querySelector("a[data-wk-path]"));
  assert.ok(await waitFor(() => P.preview.cur?.path === "notes/y.md"), "チャットの回答で出ない");
  assert.ok(await waitFor(() => inner()?.textContent.includes("yの本文")), "チャットの回答の浮き窓を描かない");
  unmount(cm);
  ct.remove();
  await reset();
  tests++;
}

// ---------- ⑧ 大きなページ（約 400KB・画像 300 枚） ----------
{
  const parts = ["# 大きなページ\n"];
  for (let i = 0; i < 300; i++) {
    parts.push(`\n## 節 ${i}\n\n${"本文の文字。".repeat(200)}\n\n![[img${i}.png]]\n`);
  }
  const big = page("notes/big.md", parts.join(""));
  big.links = Array.from({ length: 300 }, (_, i) => ({ kind: "embed", target: `img${i}.png`, path: `img/img${i}.png`, ambiguous: false }));
  pages["notes/big.md"] = big;
  links.push({ kind: "wiki", target: "big", path: "notes/big.md", ambiguous: false });
  const bhost = document.createElement("div");
  document.body.append(bhost);
  const bv = mount(ShadowPage, { target: bhost, props: { mode: "md", page: { ...page("q.md", "[[big]]"), links }, root: "/v", refs: [], onNavigate: () => {}, readPage } });
  flushSync();
  const bsr = () => bhost.querySelector(".wk-md")?.shadowRoot;
  assert.ok(await waitFor(() => bsr()?.querySelector("a")));
  const t0 = performance.now();
  over(bsr().querySelector("a"));
  assert.ok(await waitFor(() => inner()?.querySelectorAll("img").length === 300, 20000), "大きなページの画像を描かない");
  const ms = Math.round(performance.now() - t0 - P.previewCfg.openDelay);
  const imgs = [...inner().querySelectorAll("img")];
  assert.ok(imgs.every((i) => i.getAttribute("loading") === "lazy"), "浮き窓の中の画像に loading=lazy が無い");
  console.log(`    big page: ${(big.content.length / 1024).toFixed(0)}K chars・img ${imgs.length}・描画 ${ms} ms（jsdom）`);
  unmount(bv);
  bhost.remove();
  await reset();
  tests++;
}

// ---------- ⑨ 浮き窓の中の右クリックのメニュー・画像 ----------
{
  const openX = async () => {
    over(link("x"));
    assert.ok(await waitFor(() => inner()?.querySelectorAll("a[data-wk-path]").length), "中のリンクを描かない");
    await settle();
  };
  const innerY = () => [...inner().querySelectorAll("a")].find((x) => x.textContent.trim() === "y");
  for (const [label, newTab] of [["開く", false], ["新しいタブで開く", true]]) {
    navs.length = 0;
    await openX();
    innerY().dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, composed: true, clientX: 1, clientY: 1 }));
    await settle();
    assert.equal(P.preview.cur, null, "右クリックのメニューで浮き窓を閉じない");
    const it = LM.linkMenu.cur?.items.find((i) => i?.label === label);
    assert.ok(it, `メニューに ${label} が無い`);
    LM.closeLinkMenu();
    await it.run();
    await settle();
    assert.deepEqual(navs, [[{ kind: "page", path: "notes/y.md" }, newTab]], `メニューの ${label} で開かない（浮き窓を閉じた後）`);
  }
  // 画像 = 覆い（閉じた後に props を読んで TypeError にならない）・覆いの [新しいタブで開く] = 乗せた側の onNavigate
  navs.length = 0;
  await openX();
  const errs = [];
  const onErr = (e) => errs.push(String(e?.error ?? e?.message ?? e));
  window.addEventListener("error", onErr);
  inner().querySelector("img").dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, composed: true, button: 0 }));
  await settle();
  window.removeEventListener("error", onErr);
  assert.deepEqual(errs, [], "浮き窓の中の画像でエラー");
  assert.equal(P.preview.cur, null, "画像を押しても浮き窓を閉じない");
  assert.ok(LB.lightbox.cur, "浮き窓の中の画像で覆いを開かない");
  const im = LB.lightbox.cur.images[0];
  assert.deepEqual(im.nav, { kind: "page", path: "img/a.png" });
  await LB.lightbox.cur.onNavigate(im.nav, true);
  assert.deepEqual(navs, [[{ kind: "page", path: "img/a.png" }, true]], "覆いの [新しいタブで開く] が空振り");
  LB.closeLightbox();
  // ⌘クリック = 画像を新しいタブ
  navs.length = 0;
  await openX();
  inner().querySelector("img").dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, composed: true, button: 0, metaKey: true }));
  await settle();
  assert.deepEqual(navs, [[{ kind: "page", path: "img/a.png" }, true]], "浮き窓の中の画像の ⌘クリックで開かない");
  assert.equal(LB.lightbox.cur, null);
  await reset();
  tests++;
}

// ---------- ⑪ 乗せた要素が見えない = 開かない・見えなくなったら閉じる ----------
{
  await reset();
  const req = { path: "notes/y.md", heading: null, root: "/v", onNavigate: () => {}, readPage };
  for (const [prop, value] of [["visibility", "hidden"], ["display", "none"]]) {
    const wrap = document.createElement("div");
    const a = document.createElement("a");
    wrap.append(document.createElement("div"));
    wrap.firstChild.append(a);
    document.body.append(wrap);
    // 隠した祖先の下 = 開かない
    wrap.style[prop] = value;
    P.hoverStart(a, req);
    await sleep(80);
    assert.equal(P.preview.cur, null, `${prop}: ${value} の祖先の下で開いた`);
    P.hoverEnd(a);
    // 開いている間に祖先を隠す = 閉じる
    wrap.style[prop] = "";
    P.hoverStart(a, req);
    assert.ok(await waitFor(() => P.preview.cur?.anchor === a), `見えるのに開かない（${prop}）`);
    await settle();
    wrap.style[prop] = value;
    assert.ok(await waitFor(() => !P.preview.cur, 1500), `開いている間に ${prop}: ${value} にしても閉じない`);
    wrap.remove();
    await reset();
  }
  // shadow の中のリンク（ShadowPage の host の外側の祖先を隠す）
  over(link("x"));
  assert.ok(await waitFor(() => P.preview.cur?.path === "notes/x.md"));
  await settle();
  host.style.visibility = "hidden";
  assert.ok(await waitFor(() => !P.preview.cur, 1500), "shadow の中のリンクの祖先を隠しても閉じない");
  over(link("x"));
  await sleep(80);
  assert.equal(P.preview.cur, null, "祖先を隠したままの shadow の中のリンクで開いた");
  host.style.visibility = "";
  await reset();
  tests++;
}

// ---------- ⑫ アプリのモーダルが開いたら閉じる・開いている間は出さない ----------
{
  await reset();
  over(link("x"));
  assert.ok(await waitFor(() => P.preview.cur));
  await settle();
  S.app.modal = "tags";
  await settle();
  assert.equal(P.preview.cur, null, "モーダルが開いても浮き窓が残る");
  over(link("gone"));
  await sleep(80);
  assert.equal(P.preview.cur, null, "モーダルの間に浮き窓を出した");
  // モーダルの Esc は背後（ここでは window の受け口）へ届く（浮き窓が奪わない）
  let got = 0;
  const bubble = (e) => e.key === "Escape" && got++;
  window.addEventListener("keydown", bubble);
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
  window.removeEventListener("keydown", bubble);
  assert.equal(got, 1, "モーダルの間の Esc を奪った");
  S.app.modal = null;
  await reset();
  tests++;
}

// ---------- ⑩ 見張りの時計は閉じるたびに止まる ----------
{
  await reset();
  await sleep(20);
  assert.equal(ticking.size, 0, `閉じても見張りの時計が残る（${ticking.size}）`);
  tests++;
}

unmount(pv);
unmount(view);
assert.ok(!calls.some((c) => c.name === "set_ui_pref"), "プレビューが設定に書いた");
console.log(`wiki preview: ok (${tests} tests)`);
process.exit(0);
