/* 読み手の描き方の試験（node app/src/lib/shelf/viewer.test.mjs・実部品 ShelfViewer.svelte + jsdom）。pdf.js と IntersectionObserver は偽物 = 頼まれた render と打ち切りを数える。
   見るのは帯の出入りと後始末: 帯を出たページの描画は打ち切る（worker は頼まれた順に処理する = 打ち切らないと止まったページが通り過ぎた全部の後に回る・実機で測った）・
   bitmap を返す・page.cleanup・倍率の変更・閉じる。字や画像が出るかは実機で見る */
import assert from "node:assert/strict";
import { setupHarness } from "../wiki/test-harness.mjs";

const { dom, handlers, sleep, waitFor } = setupHarness({
  fakes: {
    "pdfjs-dist": "export const GlobalWorkerOptions = {}; export class PDFDataRangeTransport { abort() {} onDataRange() {} } export const getDocument = (o) => globalThis.__pdf(o);",
    "pdfjs-dist/build/pdf.worker.mjs?url": "export default 'worker.js';",
  },
});
handlers.set_ui_pref = () => null;
dom.window.HTMLCanvasElement.prototype.getContext = () => ({});
dom.window.Element.prototype.scrollTo = () => {};
// jsdom に配置は無い（clientWidth = 0）= 幅合わせの倍率が負になる。倍率を比べる試験のために幅を与える（幅の変化の試験は resize() で変える）
let clientW = 1000;
Object.defineProperty(dom.window.HTMLElement.prototype, "clientWidth", { get: () => clientW, configurable: true });
/** 張られた ResizeObserver（resize() が幅を変えて callback を呼ぶ。外したものは呼ばない） */
const ros = [];
globalThis.ResizeObserver = class {
  constructor(f) {
    this.f = f;
    ros.push(this);
  }
  observe() {}
  disconnect() {
    this.off = true;
  }
};
const resize = (w) => {
  clientW = w;
  for (const r of ros) if (!r.off) r.f([]);
};
globalThis.IntersectionObserver = class {
  constructor(f) {
    globalThis.__io = f;
  }
  observe() {}
  unobserve() {}
  disconnect() {}
};
globalThis.fetch = async () => new Response(new Uint8Array(1), { status: 206, headers: { "content-range": "bytes 0-0/1000" } });

/** 頼まれた render の記録（来た順）。finish で描き終わる・cancel で打ち切られる */
let renders = [];
/** Promise = getPage の応答を止める */
let gate = null;
const pages = new Map();
const page = (n) => {
  if (!pages.has(n))
    pages.set(n, {
      cleaned: 0,
      getViewport: ({ scale }) => ({ width: 600 * scale, height: 800 * scale }),
      cleanup() {
        this.cleaned++;
      },
      render({ viewport }) {
        const task = { n, viewport, cancelled: 0 };
        task.promise = new Promise((ok, no) => {
          task.finish = ok;
          task.fail = no;
          task.cancel = () => {
            task.cancelled++;
            no(Object.assign(new Error("cancelled"), { name: "RenderingCancelledException" }));
          };
        });
        task.promise.catch(() => {});
        renders.push(task);
        return task;
      },
    });
  return pages.get(n);
};
let destroyed = 0;
globalThis.__pdf = () => ({
  promise: Promise.resolve({
    numPages: 20,
    getPage: async (n) => {
      await gate;
      return page(n);
    },
  }),
  destroy: async () => {
    destroyed++;
  },
});

const { mount, unmount } = await import("svelte");
const { app } = await import("../state.svelte.ts");
const { default: ShelfViewer } = await import("./ShelfViewer.svelte");
app.detailOpen = false;

const el = (n) => document.querySelector(`.sh-v-page[data-page="${n}"]`);
const band = (n, on) => globalThis.__io([{ target: el(n), isIntersecting: on }]);
const tickle = () => sleep(1);
const zoomIn = () => window.dispatchEvent(new KeyboardEvent("keydown", { metaKey: true, key: "=", bubbles: true, cancelable: true }));
const book = { root: "/r", id: "b1", path: "/r/本.pdf", title: "本", size: 1000, mtime: 1 };

