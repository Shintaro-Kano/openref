/* 窓の中に Tab の焦点を留める（node app/src/lib/focustrap.test.mjs・lib/focusTrap.ts + jsdom）。
   確かめること: 最後 → 最初・Shift で最初 → 最後・真ん中も囲いが自分で次 / 前へ動かす（ブラウザの Tab の順に頼らない = WKWebView は
   フルキーボードアクセスが切だとボタンに止まらない）・入力欄 / select から出る Tab も次へ・焦点が窓の外 / 枠そのもの → 端へ・
   押せる物 0 = 枠に留まる・disabled / tabindex=-1 / hidden / inert / 見えない物を飛ばす・radio の組は 1 つ・窓の中のメニュー / listbox は
   窓の順の一部（閉じ込めない）・重なった窓は画面でいちばん上（z・同じなら DOM で後ろ）だけ・外したら（閉じたら）囲わない・
   ⌘ / Ctrl / ⌥ の組・IME の変換中・既に扱われた Tab は素通し。
   実部品の窓（WikiDialogs の新規ページ = 骨格 Modal・WikiRescue / WikiConflict / WikiCite = 自前の枠に trapTab）は edit.test ㊳ */
import assert from "node:assert/strict";
import { setupHarness } from "./wiki/test-harness.mjs";

// 移行の窓（MigrationModal）は plugin-dialog の save も読む
setupHarness({ fakes: { "@tauri-apps/plugin-dialog": "export const open = async () => null; export const save = async () => null;" } });
const { holdTab, moveTab, tabbables } = await import("./focusTrap.ts");
let tests = 0;

const make = (html) => {
  const d = document.createElement("div");
  d.className = "frame";
  d.innerHTML = html;
  document.body.append(d);
  return d;
};
const tab = (shift = false, init = {}) => {
  const e = new KeyboardEvent("keydown", { key: "Tab", shiftKey: shift, bubbles: true, cancelable: true, ...init });
  (document.activeElement ?? document.body).dispatchEvent(e);
  return e;
};
const id = () => document.activeElement?.id || document.activeElement?.className || document.activeElement?.tagName;

// ① 端で回る・真ん中は任せる・外 → 端
{
  const outside = make('<button id="out">外</button>').querySelector("#out");
  const f = make('<button id="a">a</button><input id="b"><select id="c"><option>1</option></select><button id="d">d</button>');
  const off = holdTab(f);
  f.querySelector("#d").focus();
  let e = tab();
  assert.equal(id(), "a", "最後で Tab → 最初へ回らない");
  assert.ok(e.defaultPrevented);
  e = tab(true);
  assert.equal(id(), "d", "最初で Shift+Tab → 最後へ回らない");
  // 真ん中も囲いが動かす（既定を止めて次 / 前へ）= 入力欄・select から出る Tab も次の物へ
  f.querySelector("#a").focus();
  e = tab();
  assert.ok(e.defaultPrevented && id() === "b", `真ん中の Tab（ボタン → 入力欄）を囲いが動かさない: ${id()}`);
  e = tab();
  assert.ok(e.defaultPrevented && id() === "c", `入力欄から出る Tab が次（select）へ行かない: ${id()}`);
  e = tab(true);
  assert.ok(e.defaultPrevented && id() === "b", `select から Shift+Tab が前（入力欄）へ行かない: ${id()}`);
  f.querySelector("#c").focus();
  e = tab();
  assert.ok(e.defaultPrevented && id() === "d", `select から出る Tab が次へ行かない: ${id()}`);
  outside.focus();
  tab();
  assert.equal(id(), "a", "窓の外の焦点から Tab で窓の最初へ入らない");
  outside.focus();
  tab(true);
  assert.equal(id(), "d", "窓の外の焦点から Shift+Tab で窓の最後へ入らない");
  // 枠そのもの（tabindex=-1 の枠・中の文）= 文書の順で次 / 前
  f.setAttribute("tabindex", "-1");
  f.focus();
  tab();
  assert.equal(id(), "a", "枠から Tab で最初へ");
  f.focus();
  tab(true);
  assert.equal(id(), "d", "枠から Shift+Tab で最後へ（枠の前 = 窓の外へ出ない）");
  // 素通し: ⌘ / Ctrl / ⌥・IME の変換中・既に扱われた Tab
  f.querySelector("#d").focus();
  for (const init of [{ metaKey: true }, { ctrlKey: true }, { altKey: true }, { isComposing: true }]) {
    e = tab(false, init);
    assert.equal([e.defaultPrevented, id()].join(), "false,d", `素通しの Tab を奪った: ${JSON.stringify(init)}`);
  }
  const pre = new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true });
  f.querySelector("#d").addEventListener("keydown", (ev) => ev.preventDefault(), { once: true });
  f.querySelector("#d").dispatchEvent(pre);
  assert.equal(id(), "d", "中の部品が扱った Tab（CodeMirror の字下げ等）で回った");
  // 外したら（閉じたら）囲わない
  off();
  f.querySelector("#d").focus();
  e = tab();
  assert.equal(e.defaultPrevented, false, "外した後も囲った");
  f.remove();
  outside.parentElement.remove();
  tests++;
}

