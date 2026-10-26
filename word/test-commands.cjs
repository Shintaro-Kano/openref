// リボンの関数コマンド（commands.js）+ Office 層（doc.js）の node 実弾テスト。
// Office / Word は厳格な mock（load() + sync() 前のスカラー参照は PropertyNotLoaded・書き込みは sync で反映・
// 別 Word.run の proxy 使用は InvalidObjectPath）。citeproc は本物（vendor/citeproc.js）・スタイルは word/styles の実 CSL。
// 実行: node word/test-commands.cjs
"use strict";
const fs = require("fs");
const path = require("path");
const vm = require("vm");
const assert = require("assert");

const dir = __dirname;
const VERBOSE = process.argv.includes("-v");
const SCRIPTS = ["vendor/citeproc.js", "cite-core.js", "doc.js", "commands.js"].map(
  (f) => new vm.Script(fs.readFileSync(path.join(dir, f), "utf8"), { filename: f })
);
const LOCALE = fs.readFileSync(path.join(dir, "vendor/locales-en-US.xml"), "utf8");
const ORIGIN = "https://localhost:23120";

// ======================================================================
// 文書モデル（段落の列。段落内 = テキスト片と inline CC（入れ子可）。block CC = 段落の集合）
// ======================================================================
class MockError extends Error {
  constructor(code, message) {
    super(message);
    this.name = "OfficeExtension.Error";
    this.code = code;
    this.debugInfo = { code: code, message: message, errorLocation: "mock" };
  }
}
let nodeSeq = 0;
class CCNode {
  constructor(kind) {
    this.id = ++nodeSeq;
    this.kind = kind; // inline | block
    this.tag = "";
    this.title = "";
    this.items = []; // inline: テキスト片 {t:"text", text} と入れ子の CC {t:"cc", cc}
    this.font = {};
  }
}
class Para {
  constructor(text) {
    this.items = text ? [{ t: "text", text: text }] : [];
    this.block = null;
    this.style = "Normal";
  }
}