async function open(props = { front: true }) {
  renders = [];
  gate = null;
  pages.clear();
  document.body.replaceChildren();
  const v = mount(ShelfViewer, { target: document.body, props: Object.defineProperties({ book, onClose() {} }, Object.getOwnPropertyDescriptors(props)) });
  await waitFor(() => el(20));
  assert.ok(el(20), "枠が出ない");
  return v;
}
/** この間に作られた canvas を集める */
function trackCanvas() {
  const made = [];
  const real = document.createElement.bind(document);
  document.createElement = (t, o) => {
    const e = real(t, o);
    if (t === "canvas") made.push(e);
    return e;
  };
  return { made, stop: () => (document.createElement = real) };
}
let tests = 0;

// 描いて枠に入る → 帯を出ると bitmap を返して page.cleanup
{
  const v = await open();
  band(1, true);
  await waitFor(() => renders.length);
  renders[0].finish();
  await waitFor(() => el(1).querySelector("canvas"));
  const canvas = el(1).querySelector("canvas");
  assert.ok(canvas.width > 0, "canvas に幅が無い");
  band(1, false);
  await tickle();
  assert.deepEqual([el(1).children.length, canvas.width, page(1).cleaned], [0, 0, 1], "帯を出たページを片付けない");
  assert.equal(renders[0].cancelled, 0, "描き終わった描画を打ち切った");
  unmount(v);
  tests++;
}

// 描いている途中で帯を出る = 打ち切る・canvas の bitmap を返す・入り直せば描き直す（印が残らない）
{
  const v = await open();
  const { made, stop } = trackCanvas();
  band(5, true);
  await waitFor(() => renders.length);
  band(5, false);
  await tickle();
  stop();
  assert.equal(renders[0].cancelled, 1, "帯を出たページの描画を打ち切らない");
  assert.equal(el(5).children.length, 0, "打ち切ったページに canvas が入った");
  assert.deepEqual(made.map((c) => c.width), [0], "打ち切った canvas の bitmap を返さない");
  await waitFor(() => page(5).cleaned);
  assert.equal(page(5).cleaned, 1, "打ち切ったページの cleanup が無い");
  band(5, true);
  await waitFor(() => renders.length === 2);
  assert.equal(renders.length, 2, "入り直したページを描かない");
  renders[1].finish();
  await waitFor(() => el(5).querySelector("canvas"));
  assert.ok(el(5).querySelector("canvas"), "入り直したページが枠に入らない");
  unmount(v);
  tests++;
}

// 飛ぶ: 通り過ぎたページは全部打ち切られ、止まったページだけが生きている
{
  const v = await open();
  for (const n of [2, 3, 4]) band(n, true);
  await waitFor(() => renders.length === 3);
  for (const n of [2, 3, 4]) band(n, false);
  band(9, true);
  await waitFor(() => renders.length === 4);
  assert.deepEqual(renders.map((t) => [t.n, t.cancelled]), [[2, 1], [3, 1], [4, 1], [9, 0]]);
  unmount(v);
  tests++;
}

// getPage を待つ間に帯を出た = render を頼まない
{
  const v = await open();
  let go;
  gate = new Promise((r) => (go = r));
  band(7, true);
  await tickle();
  band(7, false);
  go();
  gate = null;
  await sleep(5);
  assert.equal(renders.length, 0, "帯を出たページの render を頼んだ");
  unmount(v);
  tests++;
}

// 倍率を変える = 途中の描画を打ち切って新しい倍率で頼む・古い描画の終わりが新しい印を消さない（同じページを 2 度頼まない）
{
  const v = await open();
  band(3, true);
  await waitFor(() => renders.length);
  const w0 = renders[0].viewport.width;
  zoomIn();
  await waitFor(() => renders.length === 2);
  assert.equal(renders[0].cancelled, 1, "倍率の変更で途中の描画を打ち切らない");
  assert.ok(w0 > 0, "幅合わせの倍率が負");
  assert.ok(Math.abs(renders[1]?.viewport.width / w0 - 1.15) < 1e-9, "新しい倍率で頼んでいない");
  await sleep(5);
  band(3, true);
  await sleep(5);
  assert.equal(renders.length, 2, "描いている途中のページをもう 1 度頼んだ");
  unmount(v);
  tests++;
}

