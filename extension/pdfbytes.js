/* OpenRef Capture — service worker・content script・popup が共有する関数（chrome.* にも global の document にも触らない）。
   読み込み: background.js = importScripts("pdfbytes.js")（classic SW）/ content script = popup が
   executeScript({files: ["pdfbytes.js", "content.js"]}) で content.js の前に注入する / popup = popup.html の <script>。
   同じ isolated world へ何度注入されても最初の 1 回の定義を使う（content.js の __openRefCaptureLoaded と同じ考え）。
   既定義の判定は「関数があるか」で見る: ページの `id="OpenRefShared"` の要素は Window の named property として
   isolated world からも見える（真値になる）ので、真偽だけで見ると定義を飛ばしてしまう。 */
(function (g) {
  "use strict";
  var s = g.OpenRefShared;
  if (s && typeof s.metaRefreshUrl === "function" && typeof s.metaRefreshUrlInDoc === "function" &&
      typeof s.safeDecode === "function") return;

  /* meta refresh の規則（「HTML が返ったら 1 回だけ追従」と背景タブの中継判定の両方）は 1 つ:
     - HTML コメントと noscript の中の meta は見ない（DOMParser はスクリプト無効で解析するので noscript 内の meta が
       要素になる = 「JS が無いときの転送先」を拾ってしまう）
     - delay が 10 秒を超えるものは中継ではない（`1800;url=/logout` 等のセッション期限タイマー。
       connectd の pdffetch.rs META_REFRESH_MAX_DELAY と同じ値）
     - 飛び先が無い / 解決できない meta は飛ばして次の meta を見る。どれも無ければ null
     入力の形が 2 つあるので入口も 2 つ（SW には DOMParser が無い）。どちらも refreshTarget を通る。 */
  function refreshTarget(content, base) {
    var delay = parseInt(content, 10);
    if (!isNaN(delay) && delay > 10) return null; // logout タイマー等は中継ではない
    var mm = /url\s*=\s*['"]?([^'"]+)/i.exec(content);
    if (!mm) return null;
    try {
      return new URL(mm[1].trim(), base).href;
    } catch (e) {
      return null;
    }
  }

  /* HTML 文字列から（service worker）。属性は正規表現で読む */
  function metaRefreshUrl(html, base) {
    html = String(html || "").replace(/<!--[\s\S]*?-->/g, "").replace(/<noscript\b[\s\S]*?<\/noscript>/gi, "");
    var metas = html.match(/<meta\b[^>]*>/gi) || [];
    for (var i = 0; i < metas.length; i++) {
      var m = metas[i];
      if (!/http-equiv\s*=\s*["']?refresh["']?/i.test(m)) continue;
      var c = /content\s*=\s*(?:"([^"]*)"|'([^']*)')/i.exec(m);
      var content = (c ? c[1] || c[2] || "" : "").replace(/&amp;/g, "&").replace(/&#x2f;/gi, "/").replace(/&#47;/g, "/");
      var url = refreshTarget(content, base);
      if (url) return url;
    }
    return null;
  }

  /* Document から（content script: DOMParser で解析した応答・live document）。属性は DOM が読む */
  function metaRefreshUrlInDoc(doc, base) {
    var metas = doc.querySelectorAll('meta[http-equiv="refresh" i]');
    for (var i = 0; i < metas.length; i++) {
      if (metas[i].closest("noscript")) continue;
      var url = refreshTarget(metas[i].getAttribute("content") || "", base);
      if (url) return url;
    }
    return null;
  }

  /* URL の一部の %xx を戻す。壊れた % 列（`%E0%A4%A` 等。ページ上のリンクはページが制御する値）は URIError を投げるので、
     そのときは null =「ここに DOI は無い」として返し、呼び手は次の手がかり（doi.org リンク・本文冒頭・題名のみ保存）へ進む
     （解析全体を落とさない = popup が「解析中…」で固まらず、HTML ページを PDF 扱いしない。% 列のままの DOI も送らない） */
  function safeDecode(s) {
    try {
      return decodeURIComponent(s);
    } catch (e) {
      return null;
    }
  }

  g.OpenRefShared = { metaRefreshUrl: metaRefreshUrl, metaRefreshUrlInDoc: metaRefreshUrlInDoc, safeDecode: safeDecode };
})(globalThis);
