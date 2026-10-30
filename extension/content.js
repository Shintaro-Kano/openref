/* OpenRef Capture — content script（popup から都度注入・activeTab）。
   役割は 2 つだけ: ①識別子（DOI/PMID）と PDF URL の抽出 ②ページ内 fetch での PDF 取得。
   書誌の組み立ては一切しない（PubMed/Crossref が正 = translator 不要の設計）。 */
(function () {
  "use strict";
  if (window.__openRefCaptureLoaded) return;
  window.__openRefCaptureLoaded = true;

  function meta(name) {
    var el = document.querySelector(
      'meta[name="' + name + '" i], meta[property="' + name + '" i]'
    );
    return el ? el.content : null;
  }

  function cleanDoi(s) {
    if (!s) return null;
    s = s.trim().replace(/^doi:\s*/i, "").replace(/^https?:\/\/(dx\.)?doi\.org\//i, "");
    s = s.replace(/[).,;\]]+$/, "");
    return /^10\.\d{4,9}\//.test(s) ? s : null;
  }

  function analyze() {
    var doi =
      cleanDoi(meta("citation_doi")) ||
      cleanDoi(meta("dc.identifier")) ||
      cleanDoi(meta("prism.doi")) ||
      cleanDoi(meta("bepress_citation_doi"));
    var pmid = (meta("citation_pmid") || "").trim() || null;

    // URL パターン: PubMed
    if (!pmid) {
      var mp = location.href.match(/pubmed\.ncbi\.nlm\.nih\.gov\/(\d{4,9})/);
      if (mp) pmid = mp[1];
    }
    // URL パターン: Atypon/Silverchair 系 /doi/(full|abs|pdf|epdf)/10.x
    if (!doi) {
      var md = location.pathname.match(
        /\/doi\/(?:full\/|abs\/|pdf\/|epdf\/|pdfdirect\/)?(10\.\d{4,9}\/[^\s?#]+)/
      );
      if (md) doi = cleanDoi(decodeURIComponent(md[1]));
    }
    // 医書.jp: PDF 自体に DOI の印字がなくても出版社 URL に識別子がある。
    if (!doi && location.hostname === "webview.isho.jp") {
      var isho = location.pathname.match(/^\/journal\/detail\/(?:pdf|abs)\/(10\.\d{4,9}\/.+)$/);
      if (isho) doi = cleanDoi(decodeURIComponent(isho[1]));
    }
    // doi.org リンク
    if (!doi) {
      var a = document.querySelector('a[href*="doi.org/10."]');
      if (a) {
        var ma = a.href.match(/doi\.org\/(10\.[^\s?#]+)/);
        if (ma) doi = cleanDoi(decodeURIComponent(ma[1]));
      }
    }
    // 最終手段: 本文冒頭から DOI
    if (!doi && !pmid && document.body) {
      var mt = document.body.innerText
        .slice(0, 30000)
        .match(/\b(10\.\d{4,9}\/[-._;()/:A-Za-z0-9]+)/);
      if (mt) doi = cleanDoi(mt[1]);
    }

    var isPdf = document.contentType === "application/pdf";
    // pdfSource = PDF URL の出どころ（popup が「本文から識別子を取る経路」に回すかの判断に使う:
    // 識別子の無いページでは meta / self 由来だけを論文本体とみなし、ページ上のリンク拾いは信用しない）
    var pdfUrl = meta("citation_pdf_url");
    var pdfSource = pdfUrl ? "meta" : null;
    if (!pdfUrl && isPdf) {
      pdfUrl = location.href;
      pdfSource = "self";
    }
    // Atypon 系: /doi/10.x → /doi/pdf/10.x
    if (!pdfUrl) {
      var mA = location.pathname.match(/^\/doi\/(?:full\/|abs\/|epdf\/)?(10\..+)$/);
      if (mA) {
        pdfUrl = location.origin + "/doi/pdf/" + mA[1] + "?download=true";
        pdfSource = "atypon";
      }
    }
    // ScienceDirect: この論文の pdfft だけを採用（参考文献・合本・補遺を除外）。
    if (!pdfUrl && location.hostname === "www.sciencedirect.com") {
      var article = location.pathname.match(/^\/science\/article\/pii\/([A-Za-z0-9]+)\/?$/);
      if (article) {
        var pdfLink = Array.from(document.querySelectorAll("a[href]")).find(function (a) {
          try {
            var u = new URL(a.href, location.href);
            return u.origin === location.origin &&
              u.pathname === "/science/article/pii/" + article[1] + "/pdfft";
          } catch (e) { return false; }
        });
        if (pdfLink) {
          pdfUrl = pdfLink.href;
          pdfSource = "sciencedirect";
        }
      }
    }
    // 明示的な PDF リンク（ページ上の先頭 1 件。検索結果ページでは無関係な論文になり得る）
    if (!pdfUrl) {
      var links = Array.prototype.slice.call(document.querySelectorAll("a[href]"));
      var hit = links.find(function (a) {
        return /\.pdf(\?|$)/i.test(a.href) || /\/doi\/(e?pdf|pdfdirect)\//.test(a.href);
      });
      if (hit) {
        pdfUrl = hit.href;
        pdfSource = "link";
      }
    }

    // 参考文献欄の PMC リンクは使わず、当該 PMID の full-text links だけを使う。
    var pmcUrl = null;
    if (location.hostname === "pubmed.ncbi.nlm.nih.gov" && pmid &&
        location.pathname === "/" + pmid + "/") {
      var pmc = Array.from(document.querySelectorAll(".full-text-links-list a[href]")).find(function (a) {
        try {
          var u = new URL(a.href);
          return u.protocol === "https:" && u.hostname === "pmc.ncbi.nlm.nih.gov" &&
            u.pathname === "/articles/pmid/" + pmid + "/";
        } catch (e) { return false; }
      });
      if (pmc) pmcUrl = pmc.href;
    }

    var title =
      meta("citation_title") || meta("og:title") || (document.title || "").trim() || null;

    return {
      doi: doi,
      pmid: pmid,
      pdfUrl: pdfUrl,
      pdfSource: pdfSource,
      pmcUrl: pmcUrl,
      title: title,
      url: location.href,
      // タブ自体が PDF（Chrome の PDF ビューアには注入できないので通常 false。pdf.js 系ビューア用）
      isPdf: isPdf,
    };
  }

  function metaRefreshUrl(doc, base) {
    var m = doc.querySelector('meta[http-equiv="refresh" i]');
    if (!m) return null;
    var mm = /url\s*=\s*['"]?([^'"]+)/i.exec(m.getAttribute("content") || "");
    if (!mm) return null;
    try {
      return new URL(mm[1].trim(), base).href;
    } catch (e) {
      return null;
    }
  }

  /* PDF の代わりに HTML が返ったときの「1 回だけ」の追従先（背景タブ経路からの要求時のみ使う）:
     meta refresh があればその先。無ければ、文書中の **同一オリジンでパスが .pdf で終わる URL**
     （a / iframe / embed / object / citation_pdf_url）が **ちょうど 1 つ**のときだけそれ。
     ホスト名の ".pdf" やクエリだけの ".pdf"、別オリジン（補遺配信ホスト等）は数えない。
     ScienceDirect の pdfft → pdf.sciencedirectassets.com 署名 URL の中継ページは meta refresh で拾う
     （出版社名で分岐しない汎用規則）。 */
  function nextPdfUrlFromHtml(html, base) {
    var doc = new DOMParser().parseFromString(html, "text/html");
    var r = metaRefreshUrl(doc, base);
    if (r) return r;
    var origin;
    try {
      origin = new URL(base).origin;
    } catch (e) {
      return null;
    }
    var urls = {};
    var n = 0;
    function add(u) {
      if (!u) return;
      var parsed;
      try {
        parsed = new URL(u, base);
      } catch (e) {
        return;
      }
      if (parsed.origin !== origin) return;
      if (!/\.pdf$/i.test(parsed.pathname)) return;
      if (!urls[parsed.href]) {
        urls[parsed.href] = true;
        n++;
      }
    }
    var c = doc.querySelector('meta[name="citation_pdf_url" i]');
    if (c) add(c.getAttribute("content"));
    var els = doc.querySelectorAll("a[href], iframe[src], embed[src], object[data]");
    for (var i = 0; i < els.length; i++) {
      add(els[i].getAttribute("href") || els[i].getAttribute("src") || els[i].getAttribute("data"));
    }
    return n === 1 ? Object.keys(urls)[0] : null;
  }

  function isPdfBytes(b) {
    return b.length > 4 && b[0] === 0x25 && b[1] === 0x50 && b[2] === 0x44 && b[3] === 0x46;
  }

  /* ページ内 fetch = ユーザーの認証済み session・正しい referrer が乗る。
     purchase/購読済みの paywall PDF はこの経路でのみ取れる。
     `follow` が真のとき（popup の fetchPdfViaTabThenBg = 背景タブ経路と PDF タブ経由の取り直し）だけ、HTML が返ったら
     nextPdfUrlFromHtml の先へ **1 回だけ** 追従する（論文ページでの最初の取得 = fetchPdfBytes は追従しない）。
     PDF でない応答の Error には `html = true` を付ける（popup が「PDF タブで開いて取り直す」経路に回す目印）。 */
  async function fetchPdfBase64(url, follow, hopped) {
    var r;
    try {
      r = await fetch(url, { credentials: "include" });
    } catch (e) {
      // 別オリジンの配信先が `Access-Control-Allow-Origin: *`（署名付き URL 等・Cookie 不要）だと
      // include は CORS で拒否される（TypeError）→ Cookie なしで取り直す
      if (new URL(url, location.href).origin === location.origin) throw e;
      r = await fetch(url, { credentials: "omit" });
    }
    if (!r.ok) throw new Error("HTTP " + r.status);
    var buf = await r.arrayBuffer();
    var b = new Uint8Array(buf);
    if (!isPdfBytes(b)) {
      if (follow && !hopped) {
        var html = new TextDecoder().decode(buf.slice(0, 2 * 1024 * 1024));
        var next = nextPdfUrlFromHtml(html, r.url || url);
        if (next && next !== url) return fetchPdfBase64(next, true, true);
      }
      var err = new Error(
        "PDF ではない応答（" + buf.byteLength + " bytes・HTML の可能性" + (hopped ? "・中継先も HTML" : "") + "）"
      );
      err.html = true;
      throw err;
    }
    var CHUNK = 0x8000;
    var parts = [];
    for (var i = 0; i < b.length; i += CHUNK) {
      parts.push(String.fromCharCode.apply(null, b.subarray(i, i + CHUNK)));
    }
    return btoa(parts.join(""));
  }

  chrome.runtime.onMessage.addListener(function (msg, _sender, sendResponse) {
    if (msg && msg.type === "openref-analyze") {
      try {
        sendResponse({ ok: true, data: analyze() });
      } catch (e) {
        sendResponse({ ok: false, error: String(e) });
      }
      return false;
    }
    if (msg && msg.type === "openref-refresh-url") {
      // 中継ページ（linkinghub.elsevier.com 等）の meta refresh の飛び先。popup の背景タブ経路が着地を待つのに使う
      try {
        sendResponse({ ok: true, url: metaRefreshUrl(document, location.href) });
      } catch (e) {
        sendResponse({ ok: false, error: String(e) });
      }
      return false;
    }
    if (msg && msg.type === "openref-fetch-pdf") {
      fetchPdfBase64(msg.url, !!msg.follow, false)
        .then(function (b64) {
          sendResponse({ ok: true, b64: b64 });
        })
        .catch(function (e) {
          sendResponse({ ok: false, error: String(e), html: !!e.html });
        });
      return true; // async
    }
    return false;
  });
})();
