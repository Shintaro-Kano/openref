/* OpenNote の実部品テスト（Svelte 5 の実部品 + jsdom）の共通の土台（edit / chat / mediaview / ime / ipc が同じ物を使う）。
   使い方: 部品を import する**前に** `const H = setupHarness()` を呼ぶ（registerHooks は呼んだ後の import にだけ効く = 部品は await import で読む）。
   中身:
   - loader（node:module の registerHooks・同期）: .svelte / .svelte.ts を svelte/compiler で client 向けに compile・svelte は browser 条件で解く・
     拡張子なしの相対 import に .ts を補う・素の .ts は TypeScript で変換（node の型の削除だけでは読めない構文 = parameter property・enum 等が来ても読めるように）
   - Tauri の偽物（data: URL の module）: invoke は globalThis.__invoke（= H.calls に記録・応答は H.handlers[command]）・
     listen は globalThis.__listeners に登録（H.emit(name, payload) で流す）・plugin-dialog の open は globalThis.__dialogOpen（無ければ null）
   - jsdom の global 化（window / document と大文字の型を全部・CodeMirror の測定の矩形は 0・scrollIntoView は no-op）
   差し替えを変えたいテストは fakes（module 名 → JS のソース文字列）で上書き・追加する（例: `{ "@tauri-apps/plugin-fs": "export const …" }`）。
   fetch の差し替えは部品ごとの約束なのでテスト側で行う（mediaview.test.mjs）。 */