// ② 押せない物を飛ばす・radio の組・押せる物 0 = 枠に留まる
{
  const f = make(
    '<button id="a">a</button><button id="dis" disabled>x</button><button id="neg" tabindex="-1">x</button><div hidden><button id="hid">x</button></div>' +
      '<div inert><button id="inert">x</button></div><button id="none" style="display:none">x</button><span style="visibility:hidden"><button id="vis">x</button></span>' +
      '<input type="radio" name="g" id="r1"><input type="radio" name="g" id="r2" checked><input type="radio" name="g" id="r3"><button id="z">z</button>',
  );
  assert.deepEqual(tabbables(f).map((e) => e.id), ["a", "r2", "z"], "押せない物を飛ばさない / radio の組が 1 つでない");
  const off = holdTab(f);
  f.querySelector("#z").focus();
  tab();
  assert.equal(id(), "a");
  off();
  f.remove();
  const empty = make("<p>文だけ</p>");
  const out = make('<button id="o">o</button>').querySelector("#o");
  const off2 = holdTab(empty);
  out.focus();
  const e = tab();
  assert.ok(e.defaultPrevented && document.activeElement === empty, `押せる物 0 の窓で枠に留まらない: ${id()}`);
  tab(true);
  assert.ok(document.activeElement === empty, "押せる物 0 の窓で Shift+Tab が外へ出た");
  off2();
  empty.remove();
  out.parentElement.remove();
  tests++;
}

// ②b <fieldset disabled>（.disabled は false のまま = :disabled で見る）: 中の物・入れ子の中は飛ばす・最初の <legend> の中は押せる。
//     focus() が効かなかった物（判定の漏れ）は飛ばして次へ・全部効かなければ 1 周で止める（Tab が窓の中で止まらない・無限に回らない）
{
  const f = make(
    '<button id="a">a</button><fieldset disabled><legend><button id="lg">lg</button></legend><button id="fb">x</button><input id="fi">' +
      '<select id="fs"><option>1</option></select><fieldset><button id="nest">x</button></fieldset></fieldset><button id="z">z</button>',
  );
  assert.deepEqual(tabbables(f).map((e) => e.id), ["a", "lg", "z"], "<fieldset disabled> の中を飛ばさない / legend の中を飛ばした");
  const off = holdTab(f);
  f.querySelector("#lg").focus();
  tab();
  assert.equal(id(), "z", `legend の中から Tab が fieldset の中に止まろうとした: ${id()}`);
  tab(true);
  assert.equal(id(), "lg", `Shift+Tab が legend へ戻らない: ${id()}`);
  // 保険: 押せる物に数えたが focus() が効かない物（判定の漏れの代わり）= 飛ばして次へ
  const lg = f.querySelector("#lg");
  lg.focus = () => {};
  f.querySelector("#a").focus();
  tab();
  assert.equal(id(), "z", `focus() が効かない物の所で Tab が止まった（飛ばして次へ行かない）: ${id()}`);
  tab(true);
  assert.equal(id(), "a", `focus() が効かない物の所で Shift+Tab が止まった: ${id()}`);
  off();
  f.remove();
  // 全部効かない = 1 周で止める（無限に回らない）・焦点が窓の外なら枠へ
  const g = make('<button id="g1">g1</button><button id="g2">g2</button>');
  for (const b of g.querySelectorAll("button")) b.focus = () => {};
  const out = make('<button id="o2">o</button>').querySelector("#o2");
  const off2 = holdTab(g);
  out.focus();
  const e = tab();
  assert.ok(e.defaultPrevented && document.activeElement === g, `押せる物の focus() が全部効かない窓で枠に留まらない: ${id()}`);
  off2();
  g.remove();
  out.parentElement.remove();
  tests++;
}