function htmlToText(html) {
  return html
    .replace(/<[^>]*>/g, "")
    .replace(/&#(\d+);/g, (_, n) => String.fromCharCode(+n))
    .replace(/&amp;/g, "&")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&nbsp;/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}
/** insertHtml を block CC に入れたときの段落（csl-entry ごと / <p> ごと） */
function htmlBlocks(html) {
  let parts;
  if (html.includes("csl-entry")) parts = html.split(/(?=<div class="csl-entry")/).filter((s) => s.includes("csl-entry"));
  else if (/<p[\s>]/.test(html)) parts = html.split(/<\/p>/).filter((s) => s.trim());
  else parts = [html];
  // 番号付きスタイルは csl-left-margin（"1."）と csl-right-inline が別 div。Word ではタブ相当 → 空白 1 つで代用
  return parts.map((s) => htmlToText(s.replace(/<\/div>/g, " </div>")));
}

class Model {
  constructor(paras) {
    this.paras = paras.map((t) => (t instanceof Para ? t : new Para(t)));
    // 選択 = {para, start, endPara?, end, sticky?}。sticky = 境界の癖の再現（後述 insertAt）
    this.sel = { para: this.paras[0], start: 0, end: 0 };
    // stickyCCs = どの経路で挿入しても末尾境界では中に入ってしまう CC（作り直し先でも入れ子になる場合の再現）
    this.stickyCCs = new Set();
    // fail = 失敗を注入する操作名（例: "paraInsertParagraph"）
    this.fail = new Set();
  }
  injected(op) {
    if (this.fail.has(op)) throw new MockError("GeneralException", "mock: 注入した失敗（" + op + "）");
  }
  itemText(it) {
    return it.t === "text" ? it.text : it.cc.items.map((s) => this.itemText(s)).join("");
  }
  paraText(p) {
    return p.items.map((it) => this.itemText(it)).join("");
  }
  ccText(cc) {
    return cc.kind === "inline"
      ? this.itemText({ t: "cc", cc: cc })
      : this.blockParas(cc)
          .map((p) => this.paraText(p))
          .join("\n");
  }
  /** 段落 p の inline 要素を入れ子ごと前順に走査: visit(item, start, end, container, parent)。
      parent = その要素を直接含む CC（入れ子の inline CC。最上位なら段落の block CC か null） */
  walk(p, visit) {
    const rec = (items, base, parent) => {
      let o = base;
      for (const it of items) {
        const len = this.itemText(it).length;
        visit(it, o, o + len, items, parent);
        if (it.t === "cc") rec(it.cc.items, o, it.cc);
        o += len;
      }
    };
    rec(p.items, 0, p.block);
  }
  /** 段落 p の [start, end] を内側に含む最も内側の inline CC（カーソルは境界ちょうどなら外側） */
  inlineAt(p, start, end) {
    let best = null;
    this.walk(p, (it, s, e) => {
      if (it.t !== "cc") return;
      const inside = start === end ? start > s && start < e : start >= s && end <= e;
      if (inside) best = { item: it, start: s, end: e };
    });
    return best;
  }
  allCCs() {
    const out = [];
    for (const p of this.paras) {
      if (p.block && !out.includes(p.block)) out.push(p.block);
      this.walk(p, (it) => {
        if (it.t === "cc") out.push(it.cc);
      });
    }
    return out;
  }
  alive(cc) {
    return this.allCCs().includes(cc);
  }
  blockParas(cc) {
    return this.paras.filter((p) => p.block === cc);
  }
  index(p) {
    const i = this.paras.indexOf(p);
    if (i < 0) throw new MockError("ItemNotFound", "段落が文書にない（削除済み）");
    return i;
  }
  findInline(cc) {
    for (const p of this.paras) {
      let hit = null;
      this.walk(p, (it, s, e, container, parent) => {
        if (it.t === "cc" && it.cc === cc) hit = { para: p, item: it, start: s, end: e, container: container, parent: parent };
      });
      if (hit) return hit;
    }
    throw new MockError("ItemNotFound", "content control が文書にない（削除済み）");
  }
  locateSeg(seg) {
    for (const p of this.paras) {
      let hit = null;
      this.walk(p, (it, s, e, container, parent) => {
        if (it === seg) hit = { para: p, start: s, end: e, container: container, parent: parent };
      });
      if (hit) return hit;
    }
    throw new MockError("ItemNotFound", "range の文字列が文書にない");
  }
  /** Word の ContentControl.parentContentControl: 入れ子なら外側の inline CC、最上位なら段落の block CC、無ければ null */
  ccParent(cc) {
    if (cc.kind === "block") return null;
    return this.findInline(cc).parent || null;
  }
  // ---- range（解決済みの位置） ----
  bounds(r) {
    switch (r.kind) {
      case "sel":
      case "span":
        this.index(r.para);
        if (r.endPara) this.index(r.endPara);
        return { para: r.para, start: r.start, endPara: r.endPara && r.endPara !== r.para ? r.endPara : undefined, end: r.end, sticky: r.sticky };
      case "point":
        this.index(r.para);
        return { para: r.para, start: r.off, end: r.off, sticky: r.sticky };
      case "seg": {
        const l = this.locateSeg(r.seg);
        return { para: l.para, start: l.start, end: l.end };
      }
      case "cc": {
        if (r.cc.kind === "block") {
          const ps = this.blockParas(r.cc);
          if (!ps.length) throw new MockError("ItemNotFound", "block CC が空");
          const last = ps[ps.length - 1];
          const lastLen = this.paraText(last).length;
          if (r.loc === "After" || r.loc === "End") return { para: last, start: lastLen, end: lastLen };
          if (r.loc === "Before" || r.loc === "Start") return { para: ps[0], start: 0, end: 0 };
          return { para: ps[0], start: 0, endPara: last === ps[0] ? undefined : last, end: lastLen };
        }
        const f = this.findInline(r.cc);
        if (r.loc === "After" || r.loc === "End") return { para: f.para, start: f.end, end: f.end };
        if (r.loc === "Before" || r.loc === "Start") return { para: f.para, start: f.start, end: f.start };
        return { para: f.para, start: f.start, end: f.end };
      }
      case "para":
        this.index(r.para);
        return { para: r.para, start: 0, end: this.paraText(r.para).length };
      default:
        throw new Error("mock: 未知の range " + r.kind);
    }
  }
  subRange(r, loc) {
    const b = this.bounds(r);
    if (loc === "Start") return { kind: "point", para: b.para, off: b.start };
    if (loc === "End" || loc === "After") return { kind: "point", para: b.endPara || b.para, off: b.end, sticky: b.sticky };
    if (loc === "Whole" || loc === "Content" || loc === undefined) return r;
    throw new MockError("InvalidArgument", "Range.getRange: 不正な rangeLocation " + loc);
  }
  paraRange(p, loc) {
    this.index(p);
    if (loc === "Whole" || loc === "Content" || loc === undefined) return { kind: "para", para: p };
    if (loc === "Start") return { kind: "point", para: p, off: 0 };
    if (loc === "End" || loc === "After") return { kind: "point", para: p, off: this.paraText(p).length };
    throw new MockError("InvalidArgument", "Paragraph.getRange: 不正な rangeLocation " + loc);
  }
  /** 位置の比較用の通し番号（段落の番号 × 1e7 + 段落内の文字位置。段落の末尾と次の段落の頭は別の位置） */
  absRange(r) {
    const b = this.bounds(r);
    return [this.index(b.para) * 1e7 + b.start, this.index(b.endPara || b.para) * 1e7 + b.end];
  }
  /** Word の Range.compareLocationWith（LocationRelation） */
  compare(ra, rb) {
    const A = this.absRange(ra);
    const B = this.absRange(rb);
    if (A[0] === B[0] && A[1] === B[1]) return "Equal";
    if (A[1] < B[0]) return "Before";
    if (A[1] === B[0] && A[0] < B[0]) return "AdjacentBefore";
    if (A[0] > B[1]) return "After";
    if (A[0] === B[1] && A[1] > B[1]) return "AdjacentAfter";
    if (A[0] <= B[0] && A[1] >= B[1]) return A[0] === B[0] ? "ContainsStart" : A[1] === B[1] ? "ContainsEnd" : "Contains";
    if (A[0] >= B[0] && A[1] <= B[1]) return A[0] === B[0] ? "InsideStart" : A[1] === B[1] ? "InsideEnd" : "Inside";
    return A[0] < B[0] ? "OverlapsBefore" : "OverlapsAfter";
  }
  /** Word の Range.expandTo: 両方を覆う範囲 */
  expand(ra, rb) {
    const A = this.bounds(ra);
    const B = this.bounds(rb);
    const pos = (p, off) => this.index(p) * 1e7 + off;
    const s = pos(A.para, A.start) <= pos(B.para, B.start) ? [A.para, A.start] : [B.para, B.start];
    const ea = [A.endPara || A.para, A.end];
    const eb = [B.endPara || B.para, B.end];
    const e = pos(ea[0], ea[1]) >= pos(eb[0], eb[1]) ? ea : eb;
    return { kind: "span", para: s[0], start: s[1], endPara: e[0], end: e[1] };
  }
  splitInsert(items, off, node) {
    let o = 0;
    for (let i = 0; i < items.length; i++) {
      const it = items[i];
      const len = this.itemText(it).length;
      if (off === o) {
        items.splice(i, 0, node);
        return;
      }
      if (off < o + len) {
        if (it.t !== "text") throw new Error("mock: CC の内側への挿入は inlineAt で処理するはず");
        items.splice(i, 1, { t: "text", text: it.text.slice(0, off - o) }, node, { t: "text", text: it.text.slice(off - o) });
        return;
      }
      o += len;
    }
    items.push(node);
  }
  /** 段落 p の off に node（text 片 / CC）を差し込む。off を厳密に内側に含む最も内側の CC の中へ入れる。
      sticky = Word で起きる境界の癖の再現: off がその CC の末尾ちょうどなら、その CC の中（末尾）に入れる
      （直前に挿入した引用の直後で Add/Edit を押すと、新しい CC が前の引用の中に入ることがある） */
  insertAt(p, off, node, sticky) {
    for (const s of [sticky].concat([...this.stickyCCs])) {
      if (!s || !this.alive(s)) continue;
      const f = this.findInline(s);
      if (f.para === p && f.end === off) {
        s.items.push(node);
        return;
      }
    }
    const inl = this.inlineAt(p, off, off);
    if (inl) this.splitInsert(inl.item.cc.items, off - inl.start, node);
    else this.splitInsert(p.items, off, node);
  }
  insertTextAt(p, off, text, sticky) {
    const seg = { t: "text", text: text };
    this.insertAt(p, off, seg, sticky);
    return { kind: "seg", seg: seg };
  }
  rangeInsertText(r, text, loc) {
    const b = this.bounds(r);
    if (loc === "Replace") {
      if (b.start !== b.end || b.endPara) throw new MockError("NotImplemented", "mock: 選択文字列の置換は非対応（このアドインは使わない）");
      return this.insertTextAt(b.para, b.start, text, b.sticky);
    }
    if (loc === "Start" || loc === "Before") return this.insertTextAt(b.para, b.start, text);
    if (loc === "End" || loc === "After") return this.insertTextAt(b.endPara || b.para, b.end, text, b.sticky);
    throw new MockError("InvalidArgument", "Range.insertText: 不正な insertLocation " + loc);
  }
  /** Range.insertContentControl: 挿入直後の文字列を包む / 折りたたんだ位置に空の CC を作る */
  rangeWrapCC(r) {
    if (r.kind === "seg") {
      const l = this.locateSeg(r.seg);
      const cc = new CCNode("inline");
      cc.items = [r.seg];
      l.container.splice(l.container.indexOf(r.seg), 1, { t: "cc", cc: cc });
      return cc;
    }
    const b = this.bounds(r);
    if (b.start === b.end && !b.endPara) {
      const cc = new CCNode("inline");
      this.insertAt(b.para, b.start, { t: "cc", cc: cc }, b.sticky);
      return cc;
    }
    throw new MockError("NotImplemented", "mock: insertContentControl は挿入直後の文字列か折りたたんだ位置だけ対応");
  }
  rangeParas(r) {
    const b = this.bounds(r);
    const a = this.index(b.para);
    const z = b.endPara ? this.index(b.endPara) : a;
    return this.paras.slice(a, z + 1);
  }
  parentCC(r) {
    const b = this.bounds(r);
    if (!b.endPara) {
      const inl = this.inlineAt(b.para, b.start, b.end);
      if (inl) return inl.item.cc;
    }
    if (b.para.block && (!b.endPara || b.endPara.block === b.para.block)) return b.para.block;
    return null;
  }
  select(r) {
    const b = this.bounds(r);
    this.sel = { para: b.para, start: b.start, endPara: b.endPara, end: b.end };
  }
  // ---- content control ----
  ccInsertHtml(cc, html) {
    if (cc.kind === "inline") {
      this.findInline(cc);
      cc.items = [{ t: "text", text: htmlToText(html) }];
      return { kind: "cc", cc: cc, loc: "Content" };
    }
    const ps = this.blockParas(cc);
    if (!ps.length) throw new MockError("ItemNotFound", "block CC が空");
    const at = this.index(ps[0]);
    this.paras = this.paras.filter((p) => p.block !== cc);
    const fresh = htmlBlocks(html).map((t) => {
      const p = new Para(t);
      p.block = cc;
      return p;
    });
    this.paras.splice(at, 0, ...fresh);
    return { kind: "cc", cc: cc, loc: "Content" };
  }
  ccInsertText(cc, text, loc) {
    if (loc !== "Replace") throw new MockError("NotImplemented", "mock: ContentControl.insertText は Replace のみ");
    if (cc.kind !== "inline") throw new MockError("NotImplemented", "mock: block CC への insertText は非対応");
    this.findInline(cc);
    cc.items = [{ t: "text", text: text }];
    return { kind: "cc", cc: cc, loc: "Content" };
  }
  ccDelete(cc, keep) {
    if (cc.kind === "inline") {
      const f = this.findInline(cc);
      const i = f.container.indexOf(f.item);
      f.container.splice(i, 1, ...(keep ? cc.items : []));
    } else {
      if (keep) this.blockParas(cc).forEach((p) => (p.block = null));
      else this.paras = this.paras.filter((p) => p.block !== cc);
    }
    if (!this.paras.length) this.paras.push(new Para("")); // Word の文書は最低 1 段落
  }
  ccParas(cc) {
    return cc.kind === "block" ? this.blockParas(cc) : [this.findInline(cc).para];
  }
  /** ContentControl.search: CC の文字列の中の出現位置（段落内の範囲）。matchCase でなければ大小無視 */
  ccSearch(cc, text, opts) {
    if (cc.kind !== "inline") throw new MockError("NotImplemented", "mock: block CC の search は非対応");
    const f = this.findInline(cc);
    const hay = this.itemText(f.item);
    const mc = !!(opts && opts.matchCase);
    const h = mc ? hay : hay.toLowerCase();
    const n = mc ? text : text.toLowerCase();
    const out = [];
    for (let i = h.indexOf(n); n && i >= 0; i = h.indexOf(n, i + n.length)) {
      out.push({ kind: "span", para: f.para, start: f.start + i, end: f.start + i + n.length });
    }
    return out;
  }
  /** Range.delete: 段落内 [start, end) の文字を消す（CC の境界は残す） */
  rangeDelete(r) {
    const b = this.bounds(r);
    if (b.endPara) throw new MockError("NotImplemented", "mock: 段落をまたぐ Range.delete は非対応");
    const rec = (items, base) => {
      let o = base;
      for (const it of items) {
        const len = this.itemText(it).length;
        if (it.t === "text") {
          const a = Math.max(b.start, o);
          const z = Math.min(b.end, o + len);
          if (a < z) it.text = it.text.slice(0, a - o) + it.text.slice(z - o);
        } else {
          rec(it.cc.items, o);
        }
        o += len;
      }
    };
    rec(b.para.items, 0);
  }
  // ---- paragraph ----
  paraInsertParagraph(p, text, loc) {
    this.injected("paraInsertParagraph");
    const i = this.index(p);
    const np = new Para(text);
    if (loc === "Before") this.paras.splice(i, 0, np);
    else if (loc === "After") this.paras.splice(i + 1, 0, np);
    else throw new MockError("InvalidArgument", "Paragraph.insertParagraph: Before / After のみ（" + loc + "）");
    return np;
  }
  paraWrapCC(p) {
    this.index(p);
    if (p.block) throw new MockError("NotImplemented", "mock: block CC の入れ子は非対応");
    const cc = new CCNode("block");
    p.block = cc;
    return cc;
  }
  prevPara(p) {
    const i = this.index(p);
    return i > 0 ? this.paras[i - 1] : null;
  }
  paraDelete(p) {
    this.paras.splice(this.index(p), 1);
    if (!this.paras.length) this.paras.push(new Para(""));
  }
  bodyInsertParagraph(text, loc) {
    const np = new Para(text);
    if (loc === "End") this.paras.push(np);
    else if (loc === "Start") this.paras.unshift(np);
    else throw new MockError("InvalidArgument", "Body.insertParagraph: Start / End のみ（" + loc + "）");
    return np;
  }
  // ---- テスト用 ----
  dump() {
    const render = (items) => items.map((it) => (it.t === "text" ? it.text : "{" + it.cc.tag + ":" + render(it.cc.items) + "}")).join("");
    return this.paras.map((p) => (p.block ? "[" + p.block.tag + "] " : "") + render(p.items));
  }
  byTag(prefix) {
    return this.allCCs().filter((cc) => cc.tag.indexOf(prefix) === 0);
  }
}

// ======================================================================
// Word JS の proxy（キュー + sync）
// ======================================================================
function parseProps(spec) {
  if (spec === undefined) return null; // 全部
  const s = Array.isArray(spec) ? spec.join(",") : String(spec);
  return s
    .split(",")
    .map((x) => x.trim())
    .filter(Boolean);
}
function pick(all, props, allowed, type) {
  const out = {};
  for (const k of props || Object.keys(all)) {
    if (k === "isNullObject") continue;
    if (!allowed.includes(k)) throw new MockError("InvalidArgument", type + ".load: 未知のプロパティ " + k);
    out[k] = all[k];
  }
  return out;
}
const CC_PROPS = ["tag", "title", "text", "id", "appearance"];
const PARA_PROPS = ["text", "style", "styleBuiltIn"];
const RANGE_PROPS = ["text"];

function makeWord(env) {
  class Ctx {
    constructor() {
      this.q = [];
      this.document = new DocumentP(this);
      this.done = false;
    }
    enqueue(fn) {
      if (this.done) throw new MockError("InvalidObjectPath", "終わった Word.run の context を使った");
      this.q.push(fn);
    }
    async sync() {
      env.stats.syncs++;
      await null;
      const q = this.q;
      this.q = [];
      for (const op of q) op();
    }
  }
  class P {
    constructor(ctx, resolve, opts) {
      this._ctx = ctx;
      this._resolveFn = resolve;
      this._resolved = false;
      this._node = undefined;
      this._loaded = null;
      if (!opts || opts.eager !== false) ctx.enqueue(() => this._get()); // Office のキューと同じく生成順に解決
    }
    _get() {
      if (!this._resolved) {
        if (!this._resolveFn) throw new Error("mock: 生成前の proxy を使った");
        this._node = this._resolveFn();
        this._resolved = true;
      }
      return this._node;
    }
    _use() {
      const n = this._get();
      if (n === null) throw new MockError("InvalidObjectPath", "null object を操作した");
      this._alive(n);
      return n;
    }
    _alive() {}
    _prop(name) {
      if (!this._loaded) throw new MockError("PropertyNotLoaded", this.constructor.name + "." + name + ": load() + sync() の前に読んだ");
      if (!(name in this._loaded)) throw new MockError("PropertyNotLoaded", this.constructor.name + "." + name + ": load していない");
      return this._loaded[name];
    }
    get isNullObject() {
      if (!this._loaded) throw new MockError("PropertyNotLoaded", this.constructor.name + ".isNullObject: load() + sync() の前に読んだ");
      return this._node === null;
    }
    load(spec) {
      const props = parseProps(spec);
      this._ctx.enqueue(() => {
        const n = this._get();
        this._loaded = n === null ? {} : this._snapshot(n, props);
      });
      return this;
    }
    _same(other) {
      if (other._ctx !== this._ctx) throw new MockError("InvalidObjectPath", "別の Word.run の proxy を混ぜた");
    }
  }
  function created(Cls, ctx) {
    return new Cls(ctx, null, { eager: false });
  }
  class FontP {
    constructor(ctx, target) {
      ["name", "size", "color", "bold"].forEach((k) => {
        Object.defineProperty(this, k, {
          set(v) {
            ctx.enqueue(() => {
              target()[k] = v;
            });
          },
          get() {
            throw new MockError("PropertyNotLoaded", "Font." + k + ": この mock は書き込み専用");
          },
        });
      });
    }
  }
  class ContentControlP extends P {
    _alive(n) {
      if (!env.model.alive(n)) throw new MockError("ItemNotFound", "削除済みの content control を操作した");
    }
    _snapshot(n, props) {
      return pick({ tag: n.tag, title: n.title, text: env.model.ccText(n), id: n.id, appearance: "BoundingBox" }, props, CC_PROPS, "ContentControl");
    }
    get tag() {
      return this._prop("tag");
    }
    set tag(v) {
      this._ctx.enqueue(() => {
        this._use().tag = v;
      });
    }
    get title() {
      return this._prop("title");
    }
    set title(v) {
      this._ctx.enqueue(() => {
        this._use().title = v;
      });
    }
    get text() {
      return this._prop("text");
    }
    get id() {
      return this._prop("id");
    }
    get font() {
      return new FontP(this._ctx, () => this._use().font);
    }
    get paragraphs() {
      return new ParagraphCollectionP(this._ctx, () => env.model.ccParas(this._use()));
    }
    get parentContentControlOrNullObject() {
      return new ContentControlP(this._ctx, () => env.model.ccParent(this._use()));
    }
    insertText(text, loc) {
      const out = created(RangeP, this._ctx);
      this._ctx.enqueue(() => {
        out._node = env.model.ccInsertText(this._use(), text, loc);
        out._resolved = true;
      });
      return out;
    }
    select() {
      this._ctx.enqueue(() => env.model.select({ kind: "cc", cc: this._use(), loc: "Whole" }));
    }
    search(text, opts) {
      return new RangeCollectionP(this._ctx, () => {
        if (env.searchFails) throw new MockError("NotImplemented", "mock: search が使えない環境（注入）");
        return env.model.ccSearch(this._use(), text, opts);
      });
    }
    insertHtml(html, loc) {
      if (loc !== "Replace") throw new MockError("InvalidArgument", "mock: insertHtml は Replace のみ");
      const out = created(RangeP, this._ctx);
      this._ctx.enqueue(() => {
        out._node = env.model.ccInsertHtml(this._use(), html);
        out._resolved = true;
      });
      return out;
    }
    delete(keep) {
      if (typeof keep !== "boolean") throw new MockError("InvalidArgument", "ContentControl.delete(keepContent) は boolean 必須");
      this._ctx.enqueue(() => env.model.ccDelete(this._use(), keep));
    }
    getRange(loc) {
      return new RangeP(this._ctx, () => ({ kind: "cc", cc: this._use(), loc: loc || "Whole" }));
    }
  }
  class RangeP extends P {
    _snapshot(r, props) {
      const b = env.model.bounds(r);
      return pick({ text: env.model.paraText(b.para).slice(b.start, b.end) }, props, RANGE_PROPS, "Range");
    }
    get text() {
      return this._prop("text");
    }
    getRange(loc) {
      return new RangeP(this._ctx, () => env.model.subRange(this._use(), loc));
    }
    insertText(text, loc) {
      const out = created(RangeP, this._ctx);
      this._ctx.enqueue(() => {
        out._node = env.model.rangeInsertText(this._use(), text, loc);
        out._resolved = true;
      });
      return out;
    }
    insertContentControl() {
      const out = created(ContentControlP, this._ctx);
      this._ctx.enqueue(() => {
        out._node = env.model.rangeWrapCC(this._use());
        out._resolved = true;
      });
      return out;
    }
    get paragraphs() {
      return new ParagraphCollectionP(this._ctx, () => env.model.rangeParas(this._use()));
    }
    get parentContentControlOrNullObject() {
      return new ContentControlP(this._ctx, () => env.model.parentCC(this._use()));
    }
    select() {
      this._ctx.enqueue(() => env.model.select(this._use()));
    }
    delete() {
      this._ctx.enqueue(() => env.model.rangeDelete(this._use()));
    }
    expandTo(other) {
      this._same(other);
      return new RangeP(this._ctx, () => env.model.expand(this._use(), other._use()));
    }
    compareLocationWith(other) {
      this._same(other);
      const out = new ClientResult("Range.compareLocationWith");
      this._ctx.enqueue(() => out._set(env.model.compare(this._use(), other._use())));
      return out;
    }
  }
  class ClientResult {
    constructor(name) {
      this._name = name;
      this._done = false;
      this._v = undefined;
    }
    _set(v) {
      this._v = v;
      this._done = true;
    }
    get value() {
      if (!this._done) throw new MockError("PropertyNotLoaded", this._name + ".value: sync() の前に読んだ");
      return this._v;
    }
  }
  class ParagraphP extends P {
    _alive(n) {
      env.model.index(n);
    }
    _snapshot(p, props) {
      return pick({ text: env.model.paraText(p), style: p.style, styleBuiltIn: p.style }, props, PARA_PROPS, "Paragraph");
    }
    get text() {
      return this._prop("text");
    }
    get styleBuiltIn() {
      return this._prop("styleBuiltIn");
    }
    set styleBuiltIn(v) {
      this._ctx.enqueue(() => {
        this._use().style = v;
      });
    }
    insertParagraph(text, loc) {
      const out = created(ParagraphP, this._ctx);
      this._ctx.enqueue(() => {
        out._node = env.model.paraInsertParagraph(this._use(), text, loc);
        out._resolved = true;
      });
      return out;
    }
    insertContentControl() {
      const out = created(ContentControlP, this._ctx);
      this._ctx.enqueue(() => {
        out._node = env.model.paraWrapCC(this._use());
        out._resolved = true;
      });
      return out;
    }
    getPreviousOrNullObject() {
      return new ParagraphP(this._ctx, () => env.model.prevPara(this._use()));
    }
    getRange(loc) {
      return new RangeP(this._ctx, () => env.model.paraRange(this._use(), loc));
    }
    delete() {
      this._ctx.enqueue(() => env.model.paraDelete(this._use()));
    }
  }
  class CollectionP {
    constructor(ctx, resolve, Item, itemProps, typeName) {
      this._ctx = ctx;
      this._resolveFn = resolve; // コレクションは使うたびに取り直す（Office も load のたびに再取得）
      this._Item = Item;
      this._itemProps = itemProps;
      this._type = typeName;
      this._items = null;
    }
    get items() {
      if (!this._items) throw new MockError("PropertyNotLoaded", this._type + ".items: load() + sync() の前に読んだ");
      return this._items;
    }
    load(spec) {
      const raw = parseProps(spec) || ["items"];
      let props = [];
      let all = false;
      raw.forEach((s) => {
        if (s === "items") all = true;
        else if (s.indexOf("items/") === 0) props.push(s.slice(6));
        else throw new MockError("InvalidArgument", this._type + ".load: 未対応の指定 " + s);
      });
      if (all) props = null;
      this._ctx.enqueue(() => {
        this._items = this._resolveFn().map((n) => {
          const p = created(this._Item, this._ctx);
          p._node = n;
          p._resolved = true;
          p._loaded = p._snapshot(n, props);
          return p;
        });
      });
      return this;
    }
    getFirst() {
      return new this._Item(this._ctx, () => {
        const a = this._resolveFn();
        if (!a.length) throw new MockError("ItemNotFound", this._type + ".getFirst: 空");
        return a[0];
      });
    }
    getFirstOrNullObject() {
      return new this._Item(this._ctx, () => this._resolveFn()[0] || null);
    }
    getLast() {
      return new this._Item(this._ctx, () => {
        const a = this._resolveFn();
        if (!a.length) throw new MockError("ItemNotFound", this._type + ".getLast: 空");
        return a[a.length - 1];
      });
    }
  }
  class ContentControlCollectionP extends CollectionP {
    constructor(ctx, resolve) {
      super(ctx, resolve, ContentControlP, CC_PROPS, "ContentControlCollection");
    }
    getByTag(tag) {
      return new ContentControlCollectionP(this._ctx, () => this._resolveFn().filter((n) => n.tag === tag));
    }
    getById(id) {
      return new ContentControlP(this._ctx, () => {
        const n = this._resolveFn().find((x) => x.id === id);
        if (!n) throw new MockError("ItemNotFound", "ContentControlCollection.getById: id " + id + " が無い");
        return n;
      });
    }
    getByIdOrNullObject(id) {
      return new ContentControlP(this._ctx, () => this._resolveFn().find((x) => x.id === id) || null);
    }
  }
  class ParagraphCollectionP extends CollectionP {
    constructor(ctx, resolve) {
      super(ctx, resolve, ParagraphP, PARA_PROPS, "ParagraphCollection");
    }
  }
  class RangeCollectionP extends CollectionP {
    constructor(ctx, resolve) {
      super(ctx, resolve, RangeP, RANGE_PROPS, "RangeCollection");
    }
  }
  class BodyP {
    constructor(ctx) {
      this._ctx = ctx;
    }
    get paragraphs() {
      return new ParagraphCollectionP(this._ctx, () => env.model.paras.slice());
    }
    insertParagraph(text, loc) {
      const out = created(ParagraphP, this._ctx);
      this._ctx.enqueue(() => {
        out._node = env.model.bodyInsertParagraph(text, loc);
        out._resolved = true;
      });
      return out;
    }
  }
  class DocumentP {
    constructor(ctx) {
      this._ctx = ctx;
      this.body = new BodyP(ctx);
    }
    get contentControls() {
      return new ContentControlCollectionP(this._ctx, () => env.model.allCCs());
    }
    getSelection() {
      return new RangeP(this._ctx, () => {
        const s = env.model.sel; // sync の時点の選択（Office のキュー順）
        return { kind: "sel", para: s.para, start: s.start, endPara: s.endPara, end: s.end, sticky: s.sticky };
      });
    }
  }
  return {
    InsertLocation: { replace: "Replace", start: "Start", end: "End", before: "Before", after: "After" },
    run: async function (fn) {
      const ctx = new Ctx();
      try {
        const r = await fn(ctx);
        await ctx.sync(); // Word.run の最後の暗黙 sync
        return r;
      } finally {
        ctx.done = true;
      }
    },
  };
}

// ======================================================================
// Office（settings・dialog・requirements・actions）
// ======================================================================
const clone = (v) => (v === undefined ? undefined : JSON.parse(JSON.stringify(v)));

class Settings {
  constructor(env) {
    this.env = env;
    this.mem = clone(env.persisted);
  }
  get(k) {
    return Object.prototype.hasOwnProperty.call(this.mem, k) ? clone(this.mem[k]) : null;
  }
  set(k, v) {
    if (v === undefined) throw new Error("settings.set: undefined は保存できない");
    this.mem[k] = clone(v);
  }
  remove(k) {
    delete this.mem[k];
  }
  saveAsync(cb) {
    const snap = clone(this.mem);
    this.env.later(() => {
      if (this.env.failSave) {
        if (cb) cb({ status: "failed", error: { message: "mock: 保存に失敗" } });
        return;
      }
      this.env.persisted = snap;
      this.env.stats.saves++;
      if (cb) cb({ status: "succeeded" });
    });
  }
  refreshAsync(cb) {
    this.env.later(() => {
      this.mem = clone(this.env.persisted);
      this.env.stats.refreshes++;
      if (cb) cb({ status: "succeeded", value: this });
    });
  }
}

class MockDialog {
  constructor(host, url, opts) {
    this.host = host;
    this.url = url;
    this.opts = opts;
    this.handlers = {};
    this.state = "open";
    const u = new URL(url);
    this.origin = u.origin;
    this.page = u.pathname.replace(/^\/word\//, "");
    this.params = Object.fromEntries(u.searchParams);
    this.d = this.params.d ? JSON.parse(this.params.d) : null;
    this.docAtOpen = host.env.model.dump(); // 開いた瞬間の文書（仮置きが先に入っているかの検査用）
  }
  addEventHandler(type, h) {
    this.handlers[type] = h;
  }
  close() {
    if (this.state !== "open") return;
    this.state = "closing"; // Office は非同期に閉じる → 直後の displayDialogAsync は 12007
    this.host.env.clock.setTimeout(() => {
      this.state = "closed";
    }, this.host.closeDelay);
  }
  // ---- テストからの操作 ----
  send(msg) {
    assert.strictEqual(this.state, "open", "閉じたダイアログからメッセージは来ない");
    this.handlers.dialogMessageReceived({ message: JSON.stringify(msg), origin: ORIGIN });
  }
  userClose() {
    assert.strictEqual(this.state, "open");
    this.state = "closed";
    this.handlers.dialogEventReceived({ error: 12006 });
  }
  /** ページを読めなかった（12002 = 見つからない等）。ダイアログ自体は開いたまま */
  loadFail(code) {
    assert.strictEqual(this.state, "open");
    this.handlers.dialogEventReceived({ error: code });
  }
}
class DialogHost {
  constructor(env) {
    this.env = env;
    this.dialogs = [];
    this.fail12007 = 0;
    this.rejected = 0;
    this.closeDelay = 100;
  }
  open(url, opts, cb) {
    this.env.later(() => {
      const busy = this.dialogs.some((d) => d.state !== "closed");
      if (this.fail12007 > 0 || busy) {
        if (this.fail12007 > 0) this.fail12007--;
        this.rejected++;
        cb({ status: "failed", error: { code: 12007, message: "A dialog box is already opened from this host window." } });
        return;
      }
      const dlg = new MockDialog(this, url, opts);
      this.dialogs.push(dlg);
      cb({ status: "succeeded", value: dlg });
    });
  }
  get current() {
    const open = this.dialogs.filter((d) => d.state === "open");
    return open[open.length - 1] || null;
  }
}

class Clock {
  constructor() {
    this.now = 0;
    this.timers = [];
    this.seq = 0;
  }
  setTimeout(fn, ms) {
    const id = ++this.seq;
    this.timers.push({ id: id, at: this.now + Math.max(0, ms || 0), fn: fn });
    return id;
  }
  clearTimeout(id) {
    this.timers = this.timers.filter((t) => t.id !== id);
  }
  async advance(ms) {
    const target = this.now + ms;
    for (;;) {
      await flush();
      const due = this.timers.filter((t) => t.at <= target).sort((a, b) => a.at - b.at || a.id - b.id)[0];
      if (!due) break;
      this.timers = this.timers.filter((t) => t !== due);
      this.now = due.at;
      due.fn();
    }
    this.now = target;
    await flush();
  }
}
async function flush() {
  for (let i = 0; i < 5; i++) await new Promise((r) => setImmediate(r));
}

// ======================================================================
// 蔵書（/api/*）と環境
// ======================================================================
const LIBRARY = {
  K1: {
    type: "article-journal",
    title: "Pancreatic cancer: changing epidemiology",
    author: [{ family: "Stoffel", given: "Elena M" }, { family: "Goggins", given: "Michael" }],
    "container-title": "Gastroenterology",
    issued: { "date-parts": [[2023, 2]] },
    volume: "164", issue: "5", page: "752-765",
    DOI: "10.1053/j.gastro.2023.02.012",
  },
  K2: {
    type: "article-journal",
    title: "The peritoneum: anatomy and patterns of disease spread",
    author: [{ family: "Gaballah", given: "Ayman H" }],
    "container-title": "RadioGraphics",
    issued: { "date-parts": [[2024]] },
    volume: "44", issue: "8", page: "e230216",
  },
  K3: {
    type: "article-journal",
    title: "PI-RADS version 2.1: a critical review",
    author: [{ family: "Turkbey", given: "Baris" }],
    "container-title": "Radiology",
    issued: { "date-parts": [[2019]] },
    volume: "292", page: "464-474",
    DOI: "10.1148/radiol.2019182946",
  },
};
const STYLES = [
  { id: "vancouver", title: "Vancouver", builtin: true },
  { id: "american-medical-association", title: "AMA (JAMA)", builtin: true },
  { id: "radiology", title: "Radiology (RSNA)", builtin: true },
  { id: "american-journal-of-roentgenology", title: "AJR", builtin: true },
];

function makeEnv(opts) {
  opts = opts || {};
  const env = {
    model: new Model(opts.paras || ["Pancreatic cancer is increasing.", "Second paragraph."]),
    persisted: clone(opts.settings || {}),
    clock: new Clock(),
    stats: { syncs: 0, saves: 0, refreshes: 0 },
    fetches: [],
    associated: {},
    library: clone(opts.library || LIBRARY),
    app: opts.app || { style: "american-journal-of-roentgenology", doiAlways: true },
    offline: false,
    logs: [],
  };
  env.later = (fn) => Promise.resolve().then(fn);
  env.dialogHost = new DialogHost(env);
  const settings = new Settings(env);
  env.settings = settings;
  const json = (v) => ({ ok: true, status: 200, json: async () => clone(v), text: async () => JSON.stringify(v) });
  const text = (s) => ({ ok: true, status: 200, text: async () => s, json: async () => JSON.parse(s) });
  const nf = () => ({ ok: false, status: 404, text: async () => "not found", json: async () => ({}) });
  const fetchMock = async function (url, init) {
    env.fetches.push({ url: url, method: (init && init.method) || "GET" });
    const u = new URL(url, ORIGIN);
    const p = u.pathname;
    if (env.offline && p.indexOf("/api/") === 0) throw new TypeError("Load failed");
    if (p === "/api/styles") return json({ styles: STYLES, current: env.app.style, doi_always: env.app.doiAlways });
    if (p === "/api/status") return json({ ok: true, version: "0.1.0", items: Object.keys(env.library).length, db_version: 9 });
    if (p === "/api/items") {
      assert.strictEqual(init && init.method, "POST");
      if (env.hang) await env.hang;
      const keys = JSON.parse(init.body).keys;
      const out = {};
      keys.forEach((k) => {
        if (env.library[k]) out[k] = env.library[k];
      });
      return json(out);
    }
    let m = /^\/word\/styles\/(.+)\.csl$/.exec(p);
    if (m) {
      const f = path.join(dir, "styles", decodeURIComponent(m[1]) + ".csl");
      return fs.existsSync(f) ? text(fs.readFileSync(f, "utf8")) : nf();
    }
    if (p === "/word/locales/en-US.xml") return text(LOCALE);
    return nf();
  };
  const Office = {
    AsyncResultStatus: { Succeeded: "succeeded", Failed: "failed" },
    EventType: { DialogMessageReceived: "dialogMessageReceived", DialogEventReceived: "dialogEventReceived" },
    HostType: { Word: "Word" },
    onReady(cb) {
      const info = { host: "Word", platform: "Mac" };
      const pr = Promise.resolve(info);
      if (cb) pr.then(cb);
      return pr;
    },
    actions: {
      associate(name, fn) {
        env.associated[name] = fn;
      },
    },
    context: {
      document: { settings: settings },
      ui: {
        displayDialogAsync(url, o, cb) {
          env.dialogHost.open(url, o, cb);
        },
      },
      requirements: {
        isSetSupported(name, v) {
          const max = { WordApi: 1.4, DialogApi: 1.2 }[name] || 0;
          return max >= parseFloat(v);
        },
      },
      diagnostics: { host: "Word", platform: "Mac", version: "16.0.mock" },
    },
  };
  const consoleMock = {
    log: (...a) => VERBOSE && console.log("  [log]", ...a),
    warn: (...a) => {
      env.logs.push(["warn", a.map(String).join(" ")]);
      if (VERBOSE) console.log("  [warn]", ...a);
    },
    error: (...a) => {
      env.logs.push(["error", a.map(String).join(" ")]);
      if (VERBOSE) console.log("  [error]", ...a);
    },
  };
  const sandbox = {
    console: consoleMock,
    setTimeout: (f, ms) => env.clock.setTimeout(f, ms),
    clearTimeout: (id) => env.clock.clearTimeout(id),
    fetch: fetchMock,
    Office: Office,
    location: { origin: ORIGIN, href: ORIGIN + "/word/commands.html" },
  };
  sandbox.Word = makeWord(env);
  vm.createContext(sandbox);
  SCRIPTS.forEach((s) => s.runInContext(sandbox));
  env.sb = sandbox;
  env.Office = Office;
  env.cmds = sandbox.OpenRefCommands.COMMANDS;
  env.LIMIT_MS = sandbox.OpenRefCommands.LIMIT_MS;
  return env;
}

// ---- テスト補助 ----
function newEvent(name) {
  const ev = {
    count: 0,
    source: { id: name },
    completed() {
      ev.count++;
    },
  };
  return ev;
}
function setCursor(env, paraIndex, off) {
  const p = env.model.paras[paraIndex];
  env.model.sel = { para: p, start: off, end: off };
}
function cursorInCC(env, tag) {
  const cc = env.model.allCCs().find((c) => c.tag === tag);
  assert(cc, "CC がない: " + tag);
  if (cc.kind === "block") {
    env.model.sel = { para: env.model.blockParas(cc)[0], start: 0, end: 0 };
  } else {
    const f = env.model.findInline(cc);
    env.model.sel = { para: f.para, start: f.start + 1, end: f.start + 1 };
  }
}
function citeCCs(env) {
  return env.model.byTag("refract-cite-");
}
function ccByTag(env, tag) {
  return env.model.allCCs().find((c) => c.tag === tag);
}
/** 1 つ開いているダイアログを返す（無ければ失敗） */
function dialog(env, page) {
  const d = env.dialogHost.current;
  assert(d, "ダイアログが開いていない（期待: " + page + "）");
  if (page) assert.strictEqual(d.page, page);
  return d;
}
function noDialog(env) {
  assert.strictEqual(env.dialogHost.current, null, "ダイアログが開いたまま");
}
/** 1 コマンドの完了後: completed はちょうど 1 回（時間を進めても増えない）・ダイアログは残っていない・仮置きなし */
async function settled(env, ev) {
  await env.clock.advance(10 * 60 * 1000);
  assert.strictEqual(ev.count, 1, "event.completed() の回数 = " + ev.count);
  noDialog(env);
}
/** Add/Edit → insert まで（追加モード）。戻り値 = 確定後の引用 tag */
async function addCitation(env, keys) {
  const ev = newEvent("addEditCitation");
  const before = citeCCs(env).map((c) => c.tag);
  env.cmds.addEditCitation(ev);
  await flush();
  const dlg = dialog(env, "cite-dialog.html");
  assert.strictEqual(dlg.d.mode, "add");
  dlg.send({ type: "insert", keys: keys });
  await settled(env, ev);
  const added = citeCCs(env).filter((c) => !before.includes(c.tag));
  assert.strictEqual(added.length, 1, "引用が 1 つ増える");
  return added[0].tag;
}

// ======================================================================
// テスト
// ======================================================================
const tests = [];
function test(name, fn) {
  tests.push({ name: name, fn: fn });
}

test("登録: 7 関数を Office.actions.associate + window に", async () => {
  const env = makeEnv();
  await flush();
  const names = ["addEditCitation", "updateCitations", "citationStyle", "documentSettings", "moveBibliography", "removeCitationCodes", "showDiagnostics"];
  names.forEach((n) => {
    assert.strictEqual(typeof env.associated[n], "function", "associate されていない: " + n);
    assert.strictEqual(typeof env.sb[n], "function", "window に無い: " + n);
  });
  assert.deepStrictEqual(Object.keys(env.associated).sort(), names.slice().sort());
});

test("追加: 仮置き → insert → 付け替え・payload・台帳・番号・文献リスト・カーソル", async () => {
  const env = makeEnv();
  setCursor(env, 0, 31); // "increasing" と "." の間
  const ev = newEvent("addEditCitation");
  env.cmds.addEditCitation(ev);
  await flush();
  const dlg = dialog(env, "cite-dialog.html");
  assert.strictEqual(dlg.origin, ORIGIN, "ダイアログは host と同一オリジン");
  assert.deepStrictEqual({ width: dlg.opts.width, height: dlg.opts.height }, { width: 42, height: 50 });
  // 収録どおり: 仮置きが先に文書へ入ってからダイアログが開く
  assert.match(dlg.docAtOpen[0], /^Pancreatic cancer is increasing\{refract-pending-[a-z0-9]{8}:\[…\]\}\.$/);
  const pending = env.model.byTag("refract-pending-");
  assert.strictEqual(pending.length, 1);
  assert.strictEqual(pending[0].title, "OpenRef citation (pending)");
  const id = pending[0].tag.slice("refract-pending-".length);
  // ダイアログは非モーダル: 開いている間に打った文字が仮置きに入らないよう、カーソルは仮置きの直後（外）
  const pf = env.model.findInline(pending[0]);
  assert.deepStrictEqual([env.model.sel.para, env.model.sel.start, env.model.sel.end], [pf.para, pf.end, pf.end]);
  assert.strictEqual(env.model.inlineAt(pf.para, pf.end, pf.end), null);
  assert.deepStrictEqual(dlg.d, { mode: "add", items: [], style: { id: "american-journal-of-roentgenology", title: "AJR" } });
  assert.strictEqual(ev.count, 0, "ダイアログが閉じるまで completed しない");

  dlg.send({ type: "insert", keys: ["K1", "K2"] });
  await settled(env, ev);
  const tag = "refract-cite-" + id;
  const cc = ccByTag(env, tag);
  assert(cc, "pending → cite に付け替え（同じ 8 桁）");
  assert.strictEqual(cc.title, "OpenRef citation");
  assert.strictEqual(env.model.byTag("refract-pending-").length, 0);
  const s = env.persisted;
  assert.deepStrictEqual(
    s[tag].items.map((it) => it.key),
    ["K1", "K2"]
  );
  assert.strictEqual(s[tag].items[0].csl.title, LIBRARY.K1.title, "payload に CSL snapshot");
  assert.deepStrictEqual(s["refract-index"], [tag], "台帳に追記");
  assert.strictEqual(s["refract-style"], "american-journal-of-roentgenology", "引用の無い新規文書 = app の既定を固定");
  assert.strictEqual(s["refract-doi-always"], true);
  const doc = env.model.dump();
  assert.strictEqual(doc[0], "Pancreatic cancer is increasing{" + tag + ":[1, 2]}.");
  assert.strictEqual(doc[1], "Second paragraph.");
  assert.strictEqual(doc[2], "[refract-bib-heading] References");
  assert.strictEqual(doc.length, 5, "見出し + 文献 2 本");
  assert.match(doc[3], /^\[refract-bib\] 1\. Stoffel EM/);
  assert.match(doc[4], /^\[refract-bib\] 2\. Gaballah AH/);
  assert.deepStrictEqual(cc.font, { name: "Century", size: 10.5, color: "#000000" });
  assert.strictEqual(ccByTag(env, "refract-bib-heading").font.bold, true);
  // カーソルは引用の直後（引用の外）
  const f = env.model.findInline(cc);
  assert.deepStrictEqual([env.model.sel.para, env.model.sel.start, env.model.sel.end], [f.para, f.end, f.end]);
  assert.strictEqual(env.model.inlineAt(f.para, env.model.sel.start, env.model.sel.end), null);
});

test("キャンセル: 仮置きが消えて文書は元どおり・設定は保存しない", async () => {
  const env = makeEnv();
  setCursor(env, 0, 10);
  const before = env.model.dump();
  const ev = newEvent("addEditCitation");
  env.cmds.addEditCitation(ev);
  await flush();
  assert.notDeepStrictEqual(env.model.dump(), before, "仮置きが入っている");
  dialog(env, "cite-dialog.html").send({ type: "cancel" });
  await flush();
  assert.strictEqual(ev.count, 1, "時間切れを待たずに終わる");
  await settled(env, ev);
  assert.deepStrictEqual(env.model.dump(), before);
  assert.strictEqual(env.stats.saves, 0);
  assert.deepStrictEqual(env.persisted, {});
});

test("× で閉じる（12006）: キャンセルと同じ", async () => {
  const env = makeEnv();
  setCursor(env, 1, 6);
  const before = env.model.dump();
  const ev = newEvent("addEditCitation");
  env.cmds.addEditCitation(ev);
  await flush();
  dialog(env, "cite-dialog.html").userClose();
  await flush();
  assert.strictEqual(ev.count, 1, "時間切れを待たずに終わる");
  assert.deepStrictEqual(env.model.dump(), before);
  await settled(env, ev);
});

test("ダイアログの読込失敗（12002）: 閉じて仮置きを消し、理由を error notice で", async () => {
  const env = makeEnv();
  setCursor(env, 0, 31);
  const before = env.model.dump();
  const ev = newEvent("addEditCitation");
  env.cmds.addEditCitation(ev);
  await flush();
  const dlg = dialog(env, "cite-dialog.html");
  dlg.loadFail(12002);
  await flush();
  assert.notStrictEqual(dlg.state, "open", "読込失敗のダイアログは閉じる");
  await env.clock.advance(1000);
  const n = dialog(env, "notice.html");
  assert.deepStrictEqual(n.d, {
    kind: "error",
    message: "ダイアログを表示できませんでした（12002）",
    detail: ORIGIN + "/word/cite-dialog.html の配信（connectd）を確認してください。",
  });
  assert.deepStrictEqual(env.model.dump(), before, "仮置きは消えた");
  n.send({ type: "close" });
  await settled(env, ev);
});

// 後半（打ち切りの後に本体が終わる）は同一 runtime 内での振る舞い。実際の Office では completed で runtime が止まる
// （各コマンドが別 runtime）ので本体の続きは走らない — ここで確かめるのは completed が 2 回目を呼ばないことだけ
test("最終手段: 本体が止まっても 4:55 で completed（同一 runtime 内での振る舞い: 後から本体が終わっても completed は 1 回）", async () => {
  const env = makeEnv();
  setCursor(env, 0, 31);
  const ev = newEvent("addEditCitation");
  env.cmds.addEditCitation(ev);
  await flush();
  let release;
  env.hang = new Promise((r) => (release = r)); // /api/items が返らない
  dialog(env, "cite-dialog.html").send({ type: "insert", keys: ["K1"] });
  await env.clock.advance(4 * 60 * 1000 + 54 * 1000);
  assert.strictEqual(ev.count, 0);
  await env.clock.advance(2000);
  assert.strictEqual(ev.count, 1, "4:55 で打ち切り");
  assert(env.logs.some((l) => l[0] === "error" && /時間内に終わらない/.test(l[1])));
  release();
  await env.clock.advance(60 * 1000);
  assert.strictEqual(ev.count, 1, "本体が後から終わっても completed は 1 回");
});

test("時間切れ: 4:45 でダイアログを閉じ・仮置きを消し・completed（それより前は待つ）", async () => {
  const env = makeEnv();
  setCursor(env, 0, 0);
  const before = env.model.dump();
  const ev = newEvent("addEditCitation");
  env.cmds.addEditCitation(ev);
  await flush();
  const dlg = dialog(env, "cite-dialog.html");
  assert.strictEqual(env.LIMIT_MS, 285000);
  await env.clock.advance(env.LIMIT_MS - 1000);
  assert.strictEqual(ev.count, 0);
  assert.strictEqual(dlg.state, "open");
  assert.strictEqual(env.model.byTag("refract-pending-").length, 1);
  await env.clock.advance(1000);
  assert.notStrictEqual(dlg.state, "open", "dialog.close() が呼ばれた");
  assert.strictEqual(ev.count, 1);
  assert.deepStrictEqual(env.model.dump(), before);
  await settled(env, ev);
});

test("編集: 引用の中で Add/Edit → d に既存 keys → replace で差し替え・番号は初出順", async () => {
  const env = makeEnv({ paras: ["First claim.", "Second claim.", "Tail."] });
  setCursor(env, 0, 11);
  const a = await addCitation(env, ["K1"]);
  setCursor(env, 1, 12);
  const b = await addCitation(env, ["K2"]);
  assert.strictEqual(env.model.dump()[0], "First claim{" + a + ":[1]}.");
  assert.strictEqual(env.model.dump()[1], "Second claim{" + b + ":[2]}.");
  const saves = env.stats.saves;

  cursorInCC(env, a);
  const ev = newEvent("addEditCitation");
  env.cmds.addEditCitation(ev);
  await flush();
  const dlg = dialog(env, "cite-dialog.html");
  assert.deepStrictEqual(dlg.d.mode, "edit");
  assert.deepStrictEqual(dlg.d.items, [{ key: "K1", label: "Stoffel 2023" }]);
  assert.strictEqual(env.model.byTag("refract-pending-").length, 0, "編集では仮置きを入れない");
  dlg.send({ type: "replace", keys: ["K3", "K2"] });
  await settled(env, ev);
  const doc = env.model.dump();
  assert.strictEqual(doc[0], "First claim{" + a + ":[1, 2]}.");
  assert.strictEqual(doc[1], "Second claim{" + b + ":[2]}.");
  assert.deepStrictEqual(
    env.persisted[a].items.map((x) => x.key),
    ["K3", "K2"]
  );
  assert.match(doc[4], /^\[refract-bib\] 1\. Turkbey B/);
  assert.match(doc[5], /^\[refract-bib\] 2\. Gaballah AH/);
  assert.strictEqual(doc.length, 6);
  assert.deepStrictEqual(env.persisted["refract-index"], [a, b], "編集で台帳は増えない");
  assert(env.stats.saves > saves);
});

test("編集で 0 件 → delete: 最後の 1 件なら文献リストと見出しも消える", async () => {
  const env = makeEnv();
  setCursor(env, 0, 31);
  const tag = await addCitation(env, ["K1"]);
  cursorInCC(env, tag);
  const ev = newEvent("addEditCitation");
  env.cmds.addEditCitation(ev);
  await flush();
  dialog(env, "cite-dialog.html").send({ type: "delete" });
  await settled(env, ev);
  assert.deepStrictEqual(env.model.dump(), ["Pancreatic cancer is increasing.", "Second paragraph."]);
  assert.strictEqual(env.model.allCCs().length, 0);
  assert.strictEqual(env.persisted[tag], undefined, "payload 削除");
  assert.deepStrictEqual(env.persisted["refract-index"], [], "台帳から外す");
});

test("編集で 0 件 → delete: 他の引用が残れば文献リストは残って番号を振り直す", async () => {
  const env = makeEnv({ paras: ["A.", "B."] });
  setCursor(env, 0, 1);
  const a = await addCitation(env, ["K1"]);
  setCursor(env, 1, 1);
  const b = await addCitation(env, ["K2"]);
  cursorInCC(env, a);
  const ev = newEvent("addEditCitation");
  env.cmds.addEditCitation(ev);
  await flush();
  dialog(env, "cite-dialog.html").send({ type: "delete" });
  await settled(env, ev);
  const doc = env.model.dump();
  assert.deepStrictEqual(doc.slice(0, 3), ["A.", "B{" + b + ":[1]}.", "[refract-bib-heading] References"]);
  assert.match(doc[3], /^\[refract-bib\] 1\. Gaballah AH/);
  assert.strictEqual(doc.length, 4);
});

test("文献リストの中で Add/Edit → error notice（仮置きは入れない）", async () => {
  const env = makeEnv();
  setCursor(env, 0, 31);
  await addCitation(env, ["K1"]);
  const before = env.model.dump();
  cursorInCC(env, "refract-bib");
  const ev = newEvent("addEditCitation");
  env.cmds.addEditCitation(ev);
  await flush();
  const n = dialog(env, "notice.html");
  assert.deepStrictEqual(n.d, { kind: "error", message: "文献リストの中には引用を挿入できません。本文にカーソルを置いてください。" });
  assert.deepStrictEqual({ width: n.opts.width, height: n.opts.height }, { width: 30, height: 28 });
  assert.deepStrictEqual(env.model.dump(), before);
  assert.strictEqual(ev.count, 0, "notice が閉じるまで待つ");
  n.send({ type: "close" });
  await settled(env, ev);
  // 見出しの中も同じ
  cursorInCC(env, "refract-bib-heading");
  const ev2 = newEvent("addEditCitation");
  env.cmds.addEditCitation(ev2);
  await flush();
  assert.strictEqual(dialog(env, "notice.html").d.kind, "error");
  dialog(env).userClose();
  await settled(env, ev2);
});

test("Update: 引用 0 件 → error notice の文言（Paperpile の OpenRef 版）", async () => {
  const env = makeEnv();
  const ev = newEvent("updateCitations");
  env.cmds.updateCitations(ev);
  await flush();
  const n = dialog(env, "notice.html");
  assert.deepStrictEqual(n.d, {
    kind: "error",
    message: "この文書に OpenRef の引用が見つかりません。",
    detail: "Add/Edit Citation で引用を挿入してください。",
  });
  n.send({ type: "close" });
  await settled(env, ev);
  assert.deepStrictEqual(env.model.dump(), ["Pancreatic cancer is increasing.", "Second paragraph."]);
});

test("Update 正常: 蔵書の書誌の変更が反映・ダイアログは出さない", async () => {
  const env = makeEnv();
  setCursor(env, 0, 31);
  const tag = await addCitation(env, ["K1"]);
  env.library.K1 = Object.assign({}, env.library.K1, { title: "Pancreatic cancer: an updated title" });
  const opened = env.dialogHost.dialogs.length;
  const ev = newEvent("updateCitations");
  env.cmds.updateCitations(ev);
  await settled(env, ev);
  assert.strictEqual(env.dialogHost.dialogs.length, opened, "成功時はダイアログなし");
  assert.match(env.model.dump()[3], /an updated title/);
  assert.strictEqual(env.model.dump()[0], "Pancreatic cancer is increasing{" + tag + ":[1]}.");
});

test("Update オフライン: /api/items が落ちても文書内 snapshot で描画", async () => {
  const env = makeEnv();
  setCursor(env, 0, 31);
  await addCitation(env, ["K1", "K3"]);
  env.library.K1 = Object.assign({}, env.library.K1, { title: "SHOULD NOT APPEAR" });
  env.offline = true;
  const ev = newEvent("updateCitations");
  env.cmds.updateCitations(ev);
  await settled(env, ev);
  const doc = env.model.dump().join("\n");
  assert(!doc.includes("SHOULD NOT APPEAR"));
  assert.match(doc, /Stoffel EM, Goggins M\. Pancreatic cancer: changing epidemiology/);
});

test("Citation Style...: tab=style・初期値 → apply でスタイル・DOI・フォントを保存して再整形", async () => {
  const env = makeEnv();
  setCursor(env, 0, 31);
  const tag = await addCitation(env, ["K1", "K2"]);
  const ev = newEvent("citationStyle");
  env.cmds.citationStyle(ev);
  await flush();
  const dlg = dialog(env, "settings-dialog.html");
  assert.strictEqual(dlg.params.tab, "style");
  assert.deepStrictEqual(dlg.d, {
    style: "american-journal-of-roentgenology",
    doiAlways: true,
    fontName: "Century",
    fontSize: 10.5,
    cites: 1,
    pinned: true,
  });
  assert.deepStrictEqual({ width: dlg.opts.width, height: dlg.opts.height }, { width: 40, height: 60 });
  dlg.send({ type: "apply", style: "american-medical-association", doiAlways: false, fontName: "Times New Roman", fontSize: 12 });
  await settled(env, ev);
  const s = env.persisted;
  assert.strictEqual(s["refract-style"], "american-medical-association");
  assert.strictEqual(s["refract-doi-always"], false);
  assert.strictEqual(s["refract-font-name"], "Times New Roman");
  assert.strictEqual(s["refract-font-size"], 12);
  assert.strictEqual(env.model.dump()[0], "Pancreatic cancer is increasing{" + tag + ":1,2}.", "AMA は上付き 1,2");
  assert.deepStrictEqual(ccByTag(env, tag).font, { name: "Times New Roman", size: 12, color: "#000000" });
  // Document Settings... は tab=doc で開く・キャンセルは何も変えない
  const ev2 = newEvent("documentSettings");
  env.cmds.documentSettings(ev2);
  await flush();
  const dlg2 = dialog(env, "settings-dialog.html");
  assert.strictEqual(dlg2.params.tab, "doc");
  assert.strictEqual(dlg2.d.style, "american-medical-association");
  const saves = env.stats.saves;
  dlg2.send({ type: "cancel" });
  await settled(env, ev2);
  assert.strictEqual(env.stats.saves, saves);
});

test("設定: 引用の無い文書でも apply で固定（以後の挿入はその設定）", async () => {
  const env = makeEnv();
  const ev = newEvent("documentSettings");
  env.cmds.documentSettings(ev);
  await flush();
  const dlg = dialog(env, "settings-dialog.html");
  assert.strictEqual(dlg.d.cites, 0);
  assert.strictEqual(dlg.d.pinned, false);
  assert.strictEqual(dlg.d.style, "american-journal-of-roentgenology", "未固定の新規文書は app の既定を表示");
  dlg.send({ type: "apply", style: "radiology", doiAlways: false, fontName: "", fontSize: "abc" });
  await settled(env, ev);
  assert.strictEqual(env.persisted["refract-style"], "radiology");
  assert.strictEqual(env.persisted["refract-font-name"], "Century", "空 = 既定");
  assert.strictEqual(env.persisted["refract-font-size"], 10.5, "不正 = 既定");
  setCursor(env, 0, 31);
  const tag = await addCitation(env, ["K1"]);
  assert.strictEqual(env.persisted["refract-style"], "radiology", "挿入で上書きしない");
  assert.strictEqual(env.model.dump()[0], "Pancreatic cancer is increasing{" + tag + ":(1)}.");
});

test("Move bibliography here: カーソル段落の直後へ作り直す", async () => {
  const env = makeEnv({ paras: ["Intro text.", "Discussion.", "Conclusion."] });
  setCursor(env, 0, 10);
  const tag = await addCitation(env, ["K1", "K2"]);
  assert.strictEqual(env.model.dump()[3], "[refract-bib-heading] References");
  setCursor(env, 1, 3);
  const ev = newEvent("moveBibliography");
  env.cmds.moveBibliography(ev);
  await settled(env, ev);
  const doc = env.model.dump();
  assert.strictEqual(doc[0], "Intro text{" + tag + ":[1, 2]}.");
  assert.strictEqual(doc[1], "Discussion.");
  assert.strictEqual(doc[2], "[refract-bib-heading] References");
  assert.match(doc[3], /^\[refract-bib\] 1\. Stoffel/);
  assert.match(doc[4], /^\[refract-bib\] 2\. Gaballah/);
  assert.strictEqual(doc[5], "Conclusion.");
  assert.strictEqual(doc.length, 6, "旧位置の見出し・文献リストは消えた");
  assert.strictEqual(env.model.byTag("refract-bib").filter((c) => c.tag === "refract-bib").length, 1);
  assert.strictEqual(env.model.paras[2].style, "Normal");
  assert.strictEqual(ccByTag(env, "refract-bib-heading").font.bold, true);
});

test("Move bibliography here: 見出し CC の無い旧文書は直前の素の References も移す（二重見出しにしない）", async () => {
  const p0 = new Para("Body.");
  const p1 = new Para("Later.");
  const ref = new Para("References");
  const bibP = new Para("1. stale");
  const cc = new CCNode("inline");
  cc.tag = "refract-cite-legacy01";
  cc.items = [{ t: "text", text: "(1)" }];
  p0.items = [{ t: "text", text: "Body" }, { t: "cc", cc: cc }, { t: "text", text: "." }];
  const bib = new CCNode("block");
  bib.tag = "refract-bib";
  bibP.block = bib;
  const env = makeEnv({
    paras: [p0, p1, ref, bibP],
    settings: { "refract-cite-legacy01": { items: [{ key: "K1", csl: LIBRARY.K1 }] } },
  });
  setCursor(env, 0, 2);
  const ev = newEvent("moveBibliography");
  env.cmds.moveBibliography(ev);
  await settled(env, ev);
  const doc = env.model.dump();
  assert.strictEqual(doc[0], "Body{refract-cite-legacy01:(1)}.");
  assert.strictEqual(doc[1], "[refract-bib-heading] References");
  assert.match(doc[2], /^\[refract-bib\] 1\. Stoffel EM/);
  assert.strictEqual(doc[3], "Later.");
  assert.strictEqual(doc.length, 4, "旧位置の素の References と文献リストは消えた");
});

test("引用ダイアログの URL が 2000 字を超えるなら label を落として key だけ渡す", async () => {
  const lib = clone(LIBRARY);
  const keys = [];
  for (let i = 0; i < 40; i++) {
    const k = "KX" + String(i).padStart(6, "0");
    keys.push(k);
    lib[k] = { type: "article-journal", title: "Item " + i, author: [{ family: "Longfamilyname" + i, given: "A" }], issued: { "date-parts": [[2000 + (i % 25)]] } };
  }
  const env = makeEnv({ library: lib });
  setCursor(env, 0, 31);
  const tag = await addCitation(env, keys);
  cursorInCC(env, tag);
  const ev = newEvent("addEditCitation");
  env.cmds.addEditCitation(ev);
  await flush();
  const dlg = dialog(env, "cite-dialog.html");
  assert.strictEqual(dlg.d.items.length, 40);
  assert(dlg.d.items.every((it) => it.key && it.label === undefined), "label なし（key だけ）");
  assert(dlg.url.length < 3000, "URL " + dlg.url.length);
  dlg.send({ type: "replace", keys: keys.slice(0, 2) });
  await settled(env, ev);
  assert.deepStrictEqual(
    env.persisted[tag].items.map((x) => x.key),
    keys.slice(0, 2)
  );
});

test("Move bibliography here: 引用の中・文献リストの中・引用 0 件はエラー", async () => {
  const env = makeEnv();
  const ev0 = newEvent("moveBibliography");
  env.cmds.moveBibliography(ev0);
  await flush();
  assert.strictEqual(dialog(env, "notice.html").d.message, "この文書に OpenRef の引用が見つかりません。");
  dialog(env).send({ type: "close" });
  await settled(env, ev0);

  setCursor(env, 0, 31);
  const tag = await addCitation(env, ["K1"]);
  const before = env.model.dump();
  for (const where of [tag, "refract-bib", "refract-bib-heading"]) {
    cursorInCC(env, where);
    const ev = newEvent("moveBibliography");
    env.cmds.moveBibliography(ev);
    await flush();
    const n = dialog(env, "notice.html");
    assert.deepStrictEqual(n.d, {
      kind: "error",
      message: "本文の段落にカーソルを置いてください。",
      detail: "引用・文献リスト・References 見出し・仮置きの中には移動できません。",
    });
    n.send({ type: "close" });
    await settled(env, ev);
  }
  assert.deepStrictEqual(env.model.dump(), before);
});

test("Remove Citation Codes: 確認 → テキストは残り CC・payload・台帳（孤児含む）が消える・仮置きは […] だけなら中身ごと / 打った文字は残す", async () => {
  const env = makeEnv({ paras: ["A claim.", "B claim.", "C."] });
  setCursor(env, 0, 7);
  const a = await addCitation(env, ["K1"]);
  setCursor(env, 1, 7);
  const b = await addCitation(env, ["K2", "K1"]);
  // 孤児（削除済み引用の payload・台帳には残っている）と仮置きの残骸 2 つ（[…] だけ / 利用者が文字を打ったもの）
  env.settings.set("refract-cite-orphan01", { items: [{ key: "K3", csl: LIBRARY.K3 }] });
  env.settings.set("refract-index", [a, b, "refract-cite-orphan01"]);
  await new Promise((r) => env.settings.saveAsync(r));
  await Word_insertPending(env, 2, 1, "refract-pending-leftover");
  await Word_insertPending(env, 2, 0, "refract-pending-typed001", "abc");
  assert.strictEqual(env.model.dump()[2], "{refract-pending-typed001:[…]abc}C{refract-pending-leftover:[…]}.");
  const textBefore = env.model.dump().map((l) => l.replace(/\{[^:]*:([^}]*)\}/g, "$1").replace(/^\[[^\]]*\] /, ""));

  // キャンセルなら何も変わらない
  const ev0 = newEvent("removeCitationCodes");
  env.cmds.removeCitationCodes(ev0);
  await flush();
  const c0 = dialog(env, "notice.html");
  assert.strictEqual(c0.d.kind, "confirm");
  assert.strictEqual(c0.d.ok, "削除");
  assert.strictEqual(c0.d.danger, true);
  assert.match(c0.d.message, /^引用と文献リストを通常のテキストに変換し、文書内の引用データ（書誌のスナップショット）も削除します。/);
  assert(c0.d.message.endsWith("旧版で削除した引用の控えは文書内に残ることがあります。"), c0.d.message);
  c0.send({ type: "cancel" });
  await settled(env, ev0);
  assert.strictEqual(citeCCs(env).length, 2);

  const ev = newEvent("removeCitationCodes");
  env.cmds.removeCitationCodes(ev);
  await flush();
  dialog(env, "notice.html").send({ type: "ok" });
  await settled(env, ev);
  assert.strictEqual(env.model.allCCs().length, 0, "OpenRef の CC は全部外れた");
  const doc = env.model.dump();
  assert.strictEqual(doc[0], "A claim[1].");
  assert.strictEqual(doc[1], "B claim[1, 2].", "クラスタ内は番号順（citeproc）");
  assert.strictEqual(doc[2], "abcC.", "[…] だけの仮置きは中身ごと消え、文字を打った仮置きは […] を消して CC だけ外れる");
  assert.strictEqual(doc[3], "References");
  assert.match(doc[4], /^1\. Stoffel/);
  textBefore[2] = "abcC.";
  assert.deepStrictEqual(doc, textBefore, "表示テキストはそのまま残る");
  for (const k of [a, b, "refract-cite-orphan01", "refract-index"]) assert.strictEqual(env.persisted[k], undefined, "消えていない: " + k);
  assert.strictEqual(env.persisted["refract-style"], "american-journal-of-roentgenology", "文書のスタイル設定は残す");
});

