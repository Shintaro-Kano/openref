/* OpenRef Capture — content script（popup と保存の工程 job.js が都度注入・activeTab）。
   役割は 2 つだけ: ①識別子（DOI/PMID）と PDF URL の抽出 ②ページ内 fetch での PDF 取得。
   書誌の組み立ては一切しない（PubMed/Crossref が正 = translator 不要の設計）。
   ページが自分について宣言した書誌メタ（evidence）は、識別子の無いページで connectd が Crossref の候補と照合する証拠にだけ使う
   （保存しない）。
   注入する側（popup・job.js）は pdfbytes.js（OpenRefShared）を先に注入する。 */
(function () {
  "use strict";
  // === true で見る（ページの id="__openRefCaptureLoaded" の要素は Window の named property として見え、真値になる）
  if (window.__openRefCaptureLoaded === true) return;
  window.__openRefCaptureLoaded = true;

  function meta(name) {
    var el = document.querySelector(
      'meta[name="' + name + '" i], meta[property="' + name + '" i]'
    );
    return el ? el.content : null;
  }

  /* 同名のメタ（name / property・大小無視）の値をすべて（文書の順・空は落とす） */
  function metas(name) {
    var els = document.querySelectorAll(
      'meta[name="' + name + '" i], meta[property="' + name + '" i]'
    );
    var out = [];
    for (var i = 0; i < els.length; i++) {
      var c = els[i] && typeof els[i].content === "string" ? els[i].content.trim() : "";
      if (c) out.push(c);
    }
    return out;
  }

  var cleanDoi = OpenRefShared.cleanDoi; // popup（J-STAGE の記事ページの citation_doi）と同じ規則

  /* ページが書いた URL の値を絶対 URL にする。基準は document.baseURI = <a href> の解決（a.href）とページ内 fetch の
     相対 URL の解決と同じ基準（<base href> があればそれ）。http(s) にならない値（javascript:・data:・解決できない値）は null。
     popup の拡張 fetch・chrome.tabs.create・「プロキシ経由で開く」は拡張のオリジンで解決するので、相対のまま渡さない */
  function httpUrl(v) {
    v = typeof v === "string" ? v.trim() : "";
    if (!v) return null; // 空白だけの値は new URL が基準の URL（= ページ自身）を返すので、ここで落とす
    try {
      var u = new URL(v, document.baseURI || location.href);
      return u.protocol === "http:" || u.protocol === "https:" ? u.href : null;
    } catch (e) {
      return null;
    }
  }

  var samePage = OpenRefShared.samePage; // 2 つの URL が同じページか（popup・保存の工程と同じ規則）

  /* 候補（未整形の DOI の列）の中に異なる DOI がちょうど 1 つならそれ（最初に出た表記）、0 か 2 つ以上なら null */
  function onlyDoi(list) {
    var seen = {};
    var first = null;
    var n = 0;
    for (var i = 0; i < list.length; i++) {
      var d = cleanDoi(list[i]);
      if (!d || seen[d.toLowerCase()]) continue;
      seen[d.toLowerCase()] = true;
      if (!first) first = d;
      n++;
    }
    return n === 1 ? first : null;
  }

  /* JSON-LD の論文: @type が ScholarlyArticle / MedicalScholarlyArticle の物が**ちょうど 1 つ**ならそれ、0 か 2 つ以上なら null
     （一覧ページは複数を持つ。Article・NewsArticle 等 = ブログ・ニュースの記事は数えない）。探すのは各 script の最上位・配列・@graph だけ
     （citation などの入れ子 = 参考文献の論文は数えない） */
  var LD_TYPES = ["ScholarlyArticle", "MedicalScholarlyArticle"];
  function ldArticle() {
    var found = [];
    function visit(node) {
      if (Array.isArray(node)) {
        node.forEach(visit);
        return;
      }
      if (!node || typeof node !== "object") return;
      var types = [].concat(node["@type"] || []).map(function (t) {
        return String(t).replace(/^.*[\/:]/, ""); // "schema:ScholarlyArticle"・"https://schema.org/ScholarlyArticle" も
      });
      if (types.some(function (t) { return LD_TYPES.indexOf(t) >= 0; })) found.push(node);
      if (Array.isArray(node["@graph"])) node["@graph"].forEach(visit);
    }
    var scripts = document.querySelectorAll('script[type="application/ld+json"]');
    for (var i = 0; i < scripts.length; i++) {
      try {
        visit(JSON.parse(scripts[i].textContent));
      } catch (e) { /* 壊れた JSON-LD は無いものとする */ }
    }
    return found.length === 1 ? found[0] : null;
  }

  /* ページが**自分について**宣言した書誌メタを、値ごとに出どころ（source）を付けて返す（無ければ null）。
     題名 = citation_title > dc.title > JSON-LD・著者（順の列）= citation_author > dc.creator > JSON-LD・
     日付 = citation_publication_date > citation_date > dc.date > JSON-LD datePublished・誌名 = citation_journal_title > prism.publicationName > JSON-LD isPartOf.name。
     citation_reference（参考文献）・本文・og:title・document.title は使わない（別の論文の題名で照合が通ってしまう） */
  function evidence() {
    var ld = ldArticle();
    function fromMeta(names, all) {
      for (var i = 0; i < names.length; i++) {
        var v = metas(names[i]);
        if (v.length) return { value: all ? v : v[0], source: names[i] };
      }
      return null;
    }
    function fromLd(v) {
      v = typeof v === "string" ? v.trim() : "";
      return v ? { value: v, source: "json-ld" } : null;
    }
    function ldAuthors() {
      var names = [].concat((ld && ld.author) || []).map(function (a) {
        if (typeof a === "string") return a.trim();
        if (!a || typeof a !== "object") return "";
        if (typeof a.familyName === "string" && a.familyName.trim()) {
          return a.familyName.trim() + (typeof a.givenName === "string" && a.givenName.trim() ? ", " + a.givenName.trim() : "");
        }
        return typeof a.name === "string" ? a.name.trim() : "";
      }).filter(Boolean);
      return names.length ? { value: names, source: "json-ld" } : null;
    }
    var ev = {
      title: fromMeta(["citation_title", "dc.title"]) || fromLd(ld && (ld.headline || ld.name)),
      authors: fromMeta(["citation_author", "dc.creator"], true) || ldAuthors(),
      date: fromMeta(["citation_publication_date", "citation_date", "dc.date"]) || fromLd(ld && ld.datePublished),
      journal: fromMeta(["citation_journal_title", "prism.publicationName"]) || fromLd(ld && ld.isPartOf && ld.isPartOf.name),
    };
    var out = null;
    Object.keys(ev).forEach(function (k) {
      if (ev[k]) (out = out || {})[k] = ev[k];
    });
    return out;
  }

  /* proxy = 大学プロキシの前置き（popup が openref-analyze に添える。無ければ今までどおり）。
     ホスト名で分岐するサイト規則（PubMed / PMC・医書.jp・ScienceDirect・J-STAGE）は、EZproxy のホスト名方式を戻した
     出版社のホスト名で見る。取得する URL（pdfUrl・pmcUrl・landingUrl）と url はプロキシのまま（user の session はそこにある） */
  function analyze(proxy) {
    var host = OpenRefShared.deproxifyHost(location.hostname, proxy);
    var doi =
      cleanDoi(meta("citation_doi")) ||
      cleanDoi(meta("dc.identifier")) ||
      cleanDoi(meta("prism.doi")) ||
      cleanDoi(meta("bepress_citation_doi"));
    var pmid = (meta("citation_pmid") || "").trim() || null;

    // URL の規則（PubMed の /<PMID>/・Atypon/Silverchair 系の /doi/…/10.x・医書.jp）は popup の URL だけの解析と同じ 1 つ（pdfbytes.js の urlIds）。
    // メタが無いときだけ使う
    var ids = OpenRefShared.urlIds(location.href, proxy);
    if (!pmid) pmid = ids.pmid;
    if (!doi) doi = ids.doi;
    // 以下の 2 つ（doi.org リンク・本文冒頭）は確度が低い: 目次・参考文献欄の DOI でもありうる。
    // ページ中の**異なる DOI がちょうど 1 つ**（cleanDoi の後・大小無視）のときだけ採り、2 つ以上なら採らない
    // （目次・参考文献欄の DOI を自分の DOI と取り違えない = 迷ったら識別子なし。識別子なしの扱いは保存の工程 job.js の既存の経路）
    // doi.org リンク
    if (!doi) {
      var doiLinks = Array.prototype.slice.call(document.querySelectorAll('a[href*="doi.org/10."]'));
      doi = onlyDoi(doiLinks.map(function (a) {
        var ma = a.href.match(/doi\.org\/(10\.[^\s?#]+)/);
        return ma ? OpenRefShared.safeDecode(ma[1]) : null;
      }));
    }
    // 最終手段: 本文冒頭から DOI
    if (!doi && !pmid && document.body) {
      doi = onlyDoi(document.body.innerText
        .slice(0, 30000)
        .match(/\b10\.\d{4,9}\/[-._;()/:A-Za-z0-9]+/g) || []);
    }

    var isPdf = document.contentType === "application/pdf";
    // pdfSource = PDF URL の出どころ（保存の工程が「本文から識別子を取る経路」に回すかの判断に使う = pdfbytes.js の shouldIngestPdf:
    // 識別子の無いページでは meta / self 由来だけを論文本体とみなし、ページ上のリンク拾いは信用しない）
    // 相対 URL はページを基準に解決する。http(s) にならない値は宣言として採らず、後段（タブ自体・Atypon・リンク）へ落とす
    var pdfUrl = httpUrl(meta("citation_pdf_url"));
    // HTML のページの宣言が自分自身を指す値（"#x"・"?"・自分のパス等）は採らない（pdfbytes.js の isPdfTab = pdfUrl === url が
    // HTML を PDF タブと見誤り、背景タブで同じ記事ページを開き直す）。タブ自体が PDF なら自分を指すのは正しいので、今までどおり meta
    if (pdfUrl && !isPdf && samePage(pdfUrl, location.href)) pdfUrl = null;
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
    if (!pdfUrl && host === "www.sciencedirect.com") {
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
      // a.href は DOM が解決済みの絶対 URL。javascript: 等の http(s) でないリンクは採らない
      var hit = links.find(function (a) {
        return httpUrl(a.href) && OpenRefShared.looksLikePdfUrl(a.href); // popup の URL だけの解析と同じ規則
      });
      if (hit) {
        pdfUrl = hit.href;
        pdfSource = "link";
      }
    }

    // 参考文献欄の PMC リンクは使わず、当該 PMID の full-text links だけを使う。
    // リンクのホストは戻さない（プロキシが書き換えた PMC のリンクは保存の工程の PMC 本文経由の照合 = pmcLinkFor に合わないので使わない = 従来どおり）
    // 持つのは正準の URL（pdfbytes.js の pmcLinkFor = 保存の工程が開く前の照合と同じ 1 つ。クエリ・断片は落とす）
    var pmcUrl = null;
    if (host === "pubmed.ncbi.nlm.nih.gov" && pmid &&
        location.pathname === "/" + pmid + "/") {
      var fullText = document.querySelectorAll(".full-text-links-list a[href]");
      for (var i = 0; i < fullText.length && !pmcUrl; i++) pmcUrl = OpenRefShared.pmcLinkFor(fullText[i].href, pmid);
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
      // J-STAGE の PDF（…/_pdf/-char/ja）なら記事ページの URL。DOI はそこにだけあり、popup が引く（fillFromJstageArticle）
      landingUrl: OpenRefShared.jstageArticleUrl(location.href, proxy),
      // タブ自体が PDF（Chrome の PDF ビューアの文書も application/pdf = Chrome for Testing 156 で注入できた・pdf.js 系ビューアも）
      isPdf: isPdf,
      // ページが自分について宣言した書誌メタ（照合の証拠だけ・保存しない）。popup が /capture/save に添える
      evidence: evidence(),
    };
  }

  /* PDF の代わりに HTML が返ったときの「1 回だけ」の追従先（背景タブ経路からの要求時のみ使う）:
     meta refresh があればその先。無ければ、文書中の **同一オリジンでパスが .pdf で終わる URL**
     （a / iframe / embed / object / citation_pdf_url）が **ちょうど 1 つ**のときだけそれ。
     ホスト名の ".pdf" やクエリだけの ".pdf"、別オリジン（補遺配信ホスト等）は数えない。
     ScienceDirect の pdfft → pdf.sciencedirectassets.com 署名 URL の中継ページは meta refresh で拾う
     （出版社名で分岐しない汎用規則）。meta refresh の規則は pdfbytes.js（background.js と共有）。 */
  function nextPdfUrlFromHtml(html, base) {
    var doc = new DOMParser().parseFromString(html, "text/html");
    var r = OpenRefShared.metaRefreshUrlInDoc(doc, base);
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

  /* ページ内 fetch = ユーザーの認証済み session・正しい referrer が乗る。
     purchase/購読済みの paywall PDF はこの経路でのみ取れる。
     `follow` が真のとき（保存の工程 job.js の fetchPdfViaTabThenBg = 背景タブ経路と PDF タブ経由の取り直し）だけ、HTML が返ったら
     nextPdfUrlFromHtml の先へ **1 回だけ** 追従する（論文ページでの最初の取得 = fetchPdfBytes は追従しない）。
     PDF でない応答の Error には `html = true` を付ける（保存の工程が「PDF タブで開いて取り直す」経路に回す目印）。
     失敗の Error には HTTP の状態（`httpStatus`）と転送の着き先（`url`）も付ける（保存の工程の失敗の分類が使う。401・ログイン画面の URL）。 */
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
    if (!r.ok) {
      var he = new Error("HTTP " + r.status);
      he.httpStatus = r.status;
      he.url = r.url || url;
      throw he;
    }
    var buf = await r.arrayBuffer();
    var b = new Uint8Array(buf);
    if (!OpenRefShared.isPdfBytes(b)) { // %PDF の判定は拡張 fetch（background.js）と同じ 1 つ（pdfbytes.js）
      if (follow && !hopped) {
        var html = new TextDecoder().decode(buf.slice(0, 2 * 1024 * 1024));
        var next = nextPdfUrlFromHtml(html, r.url || url);
        if (next && next !== url) return fetchPdfBase64(next, true, true);
      }
      var err = new Error(
        "PDF ではない応答（" + buf.byteLength + " bytes・HTML の可能性" + (hopped ? "・中継先も HTML" : "") + "）"
      );
      err.html = true;
      err.url = r.url || url;
      throw err;
    }
    return OpenRefShared.bytesToB64(b);
  }

  chrome.runtime.onMessage.addListener(function (msg, _sender, sendResponse) {
    if (msg && msg.type === "openref-analyze") {
      try {
        sendResponse({ ok: true, data: analyze(msg.proxy) });
      } catch (e) {
        sendResponse({ ok: false, error: String(e) });
      }
      return false;
    }
    if (msg && msg.type === "openref-refresh-url") {
      // 中継ページ（linkinghub.elsevier.com 等）の meta refresh の飛び先。保存の工程の背景タブ経路が着地を待つのに使う
      // （規則は取得した HTML への追従と同じ = pdfbytes.js。delay > 10 秒のセッション期限タイマーを中継と誤認しない）
      try {
        sendResponse({ ok: true, url: OpenRefShared.metaRefreshUrlInDoc(document, location.href) });
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
          sendResponse({ ok: false, error: String(e), html: !!e.html, httpStatus: e.httpStatus || 0, url: e.url || null });
        });
      return true; // async
    }
    return false;
  });
})();
