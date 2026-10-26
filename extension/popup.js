/* OpenRef Capture — popup。フロー:
   ①connectd 接続確認 ②ページ解析（content script 注入 → 識別子/PDF URL。Chrome の PDF ビューアには
     注入できないので URL だけで解析する）
   ③識別子あり: /capture/save（書誌は PubMed/Crossref で確定）→ PDF はページ内 fetch → 拡張 fetch →
     /capture/attachment（出どころ source= と冒頭 2 頁の DOI 照合つき）。PDF リンクが無い（PubMed 等）/ 取れない → OpenRef 側の連鎖
     （/capture/fetch-oa = PMC AWS Open Data → Unpaywall → Crossref link → doi.org 着地ページ）→ それも駄目なら⑥
   ④識別子なし・PDF タブ（or citation_pdf_url あり）: PDF のバイト列を取って /capture/pdf へ。connectd が本文から
     DOI/PMID を抽出 → 書誌確定 → 既存 item に添付 or 新規登録（識別子が無ければ要確認 stub）
   ⑤識別子なし・PDF も無い（ページ上のリンク拾いは信用しない）: /capture/save（題名のみの document・捏造しない）
   ⑥層 2（実験的）: ③の OA 経路も失敗し DOI があれば **DOI ページを背景タブで開いて取得**（viaDoiTab）。全サイト許可が要るので
     初回の保存クリック時に 1 度だけ求め、拒否は記録して以後は聞かない */
"use strict";

const BASE = "http://127.0.0.1:23119";
const ALL_SITES = { origins: ["https://*/*", "http://*/*"] };
const DECLINED_KEY = "openref_allsites_declined";
const DOI_TAB_TIMEOUT_MS = 20000;
const STEP_TIMEOUT_MS = 10000; // executeScript / sendMessage 1 回あたり
let allSites = false; // optional_host_permissions（全サイト）が付与済みか
let allSitesDeclined = false; // 過去に拒否した記録（init で読む。クリック時に await を挟まないため）
let savedDoi = null; // /capture/save が確定した DOI（応答に doi キーがあればそれ。無い旧 connectd だけページ抽出の DOI）。層 2 が使う
let tab = null;
let page = null; // {doi, pmid, pdfUrl, pdfSource, title, url, isPdf}
let contentScriptOk = false;

const $ = (id) => document.getElementById(id);
const setStatus = (msg, cls) => {
  $("status").textContent = msg;
  $("status").className = cls || "";
};
const kb = (n) => Math.round(n / 1024) + " KB";
/* 表示用: 保存 CSL の題名は Crossref のマークアップ（<i> <sup> <sub> <scp>）込みのまま（citeproc の書式用）なので、表示では剥がす */
const plain = (t) => String(t || "").replace(/<[^>]+>/g, "").replace(/&amp;/g, "&").replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/\s+/g, " ").trim();

async function api(path, opts) {
  const r = await fetch(BASE + path, opts);
  const body = await r.json().catch(() => ({}));
  if (!r.ok) {
    const err = new Error(body.error || "HTTP " + r.status);
    err.status = r.status; // 409 = 別論文（found_doi）等を呼び手が見分けられるように
    err.body = body;
    throw err;
  }
  return body;
}

function sendToTab(msg, tabId) {
  return new Promise((resolve) => {
    chrome.tabs.sendMessage(tabId == null ? tab.id : tabId, msg, (res) => {
      if (chrome.runtime.lastError) resolve({ ok: false, error: chrome.runtime.lastError.message });
      else resolve(res || { ok: false, error: "no response" });
    });
  });
}
function sendToBg(msg) {
  return new Promise((resolve) => {
    chrome.runtime.sendMessage(msg, (res) => {
      if (chrome.runtime.lastError) resolve({ ok: false, error: chrome.runtime.lastError.message });
      else resolve(res || { ok: false, error: "no response" });
    });
  });
}