test("Remove Citation Codes: 何も無い文書は error notice", async () => {
  const env = makeEnv();
  const ev = newEvent("removeCitationCodes");
  env.cmds.removeCitationCodes(ev);
  await flush();
  assert.strictEqual(dialog(env, "notice.html").d.kind, "error");
  dialog(env).userClose();
  await settled(env, ev);
});

test("旧文書（台帳なし・style 未保存・引用あり）: 従来どおり vancouver で更新・見出しの legacy 取り込み", async () => {
  // 旧版で作った文書: 引用 CC 2 つ + 文献リスト CC + 素の "References" 段落（見出し CC なし）
  const p0 = new Para("Old one.");
  const p1 = new Para("Old two.");
  const ref = new Para("References");
  const bibP = new Para("stale bibliography");
  const env = makeEnv({
    paras: [p0, p1, ref, bibP],
    settings: {
      "refract-cite-aaaaaaaa": { items: [{ key: "K2", csl: LIBRARY.K2 }] },
      "refract-cite-bbbbbbbb": { items: [{ key: "K1", csl: LIBRARY.K1 }, { key: "K2", csl: LIBRARY.K2 }] },
    },
  });
  [["refract-cite-aaaaaaaa", p0], ["refract-cite-bbbbbbbb", p1]].forEach(([tag, p]) => {
    const cc = new CCNode("inline");
    cc.tag = tag;
    cc.title = "Refract citation"; // 改名前の版が付けた title（照合には使わない）
    cc.items = [{ t: "text", text: "[?]" }];
    p.items = [{ t: "text", text: "Old " + (tag === "refract-cite-aaaaaaaa" ? "one" : "two") }, { t: "cc", cc: cc }, { t: "text", text: "." }];
  });
  const bib = new CCNode("block");
  bib.tag = "refract-bib";
  bibP.block = bib;

  const ev = newEvent("updateCitations");
  env.cmds.updateCitations(ev);
  await settled(env, ev);
  const doc = env.model.dump();
  assert.strictEqual(doc[0], "Old one{refract-cite-aaaaaaaa:(1)}.");
  assert.strictEqual(doc[1], "Old two{refract-cite-bbbbbbbb:(1,2)}.", "K2 = 1（初出）・K1 = 2");
  assert.strictEqual(doc[2], "[refract-bib-heading] References", "素の References を見出し CC に取り込む");
  assert.match(doc[3], /^\[refract-bib\] 1\. Gaballah AH/);
  assert.match(doc[4], /^\[refract-bib\] 2\. Stoffel EM/);
  assert.strictEqual(doc.length, 5);
  assert.strictEqual(env.persisted["refract-style"], undefined, "Update ではスタイルを固定しない");
  assert.strictEqual(env.persisted["refract-index"], undefined, "Update で台帳を作らない（自動掃除もしない）");

  // その旧文書に追加 → 引用が既にあるので vancouver・DOI 付け足し無しで固定（app の既定 AJR + DOI ではない）
  setCursor(env, 1, 3);
  const tag = await addCitation(env, ["K3"]);
  assert.strictEqual(env.persisted["refract-style"], "vancouver");
  assert.strictEqual(env.persisted["refract-doi-always"], false);
  assert.deepStrictEqual(env.persisted["refract-index"], [tag]);
  // 初出順: K2（1 段落目）= 1 → 新しい K3 = 2 → K1 = 3
  assert.strictEqual(env.model.dump()[1], "Old{" + tag + ":(2)} two{refract-cite-bbbbbbbb:(1,3)}.");
});

