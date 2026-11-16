/* OpenRef Capture — popup。受け持つのは 3 つだけ:
   ①connectd の接続確認とページの解析（content script 注入 → 識別子/PDF URL。注入できなければ URL だけで解析する。
     J-STAGE の PDF タブは記事ページの citation_doi を popup が引く = fillFromJstageArticle）
   ②全サイト許可の要求（層 2 = DOI ページを背景タブで開く経路に要る。保存クリックの直後でないと求められない）
   ③表示。保存の工程は service worker（job.js）が進める: 保存ボタンで openref-save を送り、以後は工程の記録
     （chrome.storage.session の 1 件）を表示する。popup を閉じても工程は続き、開き直すと同じタブ・同じページなら続き
     （進み具合・結果・分類・確定した DOI）を出す。別のタブの保存が走っている間は、そう出す（保存は service worker が断る）
   ボタン 1 つ（offerAction）: 分類が challenge / stuck / login で終わった保存に、全サイト許可があれば「窓で開く」（確認やログインを
   利用者が窓で済ませると、service worker が前の保存の item に取り込み直す。前置きがあれば前置き経由の窓）。全サイト許可が無ければ、
   大学プロキシの前置き（実験的・設定 = options.html）があって分類が login / stuck のときだけ「プロキシ経由で開く」（新しいタブ） */
"use strict";

const { BASE, ALL_SITES, DECLINED_KEY, PROXY_KEY, JOB_KEY, RETRY_CLASSES, withTimeout } = OpenRefShared;
const JSTAGE_ARTICLE_TIMEOUT_MS = 8000; // J-STAGE の PDF タブから記事ページを引く上限（popup を開いたときに 1 回だけ）
let allSites = false; // optional_host_permissions（全サイト）が付与済みか
let allSitesDeclined = false; // 過去に拒否した記録（init で読む。クリック時に await を挟まないため）
let proxyTemplate = null; // 大学プロキシの前置き。受けられる https:// の URL だけ（init で読む）。null = 使わない（今までどおり）
let failClass = null; // 表示中の保存の失敗の分類（工程の記録から。「プロキシ経由で開く」を出すかに使う）
let savedDoi = null; // 表示中の保存で確定した DOI（工程の記録から。「プロキシ経由で開く」の行き先に使う）
let shownId = null; // 表示中の（このタブ・このページの）保存の記録の id（確認の窓のボタンが送る）
let offerMode = null; // ボタンの今の役: "window" = 確認の窓・"tab" = プロキシ経由で新しいタブ・null = 出していない
let tab = null;
let page = null; // {doi, pmid, pdfUrl, pdfSource, title, url, isPdf, landingUrl, evidence}
let contentScriptOk = false;
let followed = false; // この popup で工程の走行を見た（終わった後の保存ボタン: 成功なら押せないまま = 二重保存を避ける）
let otherNote = false; // 状態行に「別のタブの保存が進行中」を出している
let runningId = null; // 走っている保存の記録の id（「中断」が送る）
let rendered = false; // init の描画が済んだ（それまでの記録の変化は latest に持つだけ。描画のときに当てる）
let documentId = null; // content script を入れた文書（executeScript の結果・Chrome 106 以降）。工程はページ内 fetch をこの文書に固定する
let latest; // 最新の工程の記録（undefined = まだ知らない・null = 無い）

const $ = (id) => document.getElementById(id);
/* 状態行。結果の詳細（畳んだ段ごとの理由）は消す（出すのは showResult だけ） */
const setStatus = (msg, cls) => {
  $("status").textContent = msg;
  $("status").className = cls || "";
  $("detail").style.display = "none";
  $("note").textContent = "";
  $("note").style.display = "none";
};
/* 結果の畳む部分: 記録の line が head + 改行で始まるときだけ、その後ろ。head の無い記録（前の版が残した記録・走行中・中断）は null */
const foldOf = (rec) =>
  typeof rec.head === "string" && typeof rec.line === "string" && rec.line.startsWith(rec.head + "\n") ? rec.line.slice(rec.head.length + 1) : null;
/* 別のタブ（またはこのタブの前のページ）の結果の 1 行の中身 [印, 残り]: 印 = 1 行目（書誌 ✓ ・ PDF …）・残り = 2 行目（登録 / 既存 と key・
   失敗の文）。1 行目に括弧の理由が無いときだけ、畳んだ詳細の最初の行（PDF が付かなかった最初の理由）も足す（理由を繰り返さない・
   理由の読めない 1 行を作らない）。残りは NOTE_REST 字まで（popup の幅で 3 行ほど）。head の無い記録（前の版・中断）は line の 1 行目だけ */