/* PDF ビューア等で content script が注入できない場合の URL-only 解析 */
function urlOnlyAnalyze(url) {
  const out = { doi: null, pmid: null, pdfUrl: null, pdfSource: null, title: null, url, isPdf: false };
  const mp = url.match(/pubmed\.ncbi\.nlm\.nih\.gov\/(\d{4,9})/);
  if (mp) out.pmid = mp[1];
  const md = url.match(/\/doi\/(?:full\/|abs\/|pdf\/|epdf\/|pdfdirect\/)?(10\.\d{4,9}\/[^\s?#]+)/);
  if (md) out.doi = decodeURIComponent(md[1]).replace(/[).,;\]]+$/, "");
  if (/\.pdf(\?|$)/i.test(url) || /\/doi\/(e?)pdf\//.test(url) || /\/pdfft\b/.test(url)) {
    out.pdfUrl = url;
    out.pdfSource = "self";
    out.isPdf = true;
  }
  return out;
}

/* PDF の中身から識別子を取る経路（/capture/pdf）に回すべきか。
   識別子が無く、かつ ①タブ自体が PDF ②citation_pdf_url がある（ページ自身が論文本体の PDF を宣言）
   ③content script を注入できなかった（Chrome の PDF ビューアの典型。URL が .pdf で終わらない出版社 PDF も
   ここで拾う。中身が PDF かどうかは fetch したバイト列で判定し、違えば⑤へ落ちる）のいずれか。
   ページ上のリンク拾い（pdfSource "link" / "atypon"）は検索結果ページで無関係な論文を登録し得るので回さない */
function shouldIngestPdf() {
  if (!page || page.pmid || page.doi) return false;
  if (page.isPdf || (page.pdfUrl && page.pdfUrl === page.url)) return true;
  if (page.pdfSource === "meta") return true;
  return !contentScriptOk;
}

function renderPage() {
  $("title").textContent = page.title || "（タイトル不明）";
  const bits = [];
  if (page.pmid) bits.push("PMID " + page.pmid);
  if (page.doi) bits.push("DOI " + page.doi);
  let badge;
  if (page.isPdf || (page.pdfUrl && page.pdfUrl === page.url)) badge = '<span class="badge ok">PDF タブ</span>';
  else if (page.pdfUrl) badge = '<span class="badge ok">PDF あり</span>';
  else if (!contentScriptOk && !page.pmid && !page.doi) badge = '<span class="badge ok">PDF タブ？</span>';
  else badge = '<span class="badge no">PDF リンクなし</span>';
  if (!page.pmid && !page.doi) {
    bits.push(
      shouldIngestPdf()
        ? "識別子なし → PDF 本文から DOI/PMID を抽出して登録"
        : "識別子なし（タイトルのみで保存）"
    );
  }
  $("ids").innerHTML = badge + bits.join(" · ");
  $("save").disabled = !(page.pmid || page.doi || page.title || shouldIngestPdf());
}

async function init() {
  // 層 2 用: popup が閉じられたら background が背景タブを閉じる（port 切断・SW 起動時・90 秒 alarm。登録済み id のみ）。
  // SW は 30 秒無活動で止まるので、popup が開いている間は 20 秒ごとに ping して延命する（Chrome 114+ の規則）。
  try {
    chrome.runtime.connect({ name: "openref-popup" });
  } catch (e) { /* 無くても alarms で閉じる */ }
  setInterval(() => {
    sendToBg({ type: "openref-ping" });
  }, 20000);
  try {
    allSites = await chrome.permissions.contains(ALL_SITES);
  } catch (e) {
    allSites = false;
  }
  try {
    const v = await chrome.storage.local.get(DECLINED_KEY);
    allSitesDeclined = !!v[DECLINED_KEY];
  } catch (e) {
    allSitesDeclined = false;
  }

  // 接続確認
  try {
    const s = await api("/capture/status");
    $("conn").textContent = "✓ OpenRef（" + s.items + " 件）";
  } catch (e) {
    $("conn").textContent = "✗ 未接続";
    setStatus("OpenRef（connectd）に接続できません。アプリ/デーモンの起動を確認してください。", "err");
    $("title").textContent = "—";
    return;
  }
  // フォルダ一覧
  try {
    const c = await api("/capture/collections");
    for (const col of c.collections) {
      const o = document.createElement("option");
      o.value = col.id;
      o.textContent = col.path + "（" + col.n_items + "）";
      $("collection").appendChild(o);
    }
  } catch (e) { /* 無くても保存は可能 */ }

  // ページ解析
  [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  try {
    await chrome.scripting.executeScript({ target: { tabId: tab.id }, files: ["content.js"] });
    const res = await sendToTab({ type: "openref-analyze" });
    if (res.ok) {
      page = res.data;
      contentScriptOk = true;
    }
  } catch (e) { /* PDF ビューア等（注入不可）*/ }
  if (!page) page = urlOnlyAnalyze(tab.url || "");
  if (!page.title && tab.title) page.title = tab.title;
  renderPage();
  setStatus("");
}

function b64ToBytes(b64) {
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return bytes;
}

/* PDF バイト列の取得: ページ内 fetch（認証済み session・正しい referrer）→ 拡張（service worker）fetch。
   戻り値 {bytes, how} か {bytes: null, error} */
async function fetchPdfBytes(url) {
  const errors = [];
  if (contentScriptOk) {
    setStatus("PDF を取得中…（ページ内 fetch）");
    const r1 = await sendToTab({ type: "openref-fetch-pdf", url });
    if (r1.ok) return { bytes: b64ToBytes(r1.b64), how: "ページ内" };
    errors.push("ページ内 fetch: " + r1.error);
  }
  setStatus("PDF を取得中…（拡張から fetch）");
  const r2 = await sendToBg({ type: "openref-bg-fetch-pdf", url });
  if (r2.ok) return { bytes: b64ToBytes(r2.b64), how: "拡張" };
  errors.push("拡張 fetch: " + r2.error);
  return { bytes: null, error: errors.join(" / ") };
}

/* OA 経路の失敗文から末尾の一般案内（core の定型文）を落とし、tried 連鎖だけ見せる */
function oaReason(e) {
  const m = String((e && e.message) || e);
  const i = m.indexOf("。paywall");
  return i > 0 ? m.slice(0, i) : m;
}

/* 識別子ありの item に PDF を付ける: ブラウザ fetch → /capture/attachment → 駄目なら OA 経路。
   hasIds = false（識別子なしの document）なら OA 経路は対象外（PMCID/DOI が無いので無駄打ちしない） */
async function attachPdf(key, hasIds) {
  $("keepopen").style.display = "block";
  let note;
  if (page.pdfUrl) {
    const got = await fetchPdfBytes(page.pdfUrl);
    if (got.bytes) {
      setStatus("PDF を OpenRef へ転送中…（" + kb(got.bytes.length) + "）");
      try {
        const res = await api(
          "/capture/attachment?key=" + encodeURIComponent(key) + "&source=" + encodeURIComponent(page.pdfSource || "") + "&via=page",
          {
            method: "POST",
            headers: { "Content-Type": "application/pdf" },
            body: got.bytes,
          }
        );
        const mark = res.doi_check === "unknown" ? "・DOI 未確認" : "";
        if (res.status === "attached") return "PDF 添付 ✓（" + got.how + " fetch・" + kb(res.bytes) + mark + "）";
        if (res.status === "attached_additional") return "PDF を 2 本目として添付 ✓（既存添付と別ファイル・" + kb(res.bytes) + mark + "）";
        if (res.status === "skipped") return "PDF は既に添付済み（同一ファイル）";
        note = "転送結果 " + res.status;
      } catch (e) {
        // 409 = 別論文（connectd の同一性照合）/ 422 unverified = リンク拾いの PDF に DOI が無い → ここで止める
        //（OA 経路や層 2 に進んで成功すると「別論文だった」事実が表示から消えるため）
        if (e.status === 409 || (e.status === 422 && e.body && e.body.status === "unverified")) {
          return "書誌は保存済み。PDF は添付しませんでした — " + String(e.message || e);
        }
        note = "転送失敗（" + String(e.message || e) + "・" + kb(got.bytes.length) + "）";
      }
    } else {
      note = "ブラウザ経由で取れず（" + got.error + "）";
    }
  } else {
    // PubMed ページ等: PDF リンクが無いので即 OA 経路へ
    note = "ページに PDF リンクなし";
  }
  if (!hasIds) {
    return "PDF は取得できませんでした — " + note + "（識別子なしのため OA 経路は対象外）\n" +
      "PDF を開いた状態でもう一度 Capture するか、DL して OpenRef へドロップしてください";
  }
  setStatus(note + " → OpenRef 側で取得を試行…（PMC / Unpaywall / Crossref / DOI 着地ページ）");
  try {
    const oa = await api("/capture/fetch-oa", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ key }),
    });
    return "PDF 添付 ✓（取得元: " + oa.source + "）";
  } catch (e) {
    // 層 2（DOI ページの背景タブ）はここ 1 箇所から呼ぶ
    return await afterOaFailure(key, note, e);
  }
}