test("12007: 開けなければ 300ms 間隔で再試行して開く", async () => {
  const env = makeEnv();
  env.dialogHost.fail12007 = 3;
  setCursor(env, 0, 5);
  const ev = newEvent("addEditCitation");
  env.cmds.addEditCitation(ev);
  await flush();
  assert.strictEqual(env.dialogHost.current, null);
  await env.clock.advance(3 * 300);
  const dlg = dialog(env, "cite-dialog.html");
  assert.strictEqual(env.dialogHost.rejected, 3);
  dlg.send({ type: "cancel" });
  await settled(env, ev);
});

test("確定の失敗（蔵書に無い key）: 仮置きを消し、閉じかけのダイアログの後に error notice（12007 再試行）", async () => {
  const env = makeEnv();
  setCursor(env, 0, 31);
  const before = env.model.dump();
  const ev = newEvent("addEditCitation");
  env.cmds.addEditCitation(ev);
  await flush();
  dialog(env, "cite-dialog.html").send({ type: "insert", keys: ["K1", "NOPE1234"] });
  await flush();
  assert.strictEqual(env.dialogHost.current, null, "cite ダイアログは閉じかけ（まだ notice は開けない）");
  await env.clock.advance(1000);
  const n = dialog(env, "notice.html");
  assert(env.dialogHost.rejected >= 1, "閉じかけの間は 12007");
  assert.deepStrictEqual(n.d, {
    kind: "error",
    message: "蔵書に見つからない文献があります: NOPE1234",
    detail: "ゴミ箱に移した・削除した文献は引用できません。",
  });
  assert.deepStrictEqual(env.model.dump(), before, "仮置きは消えた");
  assert.strictEqual(ev.count, 0);
  n.send({ type: "close" });
  await settled(env, ev);
  assert.strictEqual(env.persisted["refract-index"], undefined);
});