const NOTE_REST = 50;
function noteParts(rec) {
  if (typeof rec.head !== "string") return [String(rec.line || "").split("\n")[0], ""];
  const [mark, second = ""] = rec.head.split("\n");
  const fold = foldOf(rec);
  const why = mark.includes("（") || fold === null ? "" : fold.split("\n")[0].replace(/^書誌は保存済み。/, "");
  const rest = [second, why].filter(Boolean).join(" / ");
  return [mark, rest.length > NOTE_REST ? rest.slice(0, NOTE_REST) + "…" : rest];
}
/* 別のタブの結果の 1 行を、状態行と詳細の後ろ（#note）に出す = 自分の詳細の上に挟まらない。色と太字は印（1 行目）だけ */
function showNote(rec) {
  const [mark, rest] = noteParts(rec);
  const n = $("note");
  n.textContent = "";
  n.append("前の保存（" + (rec.title || rec.tabUrl || "") + "）: ");
  const m = document.createElement("span");
  m.className = "mark" + (rec.cls ? " " + rec.cls : "");
  m.textContent = mark;
  n.append(m);
  if (rest) n.append(" / " + rest);
  n.style.display = "block";
}
let foldFor = null; // 詳細を出している結果（記録の id と終わった時刻）。同じ結果を描き直すときは、開いた詳細を閉じない
/* このタブの保存の記録を状態行に出す: 結果なら head（1 行目 + 登録 / 既存 + 注記 + 次の一手）と、畳んだ詳細 */
function showResult(rec) {
  const fold = foldOf(rec);
  setStatus(fold === null ? rec.line || "" : rec.head, rec.cls);
  if (fold === null) return;
  $("detailtext").textContent = fold;
  const at = rec.id + ":" + rec.endedAt;
  if (foldFor !== at) $("detail").open = false; // 新しい結果は閉じて出す
  foldFor = at;
  $("detail").style.display = "block";
}

async function api(path, opts) {
  const r = await fetch(BASE + path, opts);
  const body = await r.json().catch(() => ({}));
  if (!r.ok) {
    const err = new Error(body.error || "HTTP " + r.status);
    err.status = r.status;
    err.body = body;
    throw err;
  }
  return body;
}

/* callback 版の sendMessage を Promise にする: lastError → {ok:false, error}・応答なし → "no response" */
function reply(send) {
  return new Promise((resolve) => {
    send((res) => {
      if (chrome.runtime.lastError) resolve({ ok: false, error: chrome.runtime.lastError.message });
      else resolve(res || { ok: false, error: "no response" });
    });
  });
}
function sendToTab(msg, tabId) {
  return reply((cb) => chrome.tabs.sendMessage(tabId == null ? tab.id : tabId, msg, cb));
}
function sendToBg(msg) {
  return reply((cb) => chrome.runtime.sendMessage(msg, cb));
}

/* EZproxy のホスト名方式を出版社のホスト名に戻した URL（規則は pdfbytes.js の plainUrl・前置きは init で読んだ物） */
const plainUrl = (u) => OpenRefShared.plainUrl(u, proxyTemplate);

/* content script を注入できない場合（chrome:// 等・PDF ビューアに入れられない Chrome）の URL-only 解析。
   識別子の規則（PubMed・/doi/…・医書.jp）と「PDF の URL らしいか」は content.js と同じ 1 つ（pdfbytes.js の urlIds・looksLikePdfUrl）。
   ここだけのサイト規則は ScienceDirect の pdfft・医書.jp の PDF ビューア・J-STAGE の PDF（タブ自体が PDF とみなす）。
   ホスト名は plainUrl の後で見る。pdfUrl・url・landingUrl はタブの URL のまま */