// getPage を待つ間に倍率が変わる = 古い方の終わりが新しい印を消さない
{
  const v = await open();
  let go;
  gate = new Promise((r) => (go = r));
  band(3, true);
  await tickle();
  zoomIn();
  await sleep(5);
  go();
  gate = null;
  await waitFor(() => renders.length);
  band(3, true);
  await sleep(5);
  assert.equal(renders.length, 1, "古い方の終わりが新しい印を消した");
  assert.ok(Math.abs(renders[0].viewport.width / 904 - 1.15) < 1e-9, "待っていた描画が古い倍率のまま");
  unmount(v);
  tests++;
}

// 同じ callback に [出た, 入った]（主 thread が重いと IntersectionObserver は積んで届ける）= 描き直す・白いまま残らない
{
  const v = await open();
  band(5, true);
  await waitFor(() => renders.length);
  globalThis.__io([{ target: el(5), isIntersecting: false }, { target: el(5), isIntersecting: true }]);
  await waitFor(() => renders.length === 2);
  assert.deepEqual(renders.map((t) => [t.n, t.cancelled]), [[5, 1], [5, 0]], "同じ callback の往復で描き直さない");
  renders[1].finish();
  await waitFor(() => el(5).querySelector("canvas"));
  assert.ok(el(5).querySelector("canvas"), "往復したページが枠に入らない");
  unmount(v);
  tests++;
}

// getPage を待つ間の往復 = 1 回だけ描く
{
  const v = await open();
  let go;
  gate = new Promise((r) => (go = r));
  band(5, true);
  await tickle();
  globalThis.__io([{ target: el(5), isIntersecting: false }, { target: el(5), isIntersecting: true }]);
  await tickle();
  go();
  gate = null;
  await waitFor(() => renders.length);
  await sleep(5);
  assert.equal(renders.length, 1, "getPage を待つ間の往復で 2 度頼んだ");
  unmount(v);
  tests++;
}

// task.cancel() が throw する = 例外を外へ出さない・描き終えた canvas を帯の外に残さない・印が残らない
{
  const v = await open();
  const warn = console.warn;
  console.warn = () => {};
  const { made, stop } = trackCanvas();
  band(5, true);
  await waitFor(() => renders.length);
  renders[0].cancel = () => {
    throw new Error("cancel failed");
  };
  band(5, false);
  renders[0].finish();
  await sleep(5);
  stop();
  console.warn = warn;
  assert.equal(el(5).children.length, 0, "帯の外のページに canvas が残った");
  assert.deepEqual(made.map((c) => c.width), [0], "cancel が throw した canvas の bitmap を返さない");
  band(5, true);
  await waitFor(() => renders.length === 2);
  assert.equal(renders.length, 2, "cancel が throw した後に入り直しても描かない");
  unmount(v);
  tests++;
}

// cancel が throw した古い描画の終わりが、入り直したページの描画の記録を消さない（次に帯を出たら打ち切れる）
{
  const v = await open();
  const warn = console.warn;
  console.warn = () => {};
  band(5, true);
  await waitFor(() => renders.length);
  renders[0].cancel = () => {
    throw new Error("cancel failed");
  };
  band(5, false);
  band(5, true);
  await waitFor(() => renders.length === 2);
  renders[0].finish();
  await sleep(5);
  console.warn = warn;
  assert.equal(el(5).children.length, 0, "古い描画の canvas が枠に入った");
  band(5, false);
  assert.equal(renders[1].cancelled, 1, "入り直したページの描画を打ち切れない");
  unmount(v);
  tests++;
}