test("確定の失敗（文書設定の保存に失敗）: payload の無い引用を残さない（仮置きごと消して error notice）", async () => {
  const env = makeEnv();
  setCursor(env, 0, 31);
  const before = env.model.dump();
  const ev = newEvent("addEditCitation");
  env.cmds.addEditCitation(ev);
  await flush();
  env.failSave = true;
  dialog(env, "cite-dialog.html").send({ type: "insert", keys: ["K1"] });
  await env.clock.advance(1000);
  const n = dialog(env, "notice.html");
  assert.strictEqual(n.d.kind, "error");
  assert.strictEqual(n.d.message, "文書設定の保存に失敗: mock: 保存に失敗");
  assert.deepStrictEqual(env.model.dump(), before, "引用 CC も仮置きも残らない");
  assert.strictEqual(citeCCs(env).length, 0);
  assert.deepStrictEqual(env.persisted, {});
  n.send({ type: "close" });
  await settled(env, ev);
});

test("仮置きの残骸の中で Add/Edit → 再利用（新しい仮置きを作らない）", async () => {
  const env = makeEnv();
  await Word_insertPending(env, 0, 8, "refract-pending-zz9yy8xx");
  cursorInCC(env, "refract-pending-zz9yy8xx");
  const ev = newEvent("addEditCitation");
  env.cmds.addEditCitation(ev);
  await flush();
  const dlg = dialog(env, "cite-dialog.html");
  assert.strictEqual(dlg.d.mode, "add");
  assert.strictEqual(env.model.byTag("refract-pending-").length, 1);
  dlg.send({ type: "insert", keys: ["K2"] });
  await settled(env, ev);
  assert(ccByTag(env, "refract-cite-zz9yy8xx"), "残骸の 8 桁で確定");
  assert.strictEqual(env.model.dump()[0], "Pancreat{refract-cite-zz9yy8xx:[1]}ic cancer is increasing.");
});