/* 識別子なし・PDF タブ: バイト列を /capture/pdf へ（本文の DOI/PMID で書誌確定）。
   {done: true} = 処理完了、{done: false, error} = PDF を取れなかったので従来の /capture/save（題名のみ）へ。
   その場合は同じ PDF を再 fetch しない（attachPdf は呼ばない） */
async function ingestPdfFlow() {
  $("keepopen").style.display = "block";
  const got = await fetchPdfBytes(page.pdfUrl || page.url);
  if (!got.bytes) {
    setStatus("PDF を取得できず（" + got.error + "）→ 題名のみで保存します…");
    return { done: false, error: got.error };
  }
  setStatus("PDF を OpenRef へ転送中…（" + kb(got.bytes.length) + "・本文から DOI/PMID を抽出して書誌を確定）");
  const qs = new URLSearchParams();
  if ($("collection").value) qs.set("collection_id", $("collection").value);
  if (page.title) qs.set("title", page.title);
  if (page.url) qs.set("url", page.url);
  const res = await api("/capture/pdf?" + qs.toString(), {
    method: "POST",
    headers: { "Content-Type": "application/pdf" },
    body: got.bytes,
  });
  const label =
    (res.first_author || "?") + (res.year ? " (" + res.year + ")" : "") +
    (res.journal ? " · " + res.journal : "");
  $("title").textContent = plain(res.title) || page.title || "";
  $("ids").textContent = res.trashed ? "（ゴミ箱内の item）" : label;
  const k = res.key;
  const why = res.reason ? "\n" + res.reason : "";
  const msgs = {
    added: "✓ 登録 " + k + " — PDF 本文の識別子で書誌を確定して添付（" + got.how + " fetch）",
    attached: "✓ 既存 " + k + " に PDF を添付（本文の識別子が既存 item と一致）",
    attached_additional: "✓ 既存 " + k + " に 2 本目として添付（既存添付と別ファイル）",
    duplicate:
      "＝ 同一 PDF が既に添付済み（" + k + (res.trashed ? "・ゴミ箱内。完全削除すれば取り込み直せます" : "") + "）" + why,
    needs_review: "？ 要確認として登録 " + k + why,
    stub: "？ 要確認として登録 " + k + "（PDF は添付済み。詳細ペインの編集で DOI/PMID を入力すると書誌が入ります）" + why,
  };
  let msg = msgs[res.status] || ("✓ " + res.status + " " + k);
  if (res.retraction) msg += "\n⚠ 撤回論文の可能性: " + res.retraction;
  setStatus(msg, res.status === "duplicate" ? "" : "done");
  return { done: true };
}