import { registerHooks, stripTypeScriptTypes } from "node:module";
import { readFileSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { compile, compileModule } from "svelte/compiler";
import { JSDOM } from "jsdom";
import ts from "typescript";

/** app/ の URL（svelte の解決の起点・.ts の変換の範囲） */
export const APP = new URL("../../../", import.meta.url).href;

/** Tauri の偽物（既定）。invoke / listen / open はそれぞれ globalThis の受け口へ */
export const DEFAULT_FAKES = {
  "@tauri-apps/api/core": "export const invoke = (...a) => globalThis.__invoke(...a); export const convertFileSrc = (p) => 'asset://localhost/' + encodeURIComponent(p);",
  "@tauri-apps/api/event":
    "export const listen = async (name, f) => { (globalThis.__listeners[name] ??= []).push(f); return () => { globalThis.__listeners[name] = globalThis.__listeners[name].filter((x) => x !== f); }; };",
  "@tauri-apps/plugin-dialog": "export const open = async (...a) => (globalThis.__dialogOpen ? globalThis.__dialogOpen(...a) : null);",
};

/** svelte の exports を browser 条件で自分で解く(node の条件解決に頼らない)。無い subpath は null */
const SVELTE_PKG = JSON.parse(readFileSync(fileURLToPath(new URL("node_modules/svelte/package.json", APP)), "utf8"));
export function resolveSvelteBrowser(spec) {
  const sub = spec === "svelte" ? "." : "./" + spec.slice("svelte/".length);
  let e = SVELTE_PKG.exports[sub];
  while (e && typeof e === "object") e = e.browser ?? e.default ?? e.import;
  return typeof e === "string" ? new URL("node_modules/svelte/" + e.replace(/^\.\//, ""), APP).href : null;
}

let installed = null; // 登録済みの fakes（1 プロセスに 1 組。違う fakes で 2 度目に呼ばれたら throw = 黙って前の物が効くのを防ぐ）

/** loader を登録する（1 プロセスに 1 回。setupHarness が呼ぶ） */
export function installLoader(fakes = DEFAULT_FAKES) {
  if (installed) {
    const same = (a, b) => Object.keys(a).length === Object.keys(b).length && Object.keys(a).every((k) => Object.hasOwn(b, k) && a[k] === b[k]);
    if (!same(installed, fakes)) throw new Error("installLoader: loader は登録済み（違う fakes では登録し直せない = 1 プロセスに 1 組）");
    return;
  }
  installed = { ...fakes };
  registerHooks({
    resolve(spec, ctx, next) {
      if (Object.hasOwn(fakes, spec)) return { url: "data:text/javascript," + encodeURIComponent(fakes[spec]), shortCircuit: true };
      if (spec === "svelte" || spec.startsWith("svelte/")) {
        // browser 側(index-client.js 等)に自分で解く。next に conditions を足して渡す方式は Node 22(CI)の registerHooks が
        // 無視して server 側(index-server.js = mount が無い)に解いていた(2.0.0 の CI 失敗・Node 26 では効いていた)
        const url = resolveSvelteBrowser(spec);
        if (url) return { url, shortCircuit: true };
      }
      if ((spec.startsWith("./") || spec.startsWith("../")) && ctx.parentURL?.startsWith("file:")) {
        const f = fileURLToPath(new URL(spec, ctx.parentURL));
        if (!existsSync(f) && existsSync(f + ".ts")) return next(spec + ".ts", ctx);
      }
      return next(spec, ctx);
    },
    load(url, ctx, next) {
      if (url.endsWith(".svelte") || url.endsWith(".svelte.ts")) {
        const file = fileURLToPath(url);
        const src = readFileSync(file, "utf8");
        const { js } = url.endsWith(".svelte")
          ? compile(src, { filename: file, generate: "client", css: "injected" })
          : compileModule(stripTypeScriptTypes(src), { filename: file, generate: "client" });
        return { format: "module", source: js.code, shortCircuit: true };
      }
      if (url.endsWith(".ts") && url.startsWith(APP) && !url.includes("/node_modules/")) {
        const out = ts.transpileModule(readFileSync(fileURLToPath(url), "utf8"), {
          compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022, verbatimModuleSyntax: false },
        });
        return { format: "module", source: out.outputText, shortCircuit: true };
      }
      return next(url, ctx);
    },
  });
}

/** jsdom を作り、window / document と DOM の型を globalThis に載せる */
export function installDom() {
  // url を与える = origin を opaque にしない。無いと下の総なめで window.localStorage の getter が SecurityError を投げる
  // (Node 22 = CI には localStorage の global が無く define が呼ばれる・Node 25+ は global があるので素通り = 手元だけ通っていた 2.0.0 の CI 失敗)
  const dom = new JSDOM("<!doctype html><body></body>", { pretendToBeVisual: true, url: "http://localhost/" });
  const define = (k) => {
    const v = dom.window[k];
    Object.defineProperty(globalThis, k, { value: typeof v === "function" && k[0] === k[0].toLowerCase() ? v.bind(dom.window) : v, configurable: true, writable: true });
  };
  // node に無い物は全部（実部品が使う DOM の型 = HTMLMediaElement・ShadowRoot・CSSStyleSheet 等）
  for (const k of Object.getOwnPropertyNames(dom.window)) if (!(k in globalThis)) define(k);
  // node にもある物は jsdom の物で上書き（Event / File / Blob / navigator / MutationObserver 等 = 部品が作る物と instanceof が合うように）
  for (const k of ["window", "document", "Node", "Element", "HTMLElement", "Text", "Comment", "DocumentFragment", "MutationObserver", "getComputedStyle", "requestAnimationFrame", "cancelAnimationFrame", "navigator", "KeyboardEvent", "MouseEvent", "Event", "File", "Blob", "DataTransfer", "Window"]) {
    if (k in dom.window) define(k);
  }
  // CodeMirror の測定（jsdom に配置は無い = 0 の矩形）・scrollIntoView は no-op
  dom.window.Range.prototype.getClientRects = () => [];
  dom.window.Range.prototype.getBoundingClientRect = () => ({ left: 0, right: 0, top: 0, bottom: 0, width: 0, height: 0, x: 0, y: 0 });
  dom.window.HTMLElement.prototype.scrollIntoView = () => {};
  return dom;
}

/**
 * 土台を一式入れる。戻り値:
 *   dom … JSDOM / calls … invoke の記録 [{name, args}] / handlers … command → 応答を返す関数（無い command は throw）/
 *   emit(name, payload) … listen で登録された受け口へ event を流す / listeners … 登録表 / sleep(ms) / waitFor(f, ms)
 * @param {{ fakes?: Record<string, string> }} [opts] fakes = Tauri の偽物の上書き・追加（module 名 → JS のソース）
 */
export function setupHarness(opts = {}) {
  installLoader({ ...DEFAULT_FAKES, ...(opts.fakes ?? {}) });
  const dom = installDom();
  const calls = [];
  const handlers = {};
  const listeners = {};
  globalThis.__listeners = listeners;
  globalThis.__invoke = async (name, payload) => {
    calls.push({ name, args: payload?.args ?? payload });
    const h = handlers[name];
    if (!h) throw new Error(`unexpected invoke: ${name}`);
    return h(payload?.args ?? payload);
  };
  const emit = (name, payload) => (listeners[name] ?? []).forEach((f) => f({ payload }));
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const waitFor = async (f, ms = 1000) => {
    for (let i = 0; i < ms / 5 && !f(); i++) await sleep(5);
    return f();
  };
  return { dom, calls, handlers, listeners, emit, sleep, waitFor };
}