test("payload の無い引用（別文書からのコピー）: 編集モード 0 件 + missing → replace で付け直せる", async () => {
  const p0 = new Para("");
  const cc = new CCNode("inline");
  cc.tag = "refract-cite-copied00";
  cc.items = [{ t: "text", text: "[7]" }];
  p0.items = [{ t: "text", text: "Copied " }, { t: "cc", cc: cc }, { t: "text", text: " here." }];
  const env = makeEnv({ paras: [p0] });
  cursorInCC(env, "refract-cite-copied00");
  const ev = newEvent("addEditCitation");
  env.cmds.addEditCitation(ev);
  await flush();
  const dlg = dialog(env, "cite-dialog.html");
  assert.deepStrictEqual([dlg.d.mode, dlg.d.items, dlg.d.missing], ["edit", [], true]);
  dlg.send({ type: "replace", keys: ["K1"] });
  await settled(env, ev);
  assert.strictEqual(env.model.dump()[0], "Copied {refract-cite-copied00:(1)} here.", "引用ありの旧文書扱い = vancouver");
});

// 実 Office の関数コマンドはコマンドごとに別の runtime で動く（MS docs）ので、このガードが効くのは
// 同じ runtime に 2 つ目のコマンドが届いた場合だけ。ここではその「同一 runtime 内での二重押下ガード」だけを確かめる
test("同一 runtime 内での二重押下ガード: 実行中に届いた押下は何もせず completed 1 回（先の Add/Edit は続行）", async () => {
  const env = makeEnv();
  setCursor(env, 0, 31);
  const ev1 = newEvent("addEditCitation");
  env.cmds.addEditCitation(ev1);
  await flush();
  const dlg = dialog(env, "cite-dialog.html");
  const ev2 = newEvent("updateCitations");
  env.cmds.updateCitations(ev2);
  const ev3 = newEvent("addEditCitation");
  env.cmds.addEditCitation(ev3);
  await flush();
  assert.strictEqual(ev2.count, 1);
  assert.strictEqual(ev3.count, 1);
  assert.strictEqual(env.dialogHost.dialogs.length, 1);
  assert(env.logs.some((l) => l[0] === "warn" && /実行中/.test(l[1])));
  dlg.send({ type: "insert", keys: ["K1"] });
  await settled(env, ev1);
  assert.strictEqual(citeCCs(env).length, 1);
});

test("Show Diagnostics...: info notice（版・API・connectd・文書）", async () => {
  const env = makeEnv();
  setCursor(env, 0, 31);
  await addCitation(env, ["K1", "K2"]);
  const ev = newEvent("showDiagnostics");
  env.cmds.showDiagnostics(ev);
  await flush();
  const n = dialog(env, "notice.html");
  assert.strictEqual(n.d.kind, "info");
  assert.strictEqual(n.d.copy, true);
  const t = n.d.message;
  [
    "OpenRef Word アドイン 1.2.0",
    "Office: Word / Mac / 16.0.mock",
    "API: WordApi 1.3 ✓ 1.4 ✓ 1.5 ✗ · DialogApi 1.1 ✓ 1.2 ✓",
    "connectd: 接続 OK（v0.1.0・蔵書 3 件・DB v9）",
    "スタイル: AJR（american-journal-of-roentgenology・文書に保存済み）",
    "引用: 1 箇所 · 文献: 2 本",
    "文献リスト: あり · 見出し: あり · 仮置き: 0 · 台帳: 1",
  ].forEach((s) => assert(t.includes(s), "診断に無い: " + s + "\n---\n" + t));
  assert(!t.includes("直近の更新"), "「直近の更新」は出さない（実 Office ではコマンドごとに runtime が別 = 常に空）");
  n.send({ type: "close" });
  await settled(env, ev);
});

test("例外は error notice で見せてから completed（Word の失敗・スタイル取得失敗）", async () => {
  const env = makeEnv();
  setCursor(env, 0, 31);
  await addCitation(env, ["K1"]);
  env.settings.set("refract-style", "no-such-style");
  await new Promise((r) => env.settings.saveAsync(r));
  const ev = newEvent("updateCitations");
  env.cmds.updateCitations(ev);
  await flush();
  const n = dialog(env, "notice.html");
  assert.strictEqual(n.d.kind, "error");
  assert.match(n.d.message, /HTTP 404 \/word\/styles\/no-such-style\.csl/);
  assert(env.logs.some((l) => l[0] === "error"), "console.error にも残す");
  n.send({ type: "close" });
  await settled(env, ev);
});

// ---- 複製・境界・Move bibliography の例外系 ----
test("コピー＆ペーストの複製（同じ tag の CC 2 個）: 片方で 0 件 → その 1 個だけ消え、もう片方・payload・台帳・文献リストは残る", async () => {
  const env = makeEnv({ paras: ["First claim.", "Second claim."] });
  setCursor(env, 0, 11);
  const tag = await addCitation(env, ["K1"]);
  pasteCopy(env, tag, 1, 12);
  assert.strictEqual(env.model.dump()[1], "Second claim{" + tag + ":[1]}.");
  cursorInNth(env, tag, 0);
  const ev = newEvent("addEditCitation");
  env.cmds.addEditCitation(ev);
  await flush();
  const dlg = dialog(env, "cite-dialog.html");
  assert.strictEqual(dlg.d.mode, "edit");
  dlg.send({ type: "delete" });
  await settled(env, ev);
  const doc = env.model.dump();
  assert.strictEqual(doc[0], "First claim.", "消したのは編集した 1 個だけ");
  assert.strictEqual(doc[1], "Second claim{" + tag + ":[1]}.", "複製は残る");
  assert.strictEqual(doc[2], "[refract-bib-heading] References");
  assert.match(doc[3], /^\[refract-bib\] 1\. Stoffel EM/);
  assert.strictEqual(doc.length, 4);
  assert.deepStrictEqual(
    env.persisted[tag].items.map((x) => x.key),
    ["K1"],
    "同じ tag の CC が残るので payload は残す"
  );
  assert.deepStrictEqual(env.persisted["refract-index"], [tag]);
  // 残った 1 個も消すと、今度は payload・台帳・文献リストも消える
  cursorInNth(env, tag, 0);
  const ev2 = newEvent("addEditCitation");
  env.cmds.addEditCitation(ev2);
  await flush();
  dialog(env, "cite-dialog.html").send({ type: "delete" });
  await settled(env, ev2);
  assert.deepStrictEqual(env.model.dump(), ["First claim.", "Second claim."]);
  assert.strictEqual(env.persisted[tag], undefined);
  assert.deepStrictEqual(env.persisted["refract-index"], []);
});

test("コピー＆ペーストの複製: 片方を replace → 編集した 1 個だけ新しい tag に分かれ（fork）、もう片方は旧内容のまま・番号も正しい", async () => {
  const env = makeEnv({ paras: ["First claim.", "Second claim."] });
  setCursor(env, 0, 11);
  const tag = await addCitation(env, ["K1"]);
  pasteCopy(env, tag, 1, 12);
  const edited = cursorInNth(env, tag, 1); // 2 段落目の複製を編集
  const ev = newEvent("addEditCitation");
  env.cmds.addEditCitation(ev);
  await flush();
  const dlg = dialog(env, "cite-dialog.html");
  assert.deepStrictEqual(dlg.d.items, [{ key: "K1", label: "Stoffel 2023" }]);
  dlg.send({ type: "replace", keys: ["K2"] });
  await settled(env, ev);
  assert.notStrictEqual(edited.tag, tag, "編集した CC は新しい tag");
  assert.match(edited.tag, /^refract-cite-[a-z0-9]{8}$/);
  assert.strictEqual(edited.title, "OpenRef citation");
  const doc = env.model.dump();
  assert.strictEqual(doc[0], "First claim{" + tag + ":[1]}.", "もう片方は旧 tag・旧内容");
  assert.strictEqual(doc[1], "Second claim{" + edited.tag + ":[2]}.");
  assert.match(doc[3], /^\[refract-bib\] 1\. Stoffel EM/);
  assert.match(doc[4], /^\[refract-bib\] 2\. Gaballah AH/);
  assert.strictEqual(doc.length, 5);
  assert.deepStrictEqual(
    env.persisted[tag].items.map((x) => x.key),
    ["K1"],
    "旧 payload はそのまま"
  );
  assert.deepStrictEqual(
    env.persisted[edited.tag].items.map((x) => x.key),
    ["K2"]
  );
  assert.deepStrictEqual(env.persisted["refract-index"], [tag, edited.tag], "fork した tag を台帳に追記");
  const f = env.model.findInline(edited);
  assert.deepStrictEqual([env.model.sel.para, env.model.sel.start], [f.para, f.end], "カーソルは編集した引用（id）の直後");
});