function urlOnlyAnalyze(url) {
  const out = { doi: null, pmid: null, pdfUrl: null, pdfSource: null, title: null, url, isPdf: false, landingUrl: null };
  const ids = OpenRefShared.urlIds(url, proxyTemplate);
  out.pmid = ids.pmid;
  out.doi = ids.doi;
  // pdfft はページ上のリンク拾いには入れない（ScienceDirect の別の論文の pdfft を拾う）= タブの URL だけの規則
  if (OpenRefShared.looksLikePdfUrl(url) || /\/pdfft\b/.test(url) || ids.isho === "pdf") {
    out.pdfUrl = url;
    out.pdfSource = "self";
    out.isPdf = true;
  }
  // J-STAGE の PDF（…/_pdf/-char/ja は上の .pdf の判定に当たらない）: タブ自体が PDF。DOI は記事ページから引く（fillFromJstageArticle）
  const jstage = OpenRefShared.jstageArticleUrl(url, proxyTemplate);
  if (jstage) {
    out.pdfUrl = url;
    out.pdfSource = "self";
    out.isPdf = true;
    out.landingUrl = jstage;
  }
  return out;
}

/* J-STAGE の PDF の同一性の比較用: origin + パス（末尾の /-char/<言語> と / を外す・クエリと断片は見ない）。
   記事ページの citation_pdf_url は …/_pdf（-char なし）、タブは …/_pdf/-char/ja のことが多い。
   ホストは plainUrl の後（プロキシの中のタブと、プロキシが書き換えない citation_pdf_url を同じ J-STAGE の PDF と見る） */
function jstagePdfKey(u, base) {
  if (!u) return null;
  try {
    const x = new URL(plainUrl(new URL(u, base).href));
    return x.origin + x.pathname.replace(/\/-char\/[a-z]+\/?$/i, "").replace(/\/$/, "");
  } catch (e) {
    return null;
  }
}

/* J-STAGE の記事ページを 1 回だけ取り、{doi, title} を返す（合わなければ throw）。
   記事ページの citation_pdf_url がこのタブの PDF と同じ所（jstagePdfKey）を指すときだけ採る
   （取り違えた記事ページの DOI で、別の論文として登録しないため） */
async function readJstageArticle(pdfUrl, landingUrl) {
  const r = await fetch(landingUrl, { credentials: "include" });
  if (!r.ok) throw new Error("HTTP " + r.status);
  const doc = new DOMParser().parseFromString(await r.text(), "text/html");
  const meta = (name) => {
    const el = doc.querySelector('meta[name="' + name + '" i]');
    return el ? (el.getAttribute("content") || "").trim() : "";
  };
  const doi = OpenRefShared.cleanDoi(meta("citation_doi"));
  if (!doi) throw new Error("citation_doi なし");
  const declared = jstagePdfKey(meta("citation_pdf_url"), r.url || landingUrl);
  if (!declared || declared !== jstagePdfKey(pdfUrl)) throw new Error("記事ページの PDF がこのタブと別");
  return { doi, title: meta("citation_title") || null };
}

/* J-STAGE の PDF タブ（DOI は記事ページの citation_doi にだけある）:
   popup が記事ページを引き、DOI と題名を p に入れる。popup は chrome-extension:// なので、この fetch はオリジンをまたぐ。
   通るのは activeTab がタブのオリジンに一時的なホスト権限をくれるから（background.js の PDF タブの fetch と同じ前提。権限は増やさない）。
   拒まれても /capture/pdf の経路に落ちるだけ。
   以後は識別子の経路（/capture/save → /capture/attachment source=self）。
   取れない・合わないときは p をそのまま（/capture/pdf の経路）にして、状態行に出す短い文を返す（何もしなければ ""） */
async function fillFromJstageArticle(p) {
  if (p.doi || p.pmid || !p.landingUrl) return "";
  setStatus("J-STAGE の記事ページで DOI を確認中…");
  try {
    const got = await withTimeout(readJstageArticle(p.url, p.landingUrl), JSTAGE_ARTICLE_TIMEOUT_MS, "J-STAGE の記事ページ");
    p.doi = got.doi;
    if (got.title) p.title = got.title;
    return "";
  } catch (e) {
    return "J-STAGE の記事ページで DOI を確かめられず（" + String(e.message || e) + "）→ PDF 本文から探します";
  }
}

/* タブ自体が PDF・/capture/pdf の経路に回すか（規則は pdfbytes.js。保存の工程と同じ 1 つ） */
const isPdfTab = () => OpenRefShared.isPdfTab(page);
const shouldIngestPdf = () => OpenRefShared.shouldIngestPdf(page, contentScriptOk);
const canSave = () => !!(page && (page.pmid || page.doi || page.title || shouldIngestPdf()));