// ③ 窓の中のメニュー / listbox（role="menu" / "listbox"）は窓の順の一部 = Tab で出入りできる（新規ラベルの色の見本・引用の結果の行）・
//    重なった窓は画面でいちばん上だけ
{
  const f = make('<button id="a">a</button><div role="listbox"><button id="m1">m1</button><button id="m2">m2</button></div><button id="z">z</button>' +
    '<div role="listbox"><div role="option" id="row" tabindex="-1">行</div></div>');
  const off = holdTab(f);
  f.querySelector("#m2").focus();
  tab();
  assert.equal(id(), "z", "窓の中の listbox の最後で Tab → 窓の次の物へ出ない（listbox に閉じ込めた）");
  f.querySelector("#m1").focus();
  tab(true);
  assert.equal(id(), "a", "窓の中の listbox の最初で Shift+Tab → 窓の前の物へ出ない");
  f.querySelector("#row").focus(); // roving の行（tabindex=-1）= 文書の順で前 / 次
  tab(true);
  assert.equal(id(), "z", "listbox の行（tabindex=-1）から Shift+Tab → 前の押せる物へ行かない");
  // 上に 2 枚目の窓（名前を変更 → 書き換えの確認 の型）= 上の窓の中だけ
  const top = make('<button id="t1">t1</button><button id="t2">t2</button>');
  const offTop = holdTab(top);
  top.querySelector("#t2").focus();
  tab();
  assert.equal(id(), "t1", "上の窓の中を回らない");
  f.querySelector("#z").focus(); // 下の窓に焦点が残っていても、囲うのは上の窓
  tab();
  assert.equal(id(), "t1", "下の窓の中を回った（上の窓が囲っていない）");
  offTop();
  top.remove();
  f.querySelector("#z").focus();
  tab();
  assert.equal(id(), "a", "上の窓を閉じた後、下の窓が囲わない");
  // 後から開いたが DOM で前にある窓（参照エディタの上に PDF 取込を開いた型）= 同じ z なら DOM で後ろ（画面で上に見える）の窓が囲う
  const under = document.createElement("div");
  under.innerHTML = '<button id="u1">u1</button><button id="u2">u2</button>';
  document.body.prepend(under);
  const offUnder = holdTab(under);
  f.querySelector("#z").focus();
  tab();
  assert.equal(id(), "a", "後から開いた DOM で前の窓（画面で下に隠れる）を囲った");
  // z-index が大きい窓は DOM の順に関わらず上
  under.style.cssText = "position:fixed;z-index:50";
  f.style.cssText = "position:fixed;z-index:40";
  f.querySelector("#z").focus();
  tab();
  assert.equal(id(), "u1", "z-index の大きい窓を囲わない");
  offUnder();
  under.remove();
  f.style.cssText = "";
  off();
  f.remove();
  tests++;
}

// ④ moveTab を直に（WikiLightbox・WikiCite は自前の受け手から呼ぶ）: 真ん中も動かす = 既定を止めて次へ
{
  const f = make('<button id="a">a</button><button id="b">b</button><button id="c">c</button>');
  f.querySelector("#b").focus();
  const e = new KeyboardEvent("keydown", { key: "Tab", cancelable: true });
  assert.equal(moveTab(e, f), true, "真ん中で動かさない");
  assert.ok(e.defaultPrevented && id() === "c", `真ん中の Tab で次へ行かない: ${id()}`);
  f.querySelector("#c").focus();
  const e2 = new KeyboardEvent("keydown", { key: "Tab", cancelable: true });
  assert.equal(moveTab(e2, f), true);
  assert.equal(id(), "a");
  f.remove();
  tests++;
}