test("編集中に引用が消された: replace も delete も error notice・文書と設定は一切変えない", async () => {
  for (const msg of [{ type: "replace", keys: ["K2"] }, { type: "delete" }]) {
    const env = makeEnv();
    setCursor(env, 0, 31);
    const tag = await addCitation(env, ["K1"]);
    cursorInCC(env, tag);
    const ev = newEvent("addEditCitation");
    env.cmds.addEditCitation(ev);
    await flush();
    const dlg = dialog(env, "cite-dialog.html");
    env.model.ccDelete(ccByTag(env, tag), false); // ダイアログを開いている間に利用者が引用を消した
    const before = env.model.dump();
    const persisted = clone(env.persisted);
    const saves = env.stats.saves;
    dlg.send(msg);
    await env.clock.advance(1000);
    const n = dialog(env, "notice.html");
    assert.deepStrictEqual(n.d, {
      kind: "error",
      message: "編集中の引用が文書から見つかりません。",
      detail: "ダイアログを開いている間に削除された可能性があります（文書は変更していません）。",
    });
    assert.deepStrictEqual(env.model.dump(), before, msg.type + ": 文書は不変");
    assert.deepStrictEqual(env.persisted, persisted, msg.type + ": 設定は不変");
    assert.strictEqual(env.stats.saves, saves, msg.type + ": saveAsync も呼ばない");
    n.send({ type: "close" });
    await settled(env, ev);
  }
});

test("Move bibliography here: 選択が本文から文献リストに跨る → 何も変えずにエラー（文献リストを失わない）", async () => {
  const env = makeEnv({ paras: ["Intro text.", "Discussion."] });
  setCursor(env, 0, 10);
  await addCitation(env, ["K1", "K2"]);
  const before = env.model.dump();
  // 2 段落目の途中から文献リスト 1 本目の途中まで（CC の外から始まる選択 = selectionContext は body と判定する）
  const bibP = env.model.paras.find((p) => p.block && p.block.tag === "refract-bib");
  env.model.sel = { para: env.model.paras[1], start: 3, endPara: bibP, end: 5 };
  const ev = newEvent("moveBibliography");
  env.cmds.moveBibliography(ev);
  await flush();
  const n = dialog(env, "notice.html");
  assert.deepStrictEqual(n.d, {
    kind: "error",
    message: "本文の段落にカーソルを置いてください。",
    detail: "引用・文献リスト・References 見出し・仮置きの中には移動できません。",
  });
  assert.deepStrictEqual(env.model.dump(), before, "文書は不変");
  n.send({ type: "close" });
  await settled(env, ev);
});

test("Move bibliography here: 見出し CC の無い旧文書で素の References 段落にカーソル → 何も変えずにエラー", async () => {
  const p0 = new Para("Body.");
  const ref = new Para("References");
  const bibP = new Para("1. stale");
  const cc = new CCNode("inline");
  cc.tag = "refract-cite-legacy02";
  cc.items = [{ t: "text", text: "(1)" }];
  p0.items = [{ t: "text", text: "Body" }, { t: "cc", cc: cc }, { t: "text", text: "." }];
  const bib = new CCNode("block");
  bib.tag = "refract-bib";
  bibP.block = bib;
  const env = makeEnv({
    paras: [p0, ref, bibP],
    settings: { "refract-cite-legacy02": { items: [{ key: "K1", csl: LIBRARY.K1 }] } },
  });
  const before = env.model.dump();
  setCursor(env, 1, 3);
  const ev = newEvent("moveBibliography");
  env.cmds.moveBibliography(ev);
  await flush();
  assert.strictEqual(dialog(env, "notice.html").d.message, "本文の段落にカーソルを置いてください。");
  assert.deepStrictEqual(env.model.dump(), before, "素の References も文献リストも残る");
  dialog(env).send({ type: "close" });
  await settled(env, ev);
});

test("Move bibliography here: 本文 2 段落にまたがる選択なら最後の段落の直後へ（誤検知しない）", async () => {
  const env = makeEnv({ paras: ["Intro text.", "Discussion.", "Conclusion."] });
  setCursor(env, 0, 10);
  const tag = await addCitation(env, ["K1"]);
  env.model.sel = { para: env.model.paras[0], start: 2, endPara: env.model.paras[1], end: 4 };
  const ev = newEvent("moveBibliography");
  env.cmds.moveBibliography(ev);
  await settled(env, ev);
  const doc = env.model.dump();
  assert.deepStrictEqual(doc.slice(0, 3), ["Intro text{" + tag + ":[1]}.", "Discussion.", "[refract-bib-heading] References"]);
  assert.match(doc[3], /^\[refract-bib\] 1\. Stoffel/);
  assert.strictEqual(doc[4], "Conclusion.");
  assert.strictEqual(doc.length, 5);
});

test("直前の引用の境界で Add/Edit: 仮置きが前の引用の中に入ったら作り直して引用の直後へ（前の引用は無傷）", async () => {
  const env = makeEnv();
  setCursor(env, 0, 31);
  const tag = await addCitation(env, ["K1"]);
  // カーソルは引用の直後（selectAfter）。Word ではこの位置で作った CC が前の引用の中に入ることがある → sticky で再現
  const prev = ccByTag(env, tag);
  env.model.sel = Object.assign({}, env.model.sel, { sticky: prev });
  const ev = newEvent("addEditCitation");
  env.cmds.addEditCitation(ev);
  await flush();
  const dlg = dialog(env, "cite-dialog.html");
  assert.strictEqual(dlg.d.mode, "add");
  assert.match(
    dlg.docAtOpen[0],
    new RegExp("^Pancreatic cancer is increasing\\{" + tag + ":\\[1\\]\\}\\{refract-pending-[a-z0-9]{8}:\\[…\\]\\}\\.$"),
    "仮置きは前の引用の外（直後）"
  );
  assert.strictEqual(env.model.ccParent(env.model.byTag("refract-pending-")[0]), null);
  dlg.send({ type: "insert", keys: ["K2"] });
  await settled(env, ev);
  assert.match(env.model.dump()[0], new RegExp("^Pancreatic cancer is increasing\\{" + tag + ":\\[1\\]\\}\\{refract-cite-[a-z0-9]{8}:\\[2\\]\\}\\.$"));
});

test("仮置きに利用者が文字を打っていたら、キャンセルでも文字は残す（[…] を消して CC だけ外す）", async () => {
  const env = makeEnv();
  setCursor(env, 0, 31);
  const ev = newEvent("addEditCitation");
  env.cmds.addEditCitation(ev);
  await flush();
  env.model.byTag("refract-pending-")[0].items.push({ t: "text", text: "xyz" }); // ダイアログを開いている間に仮置きの中へ打たれた文字
  dialog(env, "cite-dialog.html").send({ type: "cancel" });
  await settled(env, ev);
  assert.deepStrictEqual(env.model.dump(), ["Pancreatic cancer is increasingxyz.", "Second paragraph."]);
  assert.strictEqual(env.model.allCCs().length, 0);
});

test("Update: 引用 CC はあるが payload が 1 つも無い（別文書から貼り付けた引用だけ）→ 付け直しを促す文言", async () => {
  const p0 = new Para("");
  const cc = new CCNode("inline");
  cc.tag = "refract-cite-pasted01";
  cc.items = [{ t: "text", text: "[3]" }];
  p0.items = [{ t: "text", text: "Pasted " }, { t: "cc", cc: cc }, { t: "text", text: "." }];
  const env = makeEnv({ paras: [p0] });
  const ev = newEvent("updateCitations");
  env.cmds.updateCitations(ev);
  await flush();
  const n = dialog(env, "notice.html");
  assert.deepStrictEqual(n.d, {
    kind: "error",
    message: "引用はありますが OpenRef の引用データがありません（別の文書から貼り付けた引用は Add/Edit で付け直してください）",
  });
  n.send({ type: "close" });
  await settled(env, ev);
  assert.deepStrictEqual(env.model.dump(), ["Pasted {refract-cite-pasted01:[3]}."]);
});

// ---- 仮置きの残骸・入れ子 ----
const NESTED_MSG = "引用の中に別の引用が入っています（入れ子）。中の引用を削除し、Add/Edit で付け直してから Update してください。";
/** 入れ子のエラーは内側の CC を選択してから出す（場所を示す） */
const NESTED_SEL = NESTED_MSG + "（入れ子の引用を選択しました）";
const BOUNDARY_MSG = "直前の引用の境界には挿入できません。引用の後ろに空白などを 1 文字入れてから Add/Edit を押してください。";
/** 選択がちょうど cc の全体（= Word が cc を選択してそこへスクロールした状態） */
function assertSelected(env, cc, label) {
  const b = env.model.bounds({ kind: "cc", cc: cc, loc: "Whole" });
  const s = env.model.sel;
  assert.deepStrictEqual([s.para, s.start, s.end], [b.para, b.start, b.end], (label || "") + ": 選択が内側の引用の上にない");
  assert.strictEqual(env.model.parentCC({ kind: "sel", para: s.para, start: s.start, end: s.end }), cc);
}

/** テスト用: 段落 Claim{outer:[1]{inner:[2]}}. を作る（引用の入れ子） */
function nestedParaAndSettings() {
  const p0 = new Para("");
  const inner = new CCNode("inline");
  inner.tag = "refract-cite-inner001";
  inner.items = [{ t: "text", text: "[2]" }];
  const outer = new CCNode("inline");
  outer.tag = "refract-cite-outer001";
  outer.items = [{ t: "text", text: "[1]" }, { t: "cc", cc: inner }];
  p0.items = [{ t: "text", text: "Claim" }, { t: "cc", cc: outer }, { t: "text", text: "." }];
  const settings = {
    "refract-cite-outer001": { items: [{ key: "K1", csl: LIBRARY.K1 }] },
    "refract-cite-inner001": { items: [{ key: "K2", csl: LIBRARY.K2 }] },
  };
  return { p0, settings };
}

test("文字の打たれた仮置きの残骸の中で Add/Edit → 再利用せず、文字を残して外し直後に新しい仮置き（確定後も文字が残る）", async () => {
  const env = makeEnv({ paras: ["Body text.", "Second paragraph."] });
  const old = await Word_insertPending(env, 0, 4, "refract-pending-leftov02", " my typed words");
  assert.strictEqual(env.model.dump()[0], "Body{refract-pending-leftov02:[…] my typed words} text.");
  cursorInCC(env, "refract-pending-leftov02");
  const ev = newEvent("addEditCitation");
  env.cmds.addEditCitation(ev);
  await flush();
  const dlg = dialog(env, "cite-dialog.html");
  assert.strictEqual(dlg.d.mode, "add");
  assert.match(dlg.docAtOpen[0], /^Body my typed words\{refract-pending-[a-z0-9]{8}:\[…\]\} text\.$/, dlg.docAtOpen[0]);
  assert(!env.model.alive(old), "残骸の CC は外れた（[…] も消えた）");
  dlg.send({ type: "insert", keys: ["K1"] });
  await settled(env, ev);
  assert.match(env.model.dump()[0], /^Body my typed words\{refract-cite-[a-z0-9]{8}:\[1\]\} text\.$/, env.model.dump()[0]);
});

test("作り直し先でも前の引用の中に入る: 仮置きを消して error notice・前の引用は無傷・payload と台帳は増えない", async () => {
  const env = makeEnv();
  setCursor(env, 0, 31);
  const tag = await addCitation(env, ["K1"]);
  env.model.stickyCCs.add(ccByTag(env, tag)); // 末尾境界への挿入は、どの経路でも前の引用の中に入る
  const before = env.model.dump();
  const persisted = clone(env.persisted);
  const ev = newEvent("addEditCitation");
  env.cmds.addEditCitation(ev);
  await flush();
  const n = dialog(env, "notice.html");
  assert.deepStrictEqual(n.d, { kind: "error", message: BOUNDARY_MSG });
  assert.deepStrictEqual(env.model.dump(), before, "文書は元のまま（前の引用も無傷）");
  assert.strictEqual(env.model.byTag("refract-pending-").length, 0);
  assert.deepStrictEqual(env.persisted, persisted, "payload・台帳は増えない");
  n.send({ type: "close" });
  await settled(env, ev);
});

test("Update: 引用の中に引用（Claim{outer:[1]{inner:[2]}}）→ 何も変えずにエラー", async () => {
  const { p0, settings } = nestedParaAndSettings();
  const env = makeEnv({ paras: [p0, "Next."], settings: settings });
  assert.strictEqual(env.model.dump()[0], "Claim{refract-cite-outer001:[1]{refract-cite-inner001:[2]}}.");
  const before = env.model.dump();
  const ev = newEvent("updateCitations");
  env.cmds.updateCitations(ev);
  await flush();
  const n = dialog(env, "notice.html");
  assert.deepStrictEqual(n.d, { kind: "error", message: NESTED_SEL });
  assert.deepStrictEqual(env.model.dump(), before);
  assertSelected(env, ccByTag(env, "refract-cite-inner001"), "Update");
  n.send({ type: "close" });
  await settled(env, ev);
});

test("Update: 文献リスト CC の中に引用 CC → 何も変えずにエラー", async () => {
  const p0 = new Para("");
  const cite = new CCNode("inline");
  cite.tag = "refract-cite-body0001";
  cite.items = [{ t: "text", text: "[1]" }];
  p0.items = [{ t: "text", text: "Claim" }, { t: "cc", cc: cite }, { t: "text", text: "." }];
  const bibP = new Para("");
  const bib = new CCNode("block");
  bib.tag = "refract-bib";
  bibP.block = bib;
  const stray = new CCNode("inline");
  stray.tag = "refract-cite-inbib001";
  stray.items = [{ t: "text", text: "[9]" }];
  bibP.items = [{ t: "text", text: "1. Old entry " }, { t: "cc", cc: stray }];
  const env = makeEnv({
    paras: [p0, bibP],
    settings: {
      "refract-cite-body0001": { items: [{ key: "K1", csl: LIBRARY.K1 }] },
      "refract-cite-inbib001": { items: [{ key: "K2", csl: LIBRARY.K2 }] },
    },
  });
  const before = env.model.dump();
  assert.strictEqual(before[1], "[refract-bib] 1. Old entry {refract-cite-inbib001:[9]}");
  const ev = newEvent("updateCitations");
  env.cmds.updateCitations(ev);
  await flush();
  assert.deepStrictEqual(dialog(env, "notice.html").d, { kind: "error", message: NESTED_SEL });
  assert.deepStrictEqual(env.model.dump(), before);
  assertSelected(env, stray, "文献リストの中の引用");
  dialog(env).send({ type: "close" });
  await settled(env, ev);
});