// cancel が throw しても倍率が変わった後の古い描画は枠に入らない
{
  const v = await open();
  const warn = console.warn;
  console.warn = () => {};
  band(3, true);
  await waitFor(() => renders.length);
  renders[0].cancel = () => {
    throw new Error("cancel failed");
  };
  zoomIn();
  await waitFor(() => renders.length === 2);
  renders[0].finish();
  await sleep(5);
  console.warn = warn;
  assert.equal(el(3).children.length, 0, "古い倍率の canvas が枠に入った");
  unmount(v);
  tests++;
}

// 帯を出たページは倍率を変えても描かない
{
  const v = await open();
  band(3, true);
  await waitFor(() => renders.length);
  renders[0].finish();
  await waitFor(() => el(3).querySelector("canvas"));
  band(3, false);
  zoomIn();
  await sleep(5);
  assert.equal(renders.length, 1, "帯の外のページを描いた");
  unmount(v);
  tests++;
}

// 隠れている間は描かない・前面に戻ると帯を描く・失敗したページも頼み直す（front は app の値を読む getter = 親の state と同じ）
{
  app.dropActive = true;
  const v = await open({
    get front() {
      return !app.dropActive;
    },
  });
  band(5, true);
  await sleep(5);
  assert.equal(renders.length, 0, "隠れている間に描いた");
  app.dropActive = false;
  await waitFor(() => renders.length);
  assert.equal(renders.length, 1, "前面に戻っても描かない");
  renders[0].fail(new Error("broken page"));
  await waitFor(() => el(5).classList.contains("sh-v-failed"));
  app.dropActive = true;
  await sleep(5);
  app.dropActive = false;
  await waitFor(() => renders.length === 2);
  assert.equal(renders.length, 2, "失敗したページの印が残って頼み直せない");
  unmount(v);
  tests++;
}

// 幅の変化（窓・右ペイン・チャットの柱の開閉）: 隠れている（前面でない）間は描き直さない = 描いた絵も捨てない（柱の開閉 1 回で開いている本が
// 全部描き直さない）・前面に戻ると 1 回だけ合わせ直す・前面なら今までどおり 150 ms 後に 1 回・待つ間に隠れたら合わせ直さずに印だけ
{
  app.dropActive = false;
  const v = await open({
    get front() {
      return !app.dropActive;
    },
  });
  band(3, true);
  await waitFor(() => renders.length);
  renders[0].finish();
  await waitFor(() => el(3).querySelector("canvas"));
  const w0 = renders[0].viewport.width;
  app.dropActive = true; // 隠れる
  await sleep(5);
  resize(800);
  await sleep(200); // 150 ms の待ちを越える
  resize(700); // 2 度目の変化も印だけ
  await sleep(200);
  assert.equal(renders.length, 1, "隠れている間に幅が変わって描き直した");
  assert.ok(el(3).querySelector("canvas"), "隠れている間に幅が変わって描いた絵を捨てた");
  app.dropActive = false; // 前面に戻る = 1 回だけ合わせ直す
  await waitFor(() => renders.length === 2);
  await sleep(200);
  assert.equal(renders.length, 2, "前面に戻っても合わせ直さない / 2 度以上合わせ直した");
  assert.ok(renders[1].viewport.width < w0, "前面に戻っても新しい幅で合わせ直さない");
  renders[1].finish();
  await waitFor(() => el(3).querySelector("canvas"));
  // 前面なら今までどおり（150 ms 後に 1 回）
  resize(900);
  await sleep(200);
  assert.equal(renders.length, 3, "前面で幅が変わっても合わせ直さない");
  renders[2].finish();
  await waitFor(() => el(3).querySelector("canvas"));
  // 待つ間（150 ms）に隠れた = 合わせ直さずに印だけ・戻ると 1 回
  resize(600);
  await sleep(20);
  app.dropActive = true;
  await sleep(200);
  assert.equal(renders.length, 3, "待つ間に隠れたのに合わせ直した");
  assert.ok(el(3).querySelector("canvas"), "待つ間に隠れたのに描いた絵を捨てた（隠れたまま合わせ直した）");
  app.dropActive = false;
  await waitFor(() => renders.length === 4);
  await sleep(200);
  assert.equal(renders.length, 4, "待つ間に隠れて戻っても 1 回だけ合わせ直さない");
  // 前面に戻っても幅が変わっていなければ合わせ直さない（隠れて戻るだけ）
  renders[3].finish();
  await waitFor(() => el(3).querySelector("canvas"));
  app.dropActive = true;
  await sleep(5);
  app.dropActive = false;
  await sleep(200);
  assert.equal(renders.length, 4, "幅が変わっていないのに前面に戻って合わせ直した");
  clientW = 1000;
  unmount(v);
  tests++;
}