// ⑤ 実部品: 新規ラベルの色の見本（role=listbox・25 個）から Tab で出られる・参照エディタの上に PDF 取込が DOM の前に開いた
//    （App の並び = ImportModal の後に ReferenceEditor）= 画面で上の参照エディタを囲う（隠れた取込の [中止] に留まらない）
{
  const { mount, unmount, flushSync, tick } = await import("svelte");
  const S = await import("./state.svelte.ts");
  const settle = async () => {
    for (let i = 0; i < 4; i++) {
      await tick();
      flushSync();
    }
  };
  const { default: TagModals } = await import("./TagModals.svelte");
  const t1 = document.createElement("div");
  document.body.append(t1);
  const tm = mount(TagModals, { target: t1 });
  S.app.modal = "newLabel";
  await settle();
  const frame = t1.querySelector(".overlay .modal");
  assert.ok(frame, "新規ラベルの窓が出ない");
  frame.querySelector('button[aria-label="色を選ぶ"]').click(); // 色の見本を開く
  await settle();
  const sw = [...frame.querySelectorAll('[role="listbox"] button')];
  assert.ok(sw.length >= 2, `色の見本が無い: ${sw.length}`);
  const list = tabbables(frame);
  sw.at(-1).focus();
  tab();
  assert.ok(!sw.includes(document.activeElement) && document.activeElement === list[(list.indexOf(sw.at(-1)) + 1) % list.length],
    `最後の色の見本から Tab で見本の外（窓の次の物）へ出ない: ${id()}`);
  sw[0].focus();
  tab(true);
  assert.ok(!sw.includes(document.activeElement), `最初の色の見本から Shift+Tab で見本の外へ出ない: ${id()}`);
  S.app.modal = null;
  await settle();
  unmount(tm);
  t1.remove();

  // 移行の窓の作業中（frozen = <fieldset disabled>・summary から Tab が動かなかった前例）: Tab / Shift+Tab が
  // 窓の中の押せる物（fieldset の外）を回る
  const { default: MigrationModal } = await import("./MigrationModal.svelte");
  const tm2 = document.createElement("div");
  document.body.append(tm2);
  const mm = mount(MigrationModal, { target: tm2 });
  S.app.modal = "migration";
  await settle();
  const mf = tm2.querySelector(".overlay .modal");
  assert.ok(mf && mf.querySelector("fieldset") && mf.querySelector("summary"), "移行の窓の fieldset / summary が無い（試験の前提）");
  mf.querySelector("fieldset").setAttribute("disabled", "");
  const ml = tabbables(mf);
  assert.ok(ml.length >= 2 && ml.every((el) => !el.closest("fieldset[disabled]") || el.closest("legend")), `移行の窓の押せる物に fieldset の中が入った: ${ml.map((e) => e.tagName)}`);
  mf.querySelector("summary").focus();
  const seen = new Set();
  for (let k = 0; k < ml.length + 1; k++) {
    tab();
    assert.ok(mf.contains(document.activeElement) && ml.includes(document.activeElement), `移行の窓（作業中）で Tab が押せる物を回らない: ${id()}`);
    seen.add(document.activeElement);
  }
  assert.equal(seen.size, ml.length, "移行の窓（作業中）で Tab が全部の押せる物を巡らない（止まった）");
  seen.clear();
  for (let k = 0; k < ml.length + 1; k++) {
    tab(true);
    seen.add(document.activeElement);
  }
  assert.equal(seen.size, ml.length, "移行の窓（作業中）で Shift+Tab が全部の押せる物を巡らない（止まった）");
  S.app.modal = null;
  await settle();
  unmount(mm);
  tm2.remove();

  const { default: ImportModal } = await import("./ImportModal.svelte");
  const { default: ReferenceEditor } = await import("./ReferenceEditor.svelte");
  const ti = document.createElement("div");
  const te = document.createElement("div");
  document.body.append(ti, te); // App の並び = 取込が前・参照エディタが後
  S.openEditorNew();
  const re = mount(ReferenceEditor, { target: te });
  await settle();
  const im = mount(ImportModal, { target: ti });
  S.app.importJob = { files: [{ path: "/x/a.pdf", name: "a.pdf", status: "pending", message: "", key: null }], running: true, cancel: false };
  await settle();
  const ed = te.querySelector(".overlay .modal");
  assert.ok(ed && ti.querySelector(".overlay .modal"), "参照エディタと取込の窓が両方出ない（試験の前提）");
  const el = tabbables(ed);
  el.at(-1).focus();
  tab();
  assert.ok(document.activeElement === el[0], `重なった窓で、画面で上の参照エディタを囲わない（隠れた取込を囲った）: ${id()}`);
  ti.querySelector(".overlay button")?.focus(); // 隠れた取込に焦点が残っていても、Tab は上の参照エディタへ
  tab();
  assert.ok(ed.contains(document.activeElement), `隠れた取込の窓の焦点から Tab で上の窓へ入らない: ${id()}`);
  S.app.importJob = null;
  S.app.editorOpen = false;
  await settle();
  unmount(im);
  unmount(re);
  ti.remove();
  te.remove();
  tests++;
}

console.log(`focus trap: ok (${tests} tests)`);
process.exit(0);