test("Add/Edit の確定時に入れ子が見つかったら: 何も書かずにエラー（仮置きは消え、payload の無い引用も引用の無い payload も残らない）", async () => {
  const { p0, settings } = nestedParaAndSettings();
  const env = makeEnv({ paras: [p0, "Other text."], settings: settings });
  const before = env.model.dump();
  const persisted = clone(env.persisted);
  setCursor(env, 1, 5);
  const ev = newEvent("addEditCitation");
  env.cmds.addEditCitation(ev);
  await flush();
  dialog(env, "cite-dialog.html").send({ type: "insert", keys: ["K3"] });
  await env.clock.advance(1000);
  const n = dialog(env, "notice.html");
  assert.deepStrictEqual(n.d, { kind: "error", message: NESTED_SEL });
  assert.deepStrictEqual(env.model.dump(), before, "仮置きも消えて文書は元どおり");
  assert.deepStrictEqual(env.persisted, persisted, "payload・台帳・スタイル固定のどれも書いていない");
  assertSelected(env, ccByTag(env, "refract-cite-inner001"), "確定");
  n.send({ type: "close" });
  await settled(env, ev);
});

// ---- 入れ子・仮置き・Move bibliography の追加ケース ----
test("Move bibliography here: 入れ子のある文書 → 何も変えずにエラー（作り直した文献リストが空のまま残らない）・内側の引用を選択", async () => {
  // 入れ子の型: Claim{outer{inner}}. / Body. / [見出し] / [文献リスト]
  const { p0, settings } = nestedParaAndSettings();
  const head = new CCNode("block");
  head.tag = "refract-bib-heading";
  const headP = new Para("References");
  headP.block = head;
  const bib = new CCNode("block");
  bib.tag = "refract-bib";
  const bibP = new Para("1. Stoffel EM. Pancreatic cancer.");
  bibP.block = bib;
  const env = makeEnv({ paras: [p0, "Body.", headP, bibP], settings: settings });
  const before = env.model.dump();
  setCursor(env, 1, 2);
  const ev = newEvent("moveBibliography");
  env.cmds.moveBibliography(ev);
  await flush();
  const n = dialog(env, "notice.html");
  assert.deepStrictEqual(n.d, { kind: "error", message: NESTED_SEL });
  assert.deepStrictEqual(env.model.dump(), before, "見出し・文献リストとも元の位置・元の中身のまま");
  assertSelected(env, ccByTag(env, "refract-cite-inner001"), "Move");
  n.send({ type: "close" });
  await settled(env, ev);
});

test("Settings の適用: 入れ子のある文書 → 保存もせずにエラー（設定も文書も変わらない）・内側の引用を選択", async () => {
  const { p0, settings } = nestedParaAndSettings();
  settings["refract-style"] = "vancouver";
  settings["refract-doi-always"] = false;
  const env = makeEnv({ paras: [p0, "Next."], settings: settings });
  const before = env.model.dump();
  const persisted = clone(env.persisted);
  const saves = env.stats.saves;
  const ev = newEvent("citationStyle");
  env.cmds.citationStyle(ev);
  await flush();
  dialog(env, "settings-dialog.html").send({ type: "apply", style: "american-medical-association", doiAlways: true, fontName: "Times New Roman", fontSize: 12 });
  await env.clock.advance(1000);
  const n = dialog(env, "notice.html");
  assert.deepStrictEqual(n.d, { kind: "error", message: NESTED_SEL });
  assert.deepStrictEqual(env.persisted, persisted, "スタイル・DOI・フォントのどれも保存していない");
  assert.strictEqual(env.stats.saves, saves);
  assert.deepStrictEqual(env.model.dump(), before);
  assertSelected(env, ccByTag(env, "refract-cite-inner001"), "Settings");
  n.send({ type: "close" });
  await settled(env, ev);
});

test("確定する仮置き自身が OpenRef の CC の中（ほかの引用は入れ子でない）→ NESTED のエラー・何も残さない", async () => {
  const env = makeEnv({ paras: ["First claim.", "Other text."] });
  setCursor(env, 0, 11);
  const tag = await addCitation(env, ["K1"]);
  const before = env.model.dump();
  const persisted = clone(env.persisted);
  setCursor(env, 1, 5);
  const ev = newEvent("addEditCitation");
  env.cmds.addEditCitation(ev);
  await flush();
  const dlg = dialog(env, "cite-dialog.html");
  // ダイアログを開いている間に仮置きが既存の引用の中へ入った（ほかの引用同士は入れ子でない）
  const pending = env.model.byTag("refract-pending-")[0];
  const f = env.model.findInline(pending);
  f.container.splice(f.container.indexOf(f.item), 1);
  ccByTag(env, tag).items.push(f.item);
  assert.match(env.model.dump()[0], new RegExp("^First claim\\{" + tag + ":\\[1\\]\\{refract-pending-[a-z0-9]{8}:\\[…\\]\\}\\}\\.$"));
  dlg.send({ type: "insert", keys: ["K2"] });
  await env.clock.advance(1000);
  const n = dialog(env, "notice.html");
  assert.deepStrictEqual(n.d, { kind: "error", message: NESTED_SEL });
  assert.deepStrictEqual(env.persisted, persisted, "payload・台帳を書いていない（引用の無い payload を残さない）");
  assert.deepStrictEqual(env.model.dump(), before, "仮置きも消え、payload の無い引用も残らない");
  for (const c of citeCCs(env)) assert(env.persisted[c.tag] && env.persisted[c.tag].items.length, "payload の無い引用: " + c.tag);
  n.send({ type: "close" });
  await settled(env, ev);
});

test("Remove Citation Codes: 仮置きに打った文字は […] を消して残す（Second TYPED paragraph.）", async () => {
  const env = makeEnv({ paras: ["First.", "Second paragraph."] });
  setCursor(env, 0, 5);
  await addCitation(env, ["K1"]);
  await Word_insertPending(env, 1, 7, "refract-pending-typed002", "TYPED ");
  assert.strictEqual(env.model.dump()[1], "Second {refract-pending-typed002:[…]TYPED }paragraph.");
  const ev = newEvent("removeCitationCodes");
  env.cmds.removeCitationCodes(ev);
  await flush();
  dialog(env, "notice.html").send({ type: "ok" });
  await settled(env, ev);
  assert.strictEqual(env.model.dump()[1], "Second TYPED paragraph.");
  assert.strictEqual(env.model.allCCs().length, 0);
});

test("search が使えない環境: CC は外れて打った文字は残る（[…] も残る = 従来どおり delete(true) だけ）", async () => {
  const env = makeEnv();
  setCursor(env, 0, 31);
  const ev = newEvent("addEditCitation");
  env.cmds.addEditCitation(ev);
  await flush();
  env.model.byTag("refract-pending-")[0].items.push({ t: "text", text: "xyz" });
  env.searchFails = true;
  dialog(env, "cite-dialog.html").send({ type: "cancel" });
  await settled(env, ev);
  assert.deepStrictEqual(env.model.dump(), ["Pancreatic cancer is increasing[…]xyz.", "Second paragraph."]);
  assert.strictEqual(env.model.allCCs().length, 0);
  assert(env.logs.some((l) => l[0] === "warn" && /探せませんでした/.test(l[1])));
});

test("Move bibliography here: 新しいリストの作成で失敗しても旧いリストは残る（作ってから消す順）", async () => {
  const env = makeEnv({ paras: ["Intro text.", "Discussion.", "Conclusion."] });
  setCursor(env, 0, 10);
  await addCitation(env, ["K1"]);
  const before = env.model.dump();
  setCursor(env, 1, 3);
  env.model.fail.add("paraInsertParagraph");
  const ev = newEvent("moveBibliography");
  env.cmds.moveBibliography(ev);
  await flush();
  const n = dialog(env, "notice.html");
  assert.strictEqual(n.d.kind, "error");
  assert.match(n.d.message, /注入した失敗/);
  assert.deepStrictEqual(env.model.dump(), before, "旧い見出し・文献リストは残る");
  n.send({ type: "close" });
  await settled(env, ev);
});

test("Move bibliography here: 選択が文献リストの途中で終わる（OverlapsBefore）/ 途中から始まる（OverlapsAfter）だけでもエラー", async () => {
  // 見出し CC も素の References も無い文書（重なりが Contains にならない形）: [P0 引用, P1 本文, bib1, bib2, P4 本文]
  const mk = () => {
    const p0 = new Para("");
    const cite = new CCNode("inline");
    cite.tag = "refract-cite-overlap1";
    cite.items = [{ t: "text", text: "(1)" }];
    p0.items = [{ t: "text", text: "Claim" }, { t: "cc", cc: cite }, { t: "text", text: "." }];
    const bib = new CCNode("block");
    bib.tag = "refract-bib";
    const b1 = new Para("1. First entry");
    const b2 = new Para("2. Second entry");
    b1.block = bib;
    b2.block = bib;
    const p1 = new Para("Discussion.");
    const p4 = new Para("Appendix.");
    const env = makeEnv({
      paras: [p0, p1, b1, b2, p4],
      settings: { "refract-cite-overlap1": { items: [{ key: "K1", csl: LIBRARY.K1 }] } },
    });
    return { env, bib, p1, b1, b2, p4 };
  };
  const cases = [
    ["OverlapsBefore", (x) => ({ para: x.p1, start: 2, endPara: x.b1, end: 3 }), (x) => ({ kind: "span", para: x.p1, start: 0, endPara: x.b1, end: 14 })],
    ["OverlapsAfter", (x) => ({ para: x.b2, start: 2, endPara: x.p4, end: 3 }), (x) => ({ kind: "span", para: x.b2, start: 0, endPara: x.p4, end: 9 })],
  ];
  for (const [rel, sel, span] of cases) {
    const x = mk();
    // この選択の段落範囲と文献リストの関係が本当に rel であること（= ほかの関係で止まっているのではない）
    assert.strictEqual(x.env.model.compare(span(x), { kind: "cc", cc: x.bib, loc: "Whole" }), rel);
    const before = x.env.model.dump();
    x.env.model.sel = sel(x);
    const ev = newEvent("moveBibliography");
    x.env.cmds.moveBibliography(ev);
    await flush();
    const n = dialog(x.env, "notice.html");
    assert.strictEqual(n.d.message, "本文の段落にカーソルを置いてください。", rel);
    assert.deepStrictEqual(x.env.model.dump(), before, rel + ": 文書は不変");
    n.send({ type: "close" });
    await settled(x.env, ev);
  }
});

test("確定時に対象 CC の tag が変わっていた（同じ id・別の tag）: replace も delete も error・文書と設定は不変", async () => {
  for (const msg of [{ type: "replace", keys: ["K2"] }, { type: "delete" }]) {
    const env = makeEnv();
    setCursor(env, 0, 31);
    const tag = await addCitation(env, ["K1"]);
    cursorInCC(env, tag);
    const ev = newEvent("addEditCitation");
    env.cmds.addEditCitation(ev);
    await flush();
    const dlg = dialog(env, "cite-dialog.html");
    ccByTag(env, tag).tag = "refract-cite-retagged"; // ダイアログを開いている間に同じ CC が別の tag になった
    const before = env.model.dump();
    const persisted = clone(env.persisted);
    dlg.send(msg);
    await env.clock.advance(1000);
    const n = dialog(env, "notice.html");
    assert.strictEqual(n.d.message, "編集中の引用が文書から見つかりません。", msg.type);
    assert.deepStrictEqual(env.model.dump(), before, msg.type + ": 文書は不変");
    assert.deepStrictEqual(env.persisted, persisted, msg.type + ": 設定は不変");
    n.send({ type: "close" });
    await settled(env, ev);
  }
});

test("strict mock の自己検査: load 前の参照・別 context の proxy は失敗する", async () => {
  const env = makeEnv();
  const W = env.sb.Word;
  await assert.rejects(
    W.run(async (ctx) => {
      const ccs = ctx.document.contentControls;
      ccs.load("items/tag");
      return ccs.items; // sync 前
    }),
    /PropertyNotLoaded|load\(\) \+ sync\(\) の前/
  );
  await assert.rejects(
    W.run(async (ctx) => {
      const p = ctx.document.getSelection().parentContentControlOrNullObject;
      p.load("title");
      await ctx.sync();
      return p.tag; // load していない
    }),
    /load していない/
  );
  let leaked;
  await W.run(async (ctx) => {
    leaked = ctx.document.body.paragraphs.getFirst();
    await ctx.sync();
  });
  await assert.rejects(
    W.run(async () => {
      leaked.insertParagraph("x", "After");
    }),
    /終わった Word\.run/
  );
});

/** テスト用: 段落 pi の off に仮置き CC（tag 指定）を直接作る（前回の残骸の再現） */
async function Word_insertPending(env, pi, off, tag, typed) {
  const seg = env.model.insertTextAt(env.model.paras[pi], off, "[…]");
  const cc = env.model.rangeWrapCC(seg);
  cc.tag = tag;
  cc.title = "OpenRef citation (pending)";
  if (typed) cc.items.push({ t: "text", text: typed }); // 利用者が仮置きの中に打った文字
  return cc;
}
/** テスト用: 引用 CC を同じ tag・同じ表示のまま段落 pi の off へ複製する（Word のコピー＆ペースト） */
function pasteCopy(env, tag, pi, off) {
  const src = env.model.allCCs().find((c) => c.tag === tag);
  const cc = new CCNode("inline");
  cc.tag = src.tag;
  cc.title = src.title;
  cc.items = [{ t: "text", text: env.model.ccText(src) }];
  env.model.insertAt(env.model.paras[pi], off, { t: "cc", cc: cc });
  return cc;
}
/** テスト用: CC の中（先頭 + 1 文字）にカーソル。同じ tag が複数あれば n 番目 */
function cursorInNth(env, tag, n) {
  const cc = env.model.allCCs().filter((c) => c.tag === tag)[n];
  const f = env.model.findInline(cc);
  env.model.sel = { para: f.para, start: f.start + 1, end: f.start + 1 };
  return cc;
}

// ======================================================================
(async () => {
  let fail = 0;
  for (const t of tests) {
    try {
      await t.fn();
      console.log("OK   " + t.name);
    } catch (e) {
      fail++;
      console.log("FAIL " + t.name + "\n     " + (e && e.stack ? e.stack.split("\n").slice(0, 4).join("\n     ") : e));
    }
  }
  console.log(fail ? "\n" + fail + " / " + tests.length + " FAILED" : "\n全 " + tests.length + " 件 OK");
  process.exit(fail ? 1 : 0);
})();