function renderPage() {
  $("title").textContent = page.title || "（タイトル不明）";
  const bits = [];
  if (page.pmid) bits.push("PMID " + page.pmid);
  if (page.doi) bits.push("DOI " + page.doi);
  let badge;
  if (isPdfTab()) badge = '<span class="badge ok">PDF タブ</span>';
  else if (page.pmcUrl) badge = '<span class="badge ok">PMC 本文あり</span>';
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
  $("save").disabled = !canSave();
  focusSaveIfIdle();
}

/* 保存ボタンが押せるようになった時点で、そこへフォーカスを置く（ショートカットで開いて Enter で保存）。
   user がほかの入力（フォルダの選択など）を操作中ならフォーカスを奪わない。
   二重保存は click の直後の disabled が防ぐ（押せないボタンに Enter は効かない） */
function focusSaveIfIdle() {
  const save = $("save");
  if (save.disabled) return;
  const a = document.activeElement;
  if (a && a !== document.body && a !== document.documentElement) return;
  save.focus();
}

/* 記録がこの popup のタブ・ページの物か（断片 # は見ない。タブが別のページへ移っていたら別の保存とみなす。規則は pdfbytes.js の samePage） */
const isMine = (rec) => !!tab && rec.tabId === tab.id && OpenRefShared.samePage(rec.tabUrl, tab.url);

/* 結果を表示した: バッジを消し、記録に「見た」の印を付ける（別のタブの popup が同じ結果をもう一度出さない） */
function markShown(rec) {
  try {
    Promise.resolve(chrome.action.setBadgeText({ text: "" })).catch(() => {});
  } catch (e) { /* noop */ }
  if (!rec.seen) sendToBg({ type: "openref-seen", id: rec.id });
}

/* 「中断」は、走っている保存の記録があるときだけ出す（このタブの物でも、別のタブの物でも） */
function showAbort(rec) {
  const running = !!rec && rec.state === "running";
  $("abort").style.display = running ? "block" : "none";
  $("abort").disabled = false;
  runningId = running ? rec.id : null;
}

/* 工程の記録を表示する（開いたときと、記録が変わるたび）。
   このタブの物: 進み具合・結果・分類・確定した DOI。走行中は保存ボタンを押せない。終わったら、この popup で走行を見た成功は
   押せないまま（二重保存を避ける）・失敗と中断と開き直しは押せる。結果を見たのでバッジを消す。
   別のタブ（またはこのタブの前のページ）の物: 走行中は一言出す（保存を押せば service worker が断る・「中断」で止められる）。
   終わった物は、この popup が走行中を見ていたか、まだ誰も見ていなければ、題名と結果を 1 行で出す */
function showRecord(rec) {
  if (!rec || !tab || !page || !rendered) return;
  showAbort(rec);
  if (!isMine(rec)) {
    hideAction(); // 記録が別のタブ・別のページの保存に替わった = 前の結果のボタンは出したままにしない
    if (rec.state === "running") {
      setStatus("別のタブ（またはこのタブの前のページ）の保存が進行中です。終わってから保存するか、中断してください", "");
      otherNote = true;
    } else if (otherNote || rec.seen === false) {
      // 進行中の一言は消す。この popup が出している文（解析の文・自分の結果と詳細）は残す
      if (otherNote) setStatus("", "");
      showNote(rec);
      otherNote = false;
      markShown(rec);
    }
    return;
  }
  otherNote = false;
  if (rec.title) $("title").textContent = rec.title;
  if (rec.ids) $("ids").textContent = rec.ids;
  showResult(rec);
  failClass = rec.failClass || null;
  savedDoi = rec.savedDoi || null;
  shownId = rec.id;
  if (rec.state === "running") {
    followed = true;
    $("save").disabled = true;
    hideAction();
    return;
  }
  offerAction(rec);
  $("save").disabled = (followed && rec.state === "done") || !canSave();
  markShown(rec);
  focusSaveIfIdle();
}