/* ================= 層 2: DOI ページを背景タブで開いて取得（実験的） ================= */

const hostOf = (u) => {
  try {
    return new URL(u).host;
  } catch (e) {
    return u;
  }
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const withTimeout = (p, ms, what) =>
  Promise.race([
    p,
    new Promise((_, rej) => setTimeout(() => rej(new Error(what + " が " + Math.round(ms / 1000) + " 秒以内に応答しません")), ms)),
  ]);

/* OA 経路も失敗した後: DOI と全サイト許可があれば背景タブ経路、無ければ案内で終える */
async function afterOaFailure(key, note, oaErr) {
  const lines = ["書誌は保存済み。PDF は取得できませんでした — " + note, "OpenRef 側: " + oaReason(oaErr)];
  const doi = savedDoi;
  if (doi && allSites) {
    try {
      return await viaDoiTab(key, doi);
    } catch (e) {
      lines.push("DOI ページ経由: " + String(e.message || e));
    }
  } else if (doi) {
    lines.push("DOI ページ経由: 全サイトへのアクセス許可が無いため未試行（chrome://extensions → OpenRef Capture → サイトへのアクセス で許可できます）");
  } else {
    lines.push("DOI ページ経由: DOI なしのため未試行");
  }
  lines.push("誌面で PDF を開いた状態でもう一度 Capture するか、DL して OpenRef へドロップしてください");
  return lines.join("\n");
}

/* base64 の PDF を /capture/attachment へ。成功なら表示用メッセージ、失敗なら throw。
   409 = connectd の同一性照合で別論文と判定（found_doi）→ 添付されない */
async function postPdf(key, b64, how, source) {
  setStatus("PDF を OpenRef へ転送中…（" + how + "）");
  const bytes = b64ToBytes(b64);
  let res;
  try {
    res = await api(
      "/capture/attachment?key=" + encodeURIComponent(key) + "&source=" + encodeURIComponent(source || "") + "&via=doi-tab",
      {
        method: "POST",
        headers: { "Content-Type": "application/pdf" },
        body: bytes,
      }
    );
  } catch (e) {
    if (e.status === 409 && e.body && e.body.found_doi) {
      throw new Error("取得した PDF は別論文（DOI " + e.body.found_doi + "）でした。意図的なら OpenRef へドロップして取り込んでください");
    }
    if (e.status === 422 && e.body && e.body.status === "unverified") {
      throw new Error(String(e.message || e));
    }
    throw new Error("転送失敗（" + String(e.message || e) + "・" + kb(bytes.length) + "）");
  }
  const mark = res.doi_check === "unknown" ? "・DOI 未確認" : "";
  if (res.status === "attached") return "PDF 添付 ✓（" + how + "・" + kb(res.bytes) + mark + "）";
  if (res.status === "attached_additional") return "PDF を 2 本目として添付 ✓（既存添付と別ファイル・" + how + "・" + kb(res.bytes) + mark + "）";
  if (res.status === "skipped") return "PDF は既に添付済み（同一ファイル）";
  return "PDF 転送の応答が不明（" + JSON.stringify(res) + "）";
}

/* タブの読込完了を待つ。complete の後も meta refresh の連鎖で再び loading になることがあるので、
   complete から少し静かな時間が続いたら解決する（中継ページのままかどうかは呼び出し側が content script に聞く）。 */
function waitForTabLoad(tabId, timeoutMs) {
  return new Promise((resolve, reject) => {
    let lastUrl = null;
    let settle = null;
    let done = false;
    const finish = (err) => {
      if (done) return;
      done = true;
      chrome.tabs.onUpdated.removeListener(onUpd);
      chrome.tabs.onRemoved.removeListener(onRem);
      clearTimeout(timer);
      clearTimeout(settle);
      if (err) reject(err);
      else resolve(lastUrl);
    };
    const onUpd = (id, info, t) => {
      if (id !== tabId) return;
      if (t && t.url) lastUrl = t.url;
      if (info.status === "loading") {
        clearTimeout(settle);
        settle = null;
      }
      if (info.status === "complete") {
        clearTimeout(settle);
        settle = setTimeout(() => finish(), 800);
      }
    };
    const onRem = (id) => {
      if (id === tabId) finish(new Error("タブが閉じられました"));
    };
    chrome.tabs.onUpdated.addListener(onUpd);
    chrome.tabs.onRemoved.addListener(onRem);
    const timer = setTimeout(
      () => finish(new Error("DOI ページの読込が " + Math.round(timeoutMs / 1000) + " 秒以内に終わりません")),
      timeoutMs
    );
    chrome.tabs
      .get(tabId)
      .then((t) => {
        if (t.url) lastUrl = t.url;
        if (t.status === "complete") onUpd(tabId, { status: "complete" }, t);
      })
      .catch(() => {});
  });
}

/* PDF バイト列（base64）をタブ内 fetch → 失敗なら service worker の fetch で取る。
   content script の fetch はページオリジンの CORS を受ける（別オリジンの配信ホスト = AIC の storage.・
   ScienceDirect の pdf.sciencedirectassets.com は `ACAO: *` でも credentials:include で拒否される）ので、
   全サイト権限を持つ SW の fetch（CORS を受けない）へ必ず fallback する。「HTML なら 1 回だけ追従」は
   タブ内 = meta refresh か同一オリジンの .pdf パスがちょうど 1 つ / SW = meta refresh のみ（規則差は background.js に明記）。
   注意: 追従先（meta refresh の飛び先 = 任意ホスト）へも user の cookie 付きで fetch する。 */
async function fetchPdfViaTabThenBg(tabId, url, injected) {
  const errors = [];
  if (injected) {
    const f = await withTimeout(sendToTab({ type: "openref-fetch-pdf", url, follow: true }, tabId), 60000, "タブ内 fetch");
    if (f.ok) return { b64: f.b64, how: "タブ内" };
    errors.push("タブ内: " + f.error);
  }
  const g = await withTimeout(sendToBg({ type: "openref-bg-fetch-pdf", url, follow: true }), 60000, "拡張 fetch");
  if (g.ok) return { b64: g.b64, how: "拡張" };
  errors.push("拡張: " + g.error);
  throw new Error(errors.join(" / "));
}

/* DOI ページを背景タブで開き、着地ページで PDF リンクを探して取得。
   失敗でも必ずタブを閉じる（popup が途中で閉じても SW が storage.session / alarms で閉じる）。
   再試行ループは作らない（中継ページの meta refresh を最大 4 回待つだけ）。
   制限: 着地が Content-Disposition: attachment の PDF だと Chrome がダウンロードに回し、タブは空のまま → 20 秒で時間切れになる。 */
async function viaDoiTab(key, doi) {
  setStatus("DOI ページを背景タブで開いて PDF を探しています…（doi.org/" + doi + "）");
  const t = await chrome.tabs.create({
    url: "https://doi.org/" + encodeURIComponent(doi).replace(/%2F/gi, "/"),
    active: false,
  });
  await sendToBg({ type: "openref-hidden-tab", tabId: t.id, open: true });
  const started = Date.now();
  try {
    let pdfUrl = null;
    let pdfSource = null; // meta | self | atypon（宣言された所在だけを受ける。link = ページ上の .pdf リンク拾いは本体とは限らない）
    let host = "";
    let refresh = null;
    let injected = false;
    let heuristicOnly = false;
    for (let i = 0; i < 4; i++) {
      const remain = DOI_TAB_TIMEOUT_MS - (Date.now() - started);
      if (remain <= 0) throw new Error("DOI ページの読込が " + Math.round(DOI_TAB_TIMEOUT_MS / 1000) + " 秒以内に終わりません");
      const url = await waitForTabLoad(t.id, remain);
      host = hostOf(url || "");
      try {
        await withTimeout(chrome.scripting.executeScript({ target: { tabId: t.id }, files: ["content.js"] }), STEP_TIMEOUT_MS, "content script の注入");
        injected = true;
      } catch (e) {
        // 注入不可（Chrome の PDF ビューア・エラーページ・chrome:// 等）。URL が PDF なら拡張 fetch で取る
        injected = false;
        if (url && /\.pdf(\?|#|$)/i.test(url)) {
          pdfUrl = url;
          pdfSource = "self";
          break;
        }
        throw new Error("着地ページ（" + host + "）に content script を注入できません（PDF ビューア / エラーページの可能性: " + String(e.message || e) + "）");
      }
      const a = await withTimeout(sendToTab({ type: "openref-analyze" }, t.id), STEP_TIMEOUT_MS, "着地ページの解析");
      if (!a.ok) throw new Error("着地ページ（" + host + "）の解析に失敗: " + a.error);
      if (a.data.pdfUrl && ["meta", "self", "atypon"].includes(a.data.pdfSource)) {
        pdfUrl = a.data.pdfUrl;
        pdfSource = a.data.pdfSource;
        break;
      }
      if (a.data.pdfUrl) heuristicOnly = true; // link 由来 = 宣言なし。使わない
      const r = await withTimeout(sendToTab({ type: "openref-refresh-url" }, t.id), STEP_TIMEOUT_MS, "中継判定");
      refresh = r.ok ? r.url : null;
      if (!refresh) break;
      // 中継ページ（meta refresh）: 飛び先の読込を待ってやり直す
      setStatus("中継ページ（" + host + "）→ 着地を待っています…");
      await sleep(700); // linkinghub.elsevier.com は "2; url=…" = 2 秒後に飛ぶ
    }
    if (!pdfUrl) {
      throw new Error(
        "着地ページ（" + host + "）に PDF の宣言（citation_pdf_url 等）がありません" +
          (heuristicOnly ? "（ページ上の .pdf リンクは本体とは限らないため使いません）" : "") +
          (refresh ? "（中継ページのまま）" : "")
      );
    }
    setStatus("PDF を取得中…（" + host + "・認証済み session）");
    const got = await fetchPdfViaTabThenBg(t.id, pdfUrl, injected);
    return await postPdf(key, got.b64, "DOI ページ経由・" + got.how + " fetch・" + host, pdfSource);
  } finally {
    try {
      await chrome.tabs.remove(t.id);
    } catch (e) { /* 既に閉じている */ }
    sendToBg({ type: "openref-hidden-tab", tabId: t.id, open: false });
  }
}

/* 全サイト許可: 層 2 に進み得る（識別子がある）ページで、未付与かつ過去に拒否していなければ、
   保存クリック（= ユーザージェスチャ）の直後に 1 度だけ求める。後段の await の後では gesture が切れて request できない。
   拒否は chrome.storage.local に記録し、以後は聞かない（chrome://extensions のサイトへのアクセスで後から許可できる）。 */
async function maybeRequestAllSites() {
  if (allSites || allSitesDeclined || !page || !(page.doi || page.pmid)) return;
  // ここまで await を挟まない（permissions.request はユーザージェスチャの直後でないと拒否される。拒否記録は init で読み済み）
  setStatus(
    "初回のみ: PDF が取れないとき DOI ページを背景タブで開いて探すため、全サイトへのアクセス許可を求めます（拒否しても書誌は保存され、以後は聞きません）"
  );
  try {
    allSites = await chrome.permissions.request(ALL_SITES);
  } catch (e) {
    allSites = false;
  }
  if (!allSites) {
    allSitesDeclined = true;
    try {
      await chrome.storage.local.set({ [DECLINED_KEY]: true });
    } catch (e) { /* noop */ }
  }
}

$("save").addEventListener("click", async () => {
  $("save").disabled = true;
  await maybeRequestAllSites();
  try {
    let pdfFetchError = null;
    if (shouldIngestPdf()) {
      const r = await ingestPdfFlow();
      if (r.done) return;
      pdfFetchError = r.error;
    }

    setStatus("書誌を確定中…（PubMed/Crossref）");
    const saved = await api("/capture/save", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        doi: page.doi,
        pmid: page.pmid,
        title: page.title,
        url: page.url,
        collection_id: $("collection").value ? Number($("collection").value) : null,
      }),
    });
    const label =
      (saved.first_author || "?") + (saved.year ? " (" + saved.year + ")" : "") +
      (saved.journal ? " · " + saved.journal : "");
    $("title").textContent = plain(saved.title) || page.title || "";
    $("ids").textContent = label + (saved.status === "existing" ? "（既存）" : "（新規登録）");

    let pdfMsg = "";
    // 層 2 用の DOI: 応答に doi キーがあれば resolver の値（null でも）を正とし、ページ抽出の DOI へは fallback しない
    savedDoi = "doi" in saved ? saved.doi || null : page.doi || null;
    const hasIds = !!(saved.doi || page.doi || page.pmid);
    // 識別子なしページでは、PDF タブ自体か citation_pdf_url 由来の PDF だけを本体とみなす（リンク拾いは添付もしない）
    const pdfTrusted = !!(page.isPdf || (page.pdfUrl && page.pdfUrl === page.url) || page.pdfSource === "meta");
    if (saved.has_pdf) {
      pdfMsg = "PDF は既に添付済み";
    } else if (pdfFetchError !== null) {
      // PDF タブだったが取れなかった（再 fetch はしない）
      pdfMsg = "PDF は取得できませんでした（" + pdfFetchError + "）。DL して OpenRef へドロップしてください";
    } else if (!hasIds && !pdfTrusted) {
      pdfMsg = page.pdfUrl
        ? "ページ上の PDF リンクは論文本体とは限らないため添付しません（識別子なし）。PDF を開いた状態で Capture してください"
        : "PDF リンクなし（識別子なしのため OA 経路は対象外）";
    } else {
      pdfMsg = await attachPdf(saved.key, hasIds);
    }
    let msg = "✓ " + (saved.status === "existing" ? "既存 " : "登録 ") + saved.key + " — " + pdfMsg;
    if (saved.retraction) msg += "\n⚠ 撤回論文の可能性: " + saved.retraction;
    setStatus(msg, "done");
  } catch (e) {
    setStatus("✗ " + String(e.message || e), "err");
    $("save").disabled = false;
  } finally {
    $("keepopen").style.display = "none";
  }
});

init();
