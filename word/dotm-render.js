/* OpenRef for Word（Startup テンプレート版 OpenRef.dotm）— 整形の段取りと「citeproc の HTML → 本文 + 書式 run」の変換。
   connectd が JavaScriptCore で vendor/citeproc.js・cite-core.js の後に評価する
   （ブラウザ・Office には読み込まない）。整形の規則は Office JS 版 doc.js の updateAll と同じ:
   番号 = 文書順の初出順・書誌は蔵書（fresh）の最新を優先し、蔵書に無い key は payload の snapshot。
   run は UTF-16 単位（JS の文字列長そのまま）の `<開始>:<長さ>:<書式>` をカンマ区切り。書式の無い部分は run を作らない。
   node でテストできる（word/test-dotm-render.cjs）。 */
(function (root) {
  "use strict";

  /** run の書式文字（この順で並べる）: i 斜体・b 太字・u 下線・s 上付き・d 下付き・c small-caps */
  var FMT = "ibusdc";
  /** 番号付きスタイルの番号（csl-left-margin）と本文（csl-right-inline）の間。番号の側が空白で終わっていなければ 1 つ入れる
      （Vancouver の "1." + "Stoffel" が "1.Stoffel" と詰まらないように。Radiology は "1. " で終わるので足さない） */
  var MARGIN_SEP = " ";
  var BLOCK = { div: 1, p: 1, li: 1, ul: 1, ol: 1, blockquote: 1, table: 1, tr: 1, td: 1, th: 1, h1: 1, h2: 1, h3: 1, h4: 1, h5: 1, h6: 1 };
  var VOID = { br: 1, img: 1, hr: 1, wbr: 1, meta: 1, link: 1, input: 1 };
  var TAG_FMT = {
    i: { i: true },
    em: { i: true },
    cite: { i: true },
    b: { b: true },
    strong: { b: true },
    u: { u: true },
    ins: { u: true },
    sup: { s: true, d: false },
    sub: { d: true, s: false },
  };
  var NAMED = { amp: "&", lt: "<", gt: ">", quot: '"', apos: "'", nbsp: " " };

  // ---------- 字句 ----------
  /** タグの終わり（引用符の中の > は飛ばす）。見つからなければ -1 */
  function tagEnd(html, from) {
    var q = null;
    for (var i = from; i < html.length; i++) {
      var ch = html.charAt(i);
      if (q) {
        if (ch === q) q = null;
      } else if (ch === '"' || ch === "'") {
        q = ch;
      } else if (ch === ">") {
        return i;
      }
    }
    return -1;
  }

  function attrs(raw) {
    var out = {};
    var re = /([A-Za-z_:][-A-Za-z0-9_:.]*)\s*(?:=\s*("[^"]*"|'[^']*'|[^\s"'>]+))?/g;
    var m;
    while ((m = re.exec(raw))) {
      var v = m[2] === undefined ? "" : m[2];
      if (v.charAt(0) === '"' || v.charAt(0) === "'") v = v.slice(1, -1);
      out[m[1].toLowerCase()] = decode(v);
    }
    return out;
  }

  /** HTML 断片 → [{t:"text", v} | {t:"open"|"close", name, attrs}]。タグでない "<" は文字として残す */
  function tokenize(html) {
    var out = [];
    var i = 0;
    var n = html.length;
    function text(s) {
      if (s) out.push({ t: "text", v: s });
    }
    while (i < n) {
      var lt = html.indexOf("<", i);
      if (lt < 0) {
        text(html.slice(i));
        break;
      }
      text(html.slice(i, lt));
      if (html.slice(lt, lt + 4) === "<!--") {
        var ce = html.indexOf("-->", lt + 4);
        i = ce < 0 ? n : ce + 3;
        continue;
      }
      var m = /^<(\/?)([A-Za-z][A-Za-z0-9:-]*)/.exec(html.slice(lt, lt + 64));
      var gt = m ? tagEnd(html, lt + 1) : -1;
      if (!m || gt < 0) {
        text("<");
        i = lt + 1;
        continue;
      }
      var name = m[2].toLowerCase();
      var inner = html.slice(lt + m[0].length, gt);
      var selfClose = /\/\s*$/.test(inner);
      if (m[1]) out.push({ t: "close", name: name });
      else {
        out.push({ t: "open", name: name, attrs: attrs(inner.replace(/\/\s*$/, "")) });
        if (selfClose || VOID[name]) out.push({ t: "close", name: name, implied: true });
      }
      i = gt + 1;
    }
    return out;
  }

  /** 文字参照（数値 + 基本の名前付き）。未知の名前はそのまま */
  function decode(s) {
    return s.replace(/&(#[0-9]+|#[xX][0-9A-Fa-f]+|[A-Za-z]+);/g, function (all, e) {
      if (e.charAt(0) === "#") {
        var cp = e.charAt(1) === "x" || e.charAt(1) === "X" ? parseInt(e.slice(2), 16) : parseInt(e.slice(1), 10);
        if (!isFinite(cp) || cp < 0 || cp > 0x10ffff || (cp >= 0xd800 && cp <= 0xdfff)) return all;
        return String.fromCodePoint(cp);
      }
      return Object.prototype.hasOwnProperty.call(NAMED, e) ? NAMED[e] : all;
    });
  }

  /** style 属性 → 書式の上書き（citeproc の打ち消し font-style:normal 等は false） */
  function cssDelta(style) {
    var d = {};
    String(style || "")
      .split(";")
      .forEach(function (decl) {
        var kv = decl.split(":");
        var k = kv[0].trim().toLowerCase();
        var v = kv.slice(1).join(":").trim().toLowerCase();
        if (kv.length === 1) {
          // citeproc の vertical-align:baseline は style="baseline"（属性名なし）で出る
          if (k === "baseline") v = "baseline";
          else return;
          k = "vertical-align";
        }
        if (k === "font-style") {
          if (v === "italic" || v === "oblique") d.i = true;
          else if (v === "normal") d.i = false;
        } else if (k === "font-weight") {
          if (v === "bold" || v === "bolder" || /^[6-9]00$/.test(v)) d.b = true;
          else if (v === "normal" || v === "lighter" || v === "light" || /^[1-5]00$/.test(v)) d.b = false;
        } else if (k === "font-variant") {
          if (v.indexOf("small-caps") >= 0) d.c = true;
          else if (v === "normal") d.c = false;
        } else if (k === "text-decoration" || k === "text-decoration-line") {
          if (v.indexOf("underline") >= 0) d.u = true;
          else if (v === "none") d.u = false;
        } else if (k === "vertical-align") {
          if (v === "super" || v === "sup") {
            d.s = true;
            d.d = false;
          } else if (v === "sub") {
            d.d = true;
            d.s = false;
          } else if (v === "baseline") {
            d.s = false;
            d.d = false;
          }
        }
      });
    return d;
  }

  function classes(a) {
    return " " + String((a && a["class"]) || "").toLowerCase().replace(/\s+/g, " ") + " ";
  }

  // ---------- HTML → 段落・run ----------
  /** citeproc の HTML（引用クラスタ or 文献リスト）を本文 + run にする。
      block = true（文献リスト）: csl-entry（と p）ごとに 1 段落・段落は "\n" 区切り・段落の前後の空白は落とす。
      block = false（引用）: 1 行（div が来たら空白 1 つで続ける）。
      citeproc が整形のために入れる改行 + 字下げ（div の前後の空白だけの文字列）は捨てる。本文中の空白類（改行・タブ）は
      HTML と同じく空白 1 つに畳み、制御文字は落とす（"\n" は段落区切り専用）。
      戻り値 {text, runs: "<開始>:<長さ>:<書式>,…"}（UTF-16 単位） */
  function htmlToRuns(html, block) {
    var toks = tokenize(String(html || ""));
    var stack = []; // {name, delta, cls}
    var paras = [];
    var cur = null; // 今の段落 = [{text, fmt}]
    var soft = false; // 次の文字の前に空白 1 つ（csl-block / csl-indent / br の境目）
    var margin = false; // 直前に csl-left-margin が閉じた

    function fmtNow() {
      var st = {};
      for (var i = 0; i < stack.length; i++) {
        var d = stack[i].delta;
        for (var k in d) if (Object.prototype.hasOwnProperty.call(d, k)) st[k] = d[k];
      }
      var f = "";
      for (var j = 0; j < FMT.length; j++) if (st[FMT.charAt(j)]) f += FMT.charAt(j);
      return f;
    }
    function lastChar() {
      for (var i = cur ? cur.length - 1 : -1; i >= 0; i--) {
        if (cur[i].text) return cur[i].text.charAt(cur[i].text.length - 1);
      }
      return "";
    }
    function isSpace(ch) {
      return ch === " " || ch === " " || ch === "\t";
    }
    function push(text, fmt) {
      if (!text) return;
      if (!cur) cur = [];
      var last = cur[cur.length - 1];
      if (last && last.fmt === fmt) last.text += text;
      else cur.push({ text: text, fmt: fmt });
    }
    function endPara() {
      if (cur && cur.length) paras.push(cur);
      cur = null;
      soft = false;
      margin = false;
    }
    function isBlockTok(tk) {
      return tk && (tk.t === "open" || tk.t === "close") && BLOCK[tk.name];
    }

    for (var i = 0; i < toks.length; i++) {
      var tk = toks[i];
      if (tk.t === "text") {
        var raw = decode(tk.v);
        // 整形用の空白（div の前後・断片の端の空白だけの文字列）は捨てる
        if (/^[ \t\n\r\f]*$/.test(raw) && (i === 0 || i === toks.length - 1 || isBlockTok(toks[i - 1]) || isBlockTok(toks[i + 1]))) continue;
        var text = raw.replace(/[\u0000-\u0008\u000b\u000e-\u001f\u007f]/g, "").replace(/[ \t\n\r\f]+/g, " ");
        if (!text) continue;
        var f = fmtNow();
        if (cur && cur.length && !isSpace(lastChar()) && !isSpace(text.charAt(0))) {
          if (margin) push(MARGIN_SEP, "");
          else if (soft) push(" ", "");
        }
        soft = false;
        margin = false;
        push(text, f);
        continue;
      }
      var cls = tk.t === "open" ? classes(tk.attrs) : "";
      if (tk.t === "open") {
        if (tk.name === "br") {
          soft = true;
          continue;
        }
        var delta = {};
        var base = TAG_FMT[tk.name];
        if (base) for (var k in base) delta[k] = base[k];
        if (tk.attrs && tk.attrs.style) {
          var css = cssDelta(tk.attrs.style);
          for (var k2 in css) delta[k2] = css[k2];
        }
        stack.push({ name: tk.name, delta: delta, cls: cls });
        if (BLOCK[tk.name]) {
          if (block && (cls.indexOf(" csl-entry ") >= 0 || tk.name === "p")) endPara();
          else if (cls.indexOf(" csl-right-inline ") < 0 && cls.indexOf(" csl-left-margin ") < 0 && cls.indexOf(" csl-bib-body ") < 0) soft = true;
        }
      } else {
        if (tk.implied && VOID[tk.name]) continue;
        // 対応する開きタグまで閉じる（無ければ無視）
        var at = -1;
        for (var s = stack.length - 1; s >= 0; s--) {
          if (stack[s].name === tk.name) {
            at = s;
            break;
          }
        }
        if (at < 0) continue;
        var closed = stack[at];
        stack.length = at;
        if (BLOCK[tk.name]) {
          if (block && (closed.cls.indexOf(" csl-entry ") >= 0 || tk.name === "p")) endPara();
          else if (closed.cls.indexOf(" csl-left-margin ") >= 0) margin = true;
          else if (closed.cls.indexOf(" csl-right-inline ") < 0 && closed.cls.indexOf(" csl-bib-body ") < 0) soft = true;
        }
      }
    }
    endPara();

    // 段落を連結して run を作る（文献リストは段落の前後の空白を落とす）
    var text = "";
    var runs = [];
    paras.forEach(function (p, pi) {
      if (block) {
        while (p.length && /^[ \t]/.test(p[0].text)) {
          p[0].text = p[0].text.replace(/^[ \t]+/, "");
          if (!p[0].text) p.shift();
        }
        while (p.length && /[ \t]$/.test(p[p.length - 1].text)) {
          var lastSeg = p[p.length - 1];
          lastSeg.text = lastSeg.text.replace(/[ \t]+$/, "");
          if (!lastSeg.text) p.pop();
        }
        if (!p.length) return;
      }
      if (text) text += block ? "\n" : " ";
      p.forEach(function (seg) {
        if (seg.fmt) {
          var prev = runs[runs.length - 1];
          if (prev && prev.fmt === seg.fmt && prev.start + prev.len === text.length) prev.len += seg.text.length;
          else runs.push({ start: text.length, len: seg.text.length, fmt: seg.fmt });
        }
        text += seg.text;
      });
    });
    return {
      text: text,
      runs: runs
        .map(function (r) {
          return r.start + ":" + r.len + ":" + r.fmt;
        })
        .join(","),
    };
  }

  // ---------- 整形の段取り（doc.js updateAll と同じ規則） ----------
  /** input = {styleXml, locales: {lang: xml}, doiAlways, cites: [payload…]（文書順の引用 CC ごと = {items:[{key, csl}]}）,
      fresh: {key: csl}（蔵書にあった分）}。
      戻り値 {cites: [{text, runs}]（cites と同順）, bib: {text, runs}, items: 文献数, warnings: [citeproc の警告]} */
  function render(input) {
    var cites = input.cites || [];
    var fresh = input.fresh || {};
    // 文書順の初出順で番号決定
    var ordered = [];
    cites.forEach(function (p) {
      p.items.forEach(function (it) {
        if (ordered.indexOf(it.key) < 0) ordered.push(it.key);
      });
    });
    var itemsById = {};
    cites.forEach(function (p) {
      p.items.forEach(function (it) {
        itemsById[it.key] = fresh[it.key] || it.csl;
      });
    });
    var core = new root.OpenRefCite.CiteCore(root.CSL, input.styleXml, input.locales);
    core.setItems(itemsById);
    core.register(ordered);
    var out = { cites: [], bib: null, items: ordered.length, warnings: warnings.slice() };
    cites.forEach(function (p) {
      var html = core.clusterHtml(
        p.items.map(function (it) {
          return it.key;
        })
      );
      out.cites.push(htmlToRuns(html, false));
    });
    out.bib = htmlToRuns(core.bibliographyHtml({ doiAlways: !!input.doiAlways }) || "<p>(no items)</p>", true);
    out.warnings = warnings.slice();
    return out;
  }

  /** スタイルが要求する locale（cite-core の requestedLocales。取り揃えてから render を呼ぶ） */
  function locales(styleXml, enUSXml) {
    return root.OpenRefCite.requestedLocales(root.CSL, styleXml, enUSXml);
  }

  // citeproc の警告（CSL.debug → console.log）。JavaScriptCore の素の context には console が無いので受け皿を置く
  var warnings = [];
  if (typeof root.console === "undefined") {
    var keep = function () {
      if (warnings.length < 50) warnings.push(Array.prototype.join.call(arguments, " "));
    };
    root.console = { log: keep, warn: keep, error: keep, info: keep, debug: keep };
  }

  root.OpenRefDotm = {
    render: render,
    locales: locales,
    htmlToRuns: htmlToRuns,
    FMT: FMT,
    MARGIN_SEP: MARGIN_SEP,
  };
})(typeof self !== "undefined" ? self : globalThis);