async function init() {
  // 工程の記録: 変化は最初から受ける（解析の間に工程が進んだ・終わった分を取りこぼさない）。今の 1 件は service worker に聞く
  // （止まっていた service worker を起こし、残った「走行中」を「中断」に直してから答える）。答えより後の変化が先に届いていれば、そちらを使う
  chrome.storage.onChanged.addListener((changes, area) => {
    if (area !== "session" || !changes[JOB_KEY]) return;
    latest = changes[JOB_KEY].newValue || null;
    showRecord(latest); // 描画の前（rendered が偽の間）は何もしない。描画のときに latest を当てる
  });
  const job = sendToBg({ type: "openref-job" }).then((st) => {
    if (latest === undefined) latest = st.ok ? st.record : null;
  });
  try {
    allSites = await chrome.permissions.contains(ALL_SITES);
  } catch (e) {
    allSites = false;
  }
  try {
    const v = await chrome.storage.local.get([DECLINED_KEY, PROXY_KEY]);
    allSitesDeclined = !!v[DECLINED_KEY];
    proxyTemplate = OpenRefShared.usableProxy(v[PROXY_KEY]);
  } catch (e) {
    allSitesDeclined = false;
  }

  // 接続確認（connectd は DB を読めないとき items: null と error を返す = 接続はできている）
  let dbNote = "";
  try {
    const s = await api("/capture/status");
    $("conn").textContent = s.items == null ? "✓ OpenRef（件数不明）" : "✓ OpenRef（" + s.items + " 件）";
    if (s.error) dbNote = "OpenRef には接続できましたが、蔵書を読めません: " + s.error;
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
    const inj = await chrome.scripting.executeScript({ target: { tabId: tab.id }, files: ["pdfbytes.js", "content.js"] });
    documentId = (Array.isArray(inj) && inj[0] && typeof inj[0].documentId === "string" && inj[0].documentId) || null;
    const res = await sendToTab({ type: "openref-analyze", proxy: proxyTemplate });
    if (res.ok) {
      page = res.data;
      contentScriptOk = true;
    }
  } catch (e) { /* PDF ビューア等（注入不可）*/ }
  if (!page) page = urlOnlyAnalyze(tab.url || "");
  // 保存ボタンは描画まで押せない（disabled）ので、記事ページを引く間に /capture/pdf の経路へ進むことは無い
  const note = await fillFromJstageArticle(page);
  if (!page.title && tab.title) page.title = tab.title;
  await job;
  renderPage();
  setStatus([dbNote, note].filter(Boolean).join("\n"), dbNote ? "err" : "");
  // 描画と同じ同期の区間で記録を当てる（走行中なら、押せる状態を挟まずに押せないボタンにする）。
  // 描画の前に届いた変化はここで初めて表示する（先に出して seen にすると、この直前の setStatus が消してしまう）
  rendered = true;
  showRecord(latest);
}

/* 分類が login / stuck で前置きの設定があるときだけ「プロキシ経由で開く」を出す（自動では開き直さない）。
   開くのは、ページが宣言した PDF の URL（ページ上のリンク拾い = link は本体とは限らないので使わない）→ DOI ページ → ページの URL の順。
   新しいタブ（前面）で開き、user がそこでログインしてから、そのタブで Capture する。
   ページ自体がプロキシの中なら、行き先は出版社の URL に戻してから前置きを付ける（前置きを二重に付けない） */
function proxyTarget() {
  if (OpenRefShared.declaredPdf(page)) return page.pdfUrl;
  const doi = savedDoi || page.doi;
  return doi ? OpenRefShared.doiUrl(doi) : page.url;
}

/* 確認の窓を出せる記録か: 終わった保存で、分類が challenge / stuck / login、service worker がやり直しの材料（retry）を残し、
   この popup が全サイト許可を確かめた。開く先と材料は service worker が記録から取る（ここでは組み立てない） */
/* シークレットのタブは出さない（窓は通常の窓で開き、cookie が分かれるので確認やログインが引き継がれない） */
const windowOffer = (rec) => !!rec && rec.state === "done" && allSites && !!rec.retry && RETRY_CLASSES.includes(rec.failClass) && !(tab && tab.incognito);
const WINDOW_TITLE = "新しい窓でこのサイトを開きます。確認やログインはその窓で自分で済ませてください。" +
  "PDF が開けばそのまま取り込み、開かなければ窓を閉じたときに、この論文に 1 回だけ取り込みを試します（5 分で窓を閉じます）";
const WINDOW_HINT = "済んだら窓を閉じてください（PDF が開けばそのまま取り込みます）";
const TAB_TITLE = "設定した大学プロキシの前置きで新しいタブを開きます。ログインしてから、そのタブで Capture してください";