// 打ち切りでない失敗 = 印を付けて頼み直さない（無限に頼まない）・帯を出て入り直せば頼む
{
  const v = await open();
  band(5, true);
  await waitFor(() => renders.length);
  renders[0].fail(new Error("broken page"));
  await waitFor(() => el(5).classList.contains("sh-v-failed"));
  await sleep(5);
  assert.ok(el(5).classList.contains("sh-v-failed"), "失敗の印が無い");
  assert.equal(renders.length, 1, "失敗したページを頼み直した");
  band(5, false);
  band(5, true);
  await waitFor(() => renders.length === 2);
  assert.equal(renders.length, 2, "失敗したページに入り直しても頼まない");
  unmount(v);
  tests++;
}

// getPage を待つ間に閉じる = render を頼まない
{
  const v = await open();
  let go;
  gate = new Promise((r) => (go = r));
  band(5, true);
  await tickle();
  unmount(v);
  go();
  gate = null;
  await sleep(5);
  assert.equal(renders.length, 0, "閉じた後に render を頼んだ（getPage の待ち）");
  tests++;
}

// 閉じた後に IntersectionObserver の通知が来る = 頼まない・例外を出さない
{
  const v = await open();
  const e = el(2);
  unmount(v);
  globalThis.__io([{ target: e, isIntersecting: true }]);
  await sleep(5);
  assert.equal(renders.length, 0, "閉じた後に render を頼んだ");
  tests++;
}

// 閉じる = 途中の描画を打ち切る・文書を閉じる・例外を出さない（落とし穴 = onDestroy の例外で次が開けない）
{
  const v = await open();
  band(1, true);
  await waitFor(() => renders.length);
  const d0 = destroyed;
  unmount(v);
  assert.equal(renders[0].cancelled, 1, "閉じても描画を打ち切らない");
  assert.equal(destroyed, d0 + 1, "文書を閉じない");
  await tickle();
  const v2 = await open();
  band(1, true);
  await waitFor(() => renders.length);
  assert.equal(renders.length, 1, "次の本が描けない");
  unmount(v2);
  tests++;
}

// Esc: 上のモーダルが同じ Esc で閉じても本のタブは閉じない（Settings の移行の窓はどのタブの上にも開く）。
// モーダルの window keydown は App と一緒に先に登録される = ShelfViewer の onKey より先に走って app.modal を null にする
{
  let closed = 0;
  const esc = () => window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
  const modalEsc = (e) => {
    if (app.modal && e.key === "Escape") app.modal = null; // MigrationModal の <svelte:window onkeydown> と同じ = 閉じるだけ
  };
  window.addEventListener("keydown", modalEsc);
  const v = await open({ front: true, onClose: () => closed++ });
  app.modal = "migration";
  esc();
  assert.equal(app.modal, null, "前提: モーダルの Esc が先に閉じる");
  assert.equal(closed, 0, "モーダルを閉じた Esc で本のタブまで閉じた");
  esc(); // モーダルの無い Esc = 今どおり閉じる（対照）
  assert.equal(closed, 1, "モーダルの無い Esc で閉じない");
  window.removeEventListener("keydown", modalEsc);
  unmount(v);
  tests++;
}

console.log(`shelf viewer: ok (${tests} tests)`);
process.exit(0);
