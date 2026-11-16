/* OpenRef Capture — service worker・content script・popup が共有する関数（chrome.* にも global の document にも触らない）。
   読み込み: background.js = importScripts("pdfbytes.js", "job.js")（classic SW。保存の工程 job.js もこれを使う）/
   content script = popup や工程が executeScript({files: ["pdfbytes.js", "content.js"]}) で content.js の前に注入する /
   popup = popup.html の <script> / 設定 = options.html の <script>（前置きの検査に proxied を使う）。
   同じ isolated world へ何度注入されても最初の 1 回の定義を使う（content.js の __openRefCaptureLoaded と同じ考え）。
   既定義の判定は、末尾で組む「出す物の一覧」（shared）の名前がすべて同じ型で既にあるかで見る（名前の列を手で持たない = 足した名前を
   見落とさない。ページの `id="OpenRefShared"` の要素は Window の named property として isolated world からも見える = 真偽だけでは見ない）。 */
(function (g) {
  "use strict";

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

  /* DOI の後処理（拡張の DOI はすべてここを通る: content.js の抽出・URL の規則 urlIds・popup が J-STAGE の記事ページから読む citation_doi）:
     前置き（doi: / doi.org の URL）と末尾の句読点（. , ; ]）を外し、10.<登録者>/ で始まらなければ null。
     末尾の ")" は "(" より多いときだけ外す（10.1000/a(b) の ")" は DOI の一部・"(doi:10.1000/x)" の ")" は文の括弧）。
     connectd は DOI を trim と小文字化しかしないので、句読点はここで落としておく（付いたままだと書誌の確定と重複照合が外れる） */
  function cleanDoi(s) {
    if (!s) return null;
    s = s.trim().replace(/^doi:\s*/i, "").replace(/^https?:\/\/(dx\.)?doi\.org\//i, "");
    for (;;) {
      var t = s.replace(/[.,;\]]+$/, "");
      if (t.slice(-1) === ")" && t.split("(").length < t.split(")").length) t = t.slice(0, -1);
      if (t === s) break;
      s = t;
    }
    return /^10\.\d{4,9}\//.test(s) ? s : null;
  }

  /* URL から読める識別子（content.js の analyze と popup の URL だけの解析が同じ 1 つを使う）。{doi, pmid, isho}:
     - PubMed: ホストが pubmed.ncbi.nlm.nih.gov でパスが /<数字>/ だけ（桁の制限なし = core の parse_input と同じ。ほかの URL の中の PubMed の URL は見ない）
     - DOI: パスの /doi/(full|abs|pdf|epdf|pdfdirect/)?10.x（クエリ・断片は見ない = クエリの中の URL から DOI を作らない）
     - 医書.jp: /journal/detail/(pdf|abs)/<DOI>（isho = "pdf" | "abs"・PDF 自体に DOI の印字がなくても出版社 URL に識別子がある）
     ホストは前置きの EZproxy のホスト名方式を戻してから見る。%xx は safeDecode（壊れた % = DOI なし）・DOI は cleanDoi を通す。
     医書.jp の URL のパスに /doi/10. が入る型は /doi/ の規則が先に勝つ（content.js の前の挙動と同じ。popup の前の挙動は医書.jp の規則が上書きしていた） */
  function urlIds(url, proxy) {
    var out = { doi: null, pmid: null, isho: null };
    var x;
    try {
      x = new URL(plainUrl(url, proxy));
    } catch (e) {
      return out;
    }
    var path = x.pathname;
    if (x.hostname === "pubmed.ncbi.nlm.nih.gov") {
      var mp = /^\/(\d+)\/?$/.exec(path);
      if (mp) out.pmid = mp[1];
    }
    var md = path.match(/\/doi\/(?:full\/|abs\/|pdf\/|epdf\/|pdfdirect\/)?(10\.\d{4,9}\/[^\s?#]+)/);
    if (md) out.doi = cleanDoi(safeDecode(md[1]));
    if (x.hostname === "webview.isho.jp") {
      var mi = path.match(/^\/journal\/detail\/(pdf|abs)\/(10\.\d{4,9}\/.+)$/);
      if (mi) {
        out.isho = mi[1];
        if (!out.doi) out.doi = cleanDoi(safeDecode(mi[2]));
      }
    }
    return out;
  }

  /* URL の形が PDF のファイルか: パスかクエリが ".pdf" で終わる（直後が ? # 末尾）。保存の工程の着地（content script を入れられないページ）はこれだけを見る */
  function hasPdfExtension(u) {
    return /\.pdf([?#]|$)/i.test(String(u || ""));
  }
  /* PDF の URL らしいか（content.js のページ上のリンク拾いと popup の URL だけの解析）: hasPdfExtension か Atypon 系の /doi/(e)pdf|pdfdirect/ */
  function looksLikePdfUrl(u) {
    return hasPdfExtension(u) || /\/doi\/(e?pdf|pdfdirect)\//.test(String(u || ""));
  }

  /* 2 つの URL が同じページか（オリジン + パス + クエリ。断片は見ない。空のクエリ "?" はクエリ無しと同じ = URL.search が "" を返す）。
     読めない値は別のページ。http(s) でない URL（chrome:// ・chrome-extension:// ・about: ・file: 等）は文字列が完全に同じときだけ同じ
     （標準でないスキームの origin は環境によって "null" になり、chrome://newtab/ と chrome://settings/ を同じと見てしまう = 安全側に倒す）。
     使う所: citation_pdf_url がページ自身か（content.js）・記録がこのタブのページか（popup）・利用者のタブが保存を押したページのままか（job.js） */
  function samePage(a, b) {
    try {
      var x = new URL(a);
      var y = new URL(b);
      var web = function (u) { return u.protocol === "http:" || u.protocol === "https:"; };
      if (!web(x) || !web(y)) return String(a) === String(b);
      return x.origin + x.pathname + x.search === y.origin + y.pathname + y.search;
    } catch (e) {
      return false;
    }
  }

  /* PubMed の full-text links の PMC のリンクが、PMID pmid の PMC 本文を指すか。指せば正準の URL（クエリ・断片を落とす）、指さなければ null。
     別の PMID・別のホスト・http は断る。content.js が持つ値と、保存の工程が開く前の照合が同じ 1 つを使う */
  function pmcLinkFor(href, pmid) {
    if (!pmid) return null;
    var u;
    try {
      u = new URL(href);
    } catch (e) {
      return null;
    }
    var path = "/articles/pmid/" + pmid + "/";
    return u.protocol === "https:" && u.hostname === "pmc.ncbi.nlm.nih.gov" && u.pathname === path ? "https://pmc.ncbi.nlm.nih.gov" + path : null;
  }

  /* J-STAGE の PDF の URL（/article/<誌>/<巻>/<号>/<記事>/_pdf・後ろに /-char/<言語> とクエリが付きうる）→ 記事ページ
     （_pdf → _article。/-char/<言語> は保つ・クエリは落とす）の URL。当たらなければ null。
     DOI は記事ページの citation_doi にだけある（PDF の URL にも冒頭 2 頁にも無い）。
     記事ページは popup が引く（popup.js の fillFromJstageArticle。PDF タブに content script を入れられない Chrome もありうる =
     Chrome for Testing 156 では入った）。
     proxy = 大学プロキシの前置き。あればホスト名は deproxifyHost の後で見る。返す URL のオリジンはタブのまま
     （プロキシの中なら記事ページもプロキシ経由で引く = user の session はそこにある） */
  function jstageArticleUrl(url, proxy) {
    var u;
    try {
      u = new URL(url);
    } catch (e) {
      return null;
    }
    if (u.protocol !== "https:" || deproxifyHost(u.hostname, proxy) !== "www.jstage.jst.go.jp") return null;
    var m = u.pathname.match(/^(\/article\/[^/]+\/[^/]+\/[^/]+\/[^/]+\/)_pdf(\/-char\/[a-z]+)?\/?$/i);
    return m ? u.origin + m[1] + "_article" + (m[2] || "") : null;
  }

  /* ===== 大学プロキシの前置き（実験的）。設定は拡張のオプション（options.html）の 1 つだけ =====
     受けるのは https:// の URL だけ。見た目のホスト（https:// から最初の / ? # まで）と、例の URL を通して組んだ URL を
     new URL で読んだホストが同じで、ユーザー名・パスワードが無いときだけ {text, host}。ほかは {error: 理由}
     （呼び手 = proxied / deproxifyHost は「設定なし」と同じ扱い・設定のページは理由を出す）。
     はじく例: パスが無い（https://login.ezproxy.example.ac.jp → 付けた URL がホストに混ざる）・ホストの中の {url}・
     user:pw@・https://evil.com\@login…（URL の規則では \ は / なのでホストは evil.com） */
  var PROXY_SAMPLE = "https://doi.org/10.1000/xyz123";
  function parseProxy(template) {
    var t = typeof template === "string" ? template.trim() : "";
    if (!t) return { error: "前置きが空です" };
    if (!/^https:\/\//i.test(t)) return { error: "https:// で始まる URL を入れてください" };
    if (/\s/.test(t)) return { error: "空白を含めないでください" };
    var rest = t.slice("https://".length);
    var end = rest.search(/[\/?#]/);
    if (end < 0) return { error: "ホスト名の後にパスが要ります（例: https://login.ezproxy.example.ac.jp/login?url=）" };
    var authority = rest.slice(0, end).toLowerCase();
    if (authority.indexOf("\\") >= 0) return { error: "ホスト名に \\ は使えません" };
    if (authority.indexOf("@") >= 0) return { error: "ユーザー名・パスワード（@）は入れられません" };
    if (authority.indexOf("{url}") >= 0) return { error: "{url} はホスト名の中に置けません（パスかクエリに置いてください）" };
    var built;
    try {
      built = new URL(buildProxied(t, PROXY_SAMPLE));
    } catch (e) {
      return { error: "URL として読めません" };
    }
    if (built.protocol !== "https:" || built.username || built.password || !built.hostname || built.host !== authority) {
      return { error: "ホスト名が読み違えられます（" + authority + " ≠ " + built.host + "）" };
    }
    return { text: t, host: built.hostname };
  }
  function buildProxied(text, targetUrl) {
    return text.indexOf("{url}") >= 0 ? text.split("{url}").join(encodeURIComponent(targetUrl)) : text + targetUrl;
  }

  /* 前置きを通した URL。{url} があればそこへ targetUrl を encodeURIComponent で入れ（OpenAthens の redirector）、
     無ければ末尾にそのまま付ける（EZproxy の login?url=）。前置きが無い / 受けられないときは null（呼び手は前置きなしで動く） */
  function proxied(template, targetUrl) {
    var p = parseProxy(template);
    if (!p.host || !targetUrl) return null;
    return buildProxied(p.text, targetUrl);
  }

  /* EZproxy のホスト名方式（www-sciencedirect-com.<base>）を出版社のホスト名（www.sciencedirect.com）に戻す。
     <base> = 前置きのホスト（先頭の login. の段は外す）。戻すのはホスト名で分岐するサイト規則と、保存する URL だけ
     （取得する URL はプロキシのまま）。<base> の下の 1 段だけを戻す: "--" → "-"・"-" → "."。
     前置きが無い・<base> の下でない・2 段以上（ドットのまま = 規則が分からない）・戻してもドットの無い物（プロキシ自身の
     login.<base>・menu.<base> = 出版社のホストではない）はそのまま返す。プロキシの自動検出はしない */
  function deproxifyHost(host, template) {
    var p = parseProxy(template);
    if (!p.host || typeof host !== "string") return host;
    var suffix = "." + p.host.replace(/^login\./, "");
    var h = host.toLowerCase();
    if (h.length <= suffix.length || h.slice(-suffix.length) !== suffix) return host;
    var label = h.slice(0, -suffix.length);
    if (label.indexOf(".") >= 0) return host;
    var plain = label.replace(/--|-/g, function (m) {
      return m === "--" ? "-" : ".";
    });
    return plain.indexOf(".") >= 0 ? plain : host;
  }

  /* ===== popup（解析・表示）と保存の工程（service worker の job.js）の両方が使う物 ===== */
  var BASE = "http://127.0.0.1:23119"; // connectd（Capture の口）
  var ALL_SITES = { origins: ["https://*/*", "http://*/*"] }; // optional_host_permissions（全サイト）
  var DECLINED_KEY = "openref_allsites_declined"; // chrome.storage.local: 全サイト許可を拒否した記録
  var PROXY_KEY = "openref_proxy_template"; // chrome.storage.local: 大学プロキシの前置き（設定のページが書く）
  var JOB_KEY = "openref_job"; // chrome.storage.session: 保存の工程の記録（1 件。書くのは service worker だけ）
  // 工程全体の上限。超えた工程は「中断」と記録し、以後 connectd へ送らない。開始がこれより古い「走行中」の記録は
  // 中断として扱う（以後の保存が断られ続けないように）。値の根拠は job.js の JOB_LIMIT の説明
  var JOB_LIMIT_MS = 15 * 60 * 1000;
  // ページが宣言した「この論文の PDF」の所在（content.js の pdfSource）。link = ページ上の .pdf リンク拾いは含めない
  var DECLARED = ["meta", "self", "atypon", "sciencedirect"];
  /* ページが宣言した PDF の URL があるか（page = 解析の結果）。確認の窓・プロキシ経由で開く先・PDF タブで開き直す条件が使う */
  function declaredPdf(page) {
    return !!page.pdfUrl && DECLARED.indexOf(page.pdfSource) >= 0;
  }
  // 確認の窓を出せる失敗の分類（popup のボタンの条件と、保存の工程が材料を残す条件が同じ 1 つ）
  var RETRY_CLASSES = ["challenge", "stuck", "login"];

  /* バイト列が PDF か（先頭が %PDF）。ページ内 fetch（content.js）と拡張 fetch（background.js）が同じ 1 つを使う */
  function isPdfBytes(b) {
    return b.length > 4 && b[0] === 0x25 && b[1] === 0x50 && b[2] === 0x44 && b[3] === 0x46;
  }
  /* バイト列（Uint8Array）→ base64。32 KB ずつ文字列にする（String.fromCharCode の引数の数の上限を超えない） */
  function bytesToB64(b) {
    var CHUNK = 0x8000;
    var parts = [];
    for (var i = 0; i < b.length; i += CHUNK) {
      parts.push(String.fromCharCode.apply(null, b.subarray(i, i + CHUNK)));
    }
    return btoa(parts.join(""));
  }

  /* 保存された前置きのうち、使える物（https:// で受けられる物）だけ。ほかは null = 使わない（今までどおり） */
  function usableProxy(v) {
    var t = typeof v === "string" ? v.trim() : "";
    return t && proxied(t, "https://doi.org/") ? t : null;
  }

  /* EZproxy のホスト名方式（www-sciencedirect-com.<base>）を出版社のホスト名に戻した URL（前置きが無ければそのまま）。
     使うのはホスト名で分岐するサイト規則と、保存する URL（題名のみの document の URL 欄）だけ。取得する URL には使わない
     （取得はプロキシのホストへ = user の session はそこにある） */
  function plainUrl(u, proxy) {
    if (!proxy || !u) return u;
    try {
      var x = new URL(u);
      x.hostname = deproxifyHost(x.hostname, proxy);
      return x.href;
    } catch (e) {
      return u;
    }
  }

  /* ログイン画面の URL か: ホストの段（. 区切り）かパスの段（/ 区切り・末尾の英字の拡張子 = .php .sso 等を外す）が LOGIN_WORDS のどれかと一致。
     部分一致にはしない（author は auth ではない）。数字の拡張子は外さない（DOI の段 auth.123 を auth と読まない）。サイトの一覧は持たない。クエリは見ない。
     前置きの設定があれば、ホストは EZproxy のホスト名方式を戻してから見る（www-sciencedirect-com.ezproxy.<大学> の
     ezproxy の段で、プロキシの中のページをすべてログイン画面と読まない。プロキシのログイン画面 login.ezproxy.<大学>/login は残る） */
  var LOGIN_WORDS = ["login", "signin", "sign-in", "sso", "idp", "shibboleth", "openathens", "wayf", "ezproxy", "auth"];
  function isLoginUrl(u, proxy) {
    var x;
    try {
      x = new URL(u);
    } catch (e) {
      return false;
    }
    var parts = deproxifyHost(x.hostname, proxy).toLowerCase().split(".")
      .concat(x.pathname.toLowerCase().split("/").map(function (seg) { return seg.replace(/\.[a-z]{2,5}$/, ""); }));
    return parts.some(function (part) { return LOGIN_WORDS.indexOf(part) >= 0; });
  }

  /* タブ自体が PDF（取込経路の判定と popup のバッジ表示で同じ判定を使う）。page = 解析の結果 */
  function isPdfTab(page) {
    return page.isPdf || (page.pdfUrl && page.pdfUrl === page.url);
  }

  /* PDF の中身から識別子を取る経路（/capture/pdf）に回すべきか。
     識別子が無く、かつ ①タブ自体が PDF ②citation_pdf_url がある（ページ自身が論文本体の PDF を宣言）
     ③content script を注入できなかった（chrome:// 等・PDF ビューアに入れられない Chrome。URL が .pdf で終わらない出版社 PDF も
     ここで拾う。中身が PDF かどうかは fetch したバイト列で判定し、違えば題名のみの保存へ落ちる）のいずれか。
     ページ上のリンク拾い（pdfSource "link" / "atypon"）は検索結果ページで無関係な論文を登録し得るので回さない */
  function shouldIngestPdf(page, contentScriptOk) {
    if (page.pmid || page.doi) return false;
    if (isPdfTab(page)) return true;
    if (page.pdfSource === "meta") return true;
    return !contentScriptOk;
  }

  function doiUrl(doi) {
    return "https://doi.org/" + encodeURIComponent(doi).replace(/%2F/gi, "/");
  }

  /* p が ms 以内に決まらなければ「<what> が N 秒以内に応答しません」で reject */
  function withTimeout(p, ms, what) {
    return Promise.race([
      p,
      new Promise(function (_, rej) {
        setTimeout(function () { rej(new Error(what + " が " + Math.round(ms / 1000) + " 秒以内に応答しません")); }, ms);
      }),
    ]);
  }

  var shared = {
    metaRefreshUrl: metaRefreshUrl, metaRefreshUrlInDoc: metaRefreshUrlInDoc, safeDecode: safeDecode,
    cleanDoi: cleanDoi, urlIds: urlIds, hasPdfExtension: hasPdfExtension, looksLikePdfUrl: looksLikePdfUrl, samePage: samePage,
    pmcLinkFor: pmcLinkFor, jstageArticleUrl: jstageArticleUrl, proxied: proxied, deproxifyHost: deproxifyHost,
    proxyCheck: parseProxy, // 設定のページ用: {text, host}（送り先のホスト）か {error: 理由}
    BASE: BASE, ALL_SITES: ALL_SITES, DECLINED_KEY: DECLINED_KEY, PROXY_KEY: PROXY_KEY, JOB_KEY: JOB_KEY,
    JOB_LIMIT_MS: JOB_LIMIT_MS, DECLARED: DECLARED,
    usableProxy: usableProxy, plainUrl: plainUrl, isLoginUrl: isLoginUrl, isPdfTab: isPdfTab, shouldIngestPdf: shouldIngestPdf,
    doiUrl: doiUrl, withTimeout: withTimeout, isPdfBytes: isPdfBytes, bytesToB64: bytesToB64, declaredPdf: declaredPdf,
    RETRY_CLASSES: RETRY_CLASSES, PROXY_SAMPLE: PROXY_SAMPLE,
  };
  var s = g.OpenRefShared;
  if (s && Object.keys(shared).every(function (k) { return typeof s[k] === typeof shared[k]; })) return;
  g.OpenRefShared = shared;
})(globalThis);