function hideAction() {
  $("viaproxy").style.display = "none";
  $("winhint").style.display = "none";
  offerMode = null;
}
/* 結果の下のボタン 1 つ: 確認の窓（全サイト許可あり）か、プロキシ経由で新しいタブ（全サイト許可なし）か、出さない */
function offerAction(rec) {
  const btn = $("viaproxy");
  if (windowOffer(rec)) {
    offerMode = "window";
    // 前置きを付けて開くのは、宣言の PDF と DOI ページの段だけ（PMC の段は前置きを付けない = service worker の windowUrl と同じ）
    const viaProxy = !!proxyTemplate && rec.retry.stage !== "pmc";
    btn.textContent = viaProxy ? "プロキシ経由の窓でログインする（済めば自動で取り込み）" : "窓で開いて確認・ログインする（済めば自動で取り込み）";
    btn.title = WINDOW_TITLE;
    $("winhint").textContent = WINDOW_HINT;
    $("winhint").style.display = "block";
  } else if (rec && rec.state === "done" && proxyTemplate && (failClass === "login" || failClass === "stuck")) {
    offerMode = "tab";
    btn.textContent = "プロキシ経由で開く";
    btn.title = TAB_TITLE;
  } else {
    hideAction();
    return;
  }
  btn.style.display = "block";
}
/* popup が知る画面の大きさ（窓を画面の 8 割で開くため。service worker には画面が無い） */
function screenSize() {
  try {
    return typeof screen !== "undefined" && screen ? { w: screen.availWidth, h: screen.availHeight } : null;
  } catch (e) {
    return null;
  }
}
$("viaproxy").addEventListener("click", async () => {
  if (offerMode === "window") {
    const id = shownId;
    if (!id) return;
    // 押すのは 1 回だけ（やり直しはボタン 1 回につき 1 回。結果が出ればまた出る）。窓が前面に出ると popup は閉じる
    hideAction();
    $("save").disabled = true;
    followed = true;
    const r = await sendToBg({ type: "openref-retry", id, size: screenSize() });
    if (!r.ok) {
      setStatus("✗ " + (r.error || "窓を開けません"), "err");
      $("save").disabled = !canSave();
    }
    return;
  }
  if (offerMode !== "tab") return;
  const url = OpenRefShared.proxied(proxyTemplate, plainUrl(proxyTarget()));
  if (url) chrome.tabs.create({ url, active: true });
});

/* 全サイト許可: 層 2 に進み得る（識別子がある）ページで、未付与かつ過去に拒否していなければ、
   保存クリック（= ユーザージェスチャ）の直後に 1 度だけ求める。後段の await の後では gesture が切れて request できない。
   拒否は chrome.storage.local に記録し、以後は聞かない（設定のページ = options.html のスイッチで後から許可・解除できる）。
   工程は全サイト許可を自分で確かめ直す（ここでの結果は渡さない） */
async function maybeRequestAllSites() {
  if (allSites || allSitesDeclined || !(page.doi || page.pmid)) return;
  // ここまで await を挟まない（permissions.request はユーザージェスチャの直後でないと拒否される。拒否記録は init で読み済み）
  setStatus(
    "初回のみ: PDF が取れないとき DOI ページを背景タブで開いて探すため、全サイトへのアクセス許可を求めます（拒否しても書誌は保存され、以後は聞きません。拡張の設定でいつでも変えられます）"
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

/* 中断: 走っている保存（このタブでも別のタブでも）を service worker が止める。以後の表示は記録の変化（showRecord）。
   止めた後はすぐ次の保存ができる */
$("abort").addEventListener("click", async () => {
  if (!runningId) return;
  $("abort").disabled = true;
  const r = await sendToBg({ type: "openref-abort", id: runningId });
  if (!r.ok) {
    setStatus("✗ 中断できません: " + (r.error || "no response"), "err");
    $("abort").disabled = false;
  }
});

/* 保存: 工程を service worker に渡す（以後の表示は showRecord）。断られた（別の保存が走行中）・送れないときは理由を出して押せる状態に戻す */
$("save").addEventListener("click", async () => {
  $("save").disabled = true;
  failClass = null;
  hideAction();
  await maybeRequestAllSites();
  followed = true;
  const r = await sendToBg({
    type: "openref-save",
    input: { tabId: tab.id, tabUrl: tab.url || "", documentId, page, contentScriptOk, collectionId: $("collection").value, proxy: proxyTemplate },
  });
  if (!r.ok) {
    setStatus("✗ " + (r.error || "保存を始められません"), "err");
    $("save").disabled = false;
  }
});

init();
