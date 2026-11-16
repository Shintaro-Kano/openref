/* OpenRef Capture — 保存の工程（service worker で動く。background.js が importScripts("pdfbytes.js", "job.js") で読む）。
   popup は解析・全サイト許可の要求・表示だけを受け持ち、保存ボタンで openref-save を送る。以後の工程はここで進むので、
   popup を閉じても止まらない。流れ:
   ①識別子あり: /capture/save（書誌は PubMed/Crossref で確定）→ PDF はページ内 fetch（利用者のタブの content script）→
     拡張 fetch → /capture/attachment（出どころ source= と冒頭 2 頁の DOI 照合つき）。PDF リンクが無い（PubMed 等）/ 取れない →
     OpenRef 側の連鎖（/capture/fetch-oa = PMC AWS Open Data → Unpaywall → Crossref link → doi.org 着地ページ）→ それも駄目なら③
   ②識別子なし・PDF タブ（or citation_pdf_url あり）: PDF のバイト列を取って /capture/pdf へ。connectd が本文から
     DOI/PMID を抽出 → 書誌確定 → 既存 item に添付 or 新規登録（識別子が無ければ要確認 stub）。識別子も PDF も無ければ
     /capture/save にページの書誌メタ（evidence = 照合の証拠だけ）を添える（結べなければ題名のみ + 要確認。捏造しない）
   ③層 2（実験的・全サイト許可が要る）: DOI ページを背景タブで開いて取得（viaDoiTab）。
   識別子のある経路では、宣言された PDF の URL が HTML（中継ページ）を返したら、その URL を背景タブで開いてサイト自身の処理
   （PMC の proof-of-work・ScienceDirect の pdfft → 署名付き URL 等）を走らせてから取り直す（fetchPdfViaNavigation）
   ④大学プロキシの前置き（実験的）: 層 2 の DOI ページを前置き経由で開く・ホスト名で分岐する規則は EZproxy のホスト名方式を戻して見る
   ⑤確認の窓（全サイト許可が要る）: 分類が challenge / stuck / login で終わった保存について、利用者が popup のボタンを押したときだけ、
     止まった段の URL を窓で開く。確認やログインは利用者がその窓で済ませる（拡張は窓のページを操作しない）。PDF が窓に届くか、
     利用者が窓を閉じたら、前の保存の item に 1 回だけ取り込みを試す（startRetry・runRetry）

   工程の状態（タブ・解析の結果・確定した DOI・失敗の分類 など）は工程ごとのオブジェクト job に持ち、引数で渡す。大域には置かない
   （上限で「中断」と記録しても走っている処理は止まらない。大域のままだと次の保存が値を上書きし、前の工程が新しい論文の PDF を
   前の item に送りうる）。表示は chrome.storage.session の記録 1 件（OpenRefShared.JOB_KEY）を通す: 書くのはこのファイルだけ・
   popup は読むだけ。connectd へ書き込む要求の直前に、工程がまだ有効か（中断・追い越しが無いか）を確かめ、無効なら送らない。
   同時に走らせるのは 1 件だけ。
   ここに置く名前は background.js の名前とぶつけない（同じ global に読み込まれる。const の重複は読み込みの失敗になる）。
   background.js の bgFetchPdf（拡張 fetch）・hiddenTab（背景タブの後始末の登録）を直接呼ぶ。 */
"use strict";

const { BASE, JOB_KEY, JOB_LIMIT_MS, RETRY_CLASSES, withTimeout } = OpenRefShared;
const DOI_TAB_TIMEOUT_MS = 20000;
const STEP_TIMEOUT_MS = 10000; // executeScript / sendMessage 1 回あたり
const NAV_QUIET_MS = 10000; // PDF タブが HTML のまま次へ移らなければ「中継ではない（ログイン・購入頁）」とみなす
const RELAY_WAIT_MS = 8000; // 識別子の無い着地ページ（ブラウザ確認・JS の転送）が次へ移るのを待つ上限
/* 延命: service worker は 30 秒の無活動で止まる（connectd の応答を待つ間も）。拡張 API の呼び出しは無活動のタイマーを戻す
   （Chrome 110 以降）ので、工程が走っている間だけ 20 秒ごとに 1 回呼ぶ。止まる条件の 1 つ「fetch の応答が 30 秒届かない」も、
   この延命があれば 45 秒・75 秒の応答を待てた（Chrome for Testing 156 での実測）
   JOB_LIMIT（OpenRefShared.JOB_LIMIT_MS = 15 分）の根拠: 拡張の側の上限を最も長い経路で足すと、論文ページの PDF
   （ページ内 fetch 60 秒 + 拡張 fetch 60 秒 + PDF タブ = DOI_TAB_TIMEOUT 20 秒 + 最後の取り直しの「タブ内 60 秒 + 拡張 60 秒」
   = 約 260 秒）+ 添付（API_TIMEOUT 120 秒）+ PMC 本文経由（着地の読込 20 秒 + 1 段の注入・解析・中継判定 各 10 秒 +
   取り直し 120 秒 + PDF タブ 140 秒 = 約 310 秒）+ /capture/fetch-oa（300 秒）+ DOI ページ経由（約 310 秒）= 約 1300 秒。
   すべてが上限まで待つことは無いとみなして 15 分にした（超えたら中断と記録する。利用者は popup の「中断」でいつでも止められる） */
const KEEPALIVE_MS = 20000;
/* 上限の無かった待ちの時間制限。ページ内 fetch・拡張 fetch は背景タブの取り直し（fetchPdfViaTabThenBg）と同じ 60 秒。
   connectd への要求はパスごと（実測で 75 秒の応答を待てた = それより十分長く）: 既定 120 秒（/capture/save・/capture/pdf は
   書誌の確定で PubMed / Crossref へ 30 秒上限の要求を数回・/capture/attachment は手元の処理だけ）。/capture/fetch-oa は
   connectd の連鎖（PMC・Unpaywall・Crossref・doi.org の 4 段。要求 1 回の上限は 60 秒だが、転送の追従・候補の数で回数が決まらない =
   最悪値は 15 分を超える）なので、4 段が上限の要求を 1 回ずつ待つ 240 秒に 60 秒を足して 300 秒。時間切れは失敗として記録の行と
   分類（timeout）に出す。connectd 側では処理が続くことがある（後から書誌が登録される・PDF が付くことがある）ので、文でそう言う。
   connectd が時間切れになったら、その工程は先の経路へ進まない（応えない connectd に次の要求を重ねない・後から付く PDF と合わせて
   2 本付かないように）。出版社への fetch の時間切れは次の経路へ進む。
   最初の取得（論文ページの PDF のページ内 fetch と拡張 fetch）だけは 180 秒: 大きな PDF（数十 MB）を遅い回線で取る時間を残す
   （固まったら popup の「中断」で止められる） */
const FETCH_TIMEOUT_MS = 60000;
const FIRST_FETCH_TIMEOUT_MS = 180000;
const API_TIMEOUT_DEFAULT_MS = 120000;
const API_TIMEOUT_MS = { "/capture/fetch-oa": 300000 };
const JOB_ALARM = "openref-job-limit"; // 工程の上限の alarm（service worker が止まっていても起こして記録を中断に直す）

const kb = (n) => Math.round(n / 1024) + " KB";
/* 表示用: 保存 CSL の題名は Crossref のマークアップ（<i> <sup> <sub> <scp>）込みのまま（citeproc の書式用）なので、表示では剥がす */
const plain = (t) => String(t || "").replace(/<[^>]+>/g, "").replace(/&amp;/g, "&").replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/\s+/g, " ").trim();

async function api(path, opts) {
  const p = path.split("?")[0];
  const ms = API_TIMEOUT_MS[p] || API_TIMEOUT_DEFAULT_MS;
  const ctl = new AbortController();
  const timer = setTimeout(() => ctl.abort(), ms);
  try {
    let r = null;
    let body = {};
    try {
      r = await fetch(BASE + path, Object.assign({}, opts, { signal: ctl.signal }));
      body = await r.json().catch(() => ({}));
    } catch (e) {
      if (!ctl.signal.aborted) throw e;
    }
    if (ctl.signal.aborted) {
      const later = p === "/capture/save" ? "書誌が後から登録されることがあります" : "PDF が後から付くことがあります";
      throw Object.assign(kinded(new Error("OpenRef（connectd）の " + p + " が " + Math.round(ms / 1000) +
        " 秒以内に応答しません（OpenRef 側では処理が続いていることがあり、" + later + "）"), "timeout"), { connectdTimeout: true, path: p });
    }
    if (!r.ok) {
      const err = new Error(body.error || "HTTP " + r.status);
      err.status = r.status; // 409 = 別論文（found_doi）等を呼び手が見分けられるように
      err.body = body;
      throw err;
    }
    return body;
  } finally {
    clearTimeout(timer);
  }
}

/* ================= 工程の記録（chrome.storage.session の 1 件）=================
   {id, tabId, tabUrl, startedAt, state: running | done | error | aborted, line（進み具合 / 結果の文）, head（結果の畳まずに見せる部分 = line の先頭）, cls（"" | done | warn | err）,
    title, ids（保存した item の題名と「著者 (年) · 誌名」）, failClass（失敗の分類）, savedDoi（確定した DOI）, endedAt}
   読み書きは 1 本の直列キューで行う（進み具合の書き込み・有効性の確かめ・開始・中断が交差しない = 単一の書き手） */
let recordChain = Promise.resolve();
function inOrder(fn) {
  const run = recordChain.then(fn);
  recordChain = run.catch(() => {});
  return run;
}
async function loadRecord() {
  try {
    const v = await chrome.storage.session.get(JOB_KEY);
    return v[JOB_KEY] || null;
  } catch (e) {
    return null;
  }
}
/* 書けたら true。途中の書き込みに失敗しても工程は進める（表示が遅れるだけ）。開始の書き込みだけは確かめる（startJob） */
async function saveRecord(rec) {
  try {
    await chrome.storage.session.set({ [JOB_KEY]: rec });
    return true;
  } catch (e) {
    return false;
  }
}
/* この工程がまだ記録の持ち主か: 中断していない・記録が自分の id のまま走行中（次の保存に追い越されていない） */
const isCurrent = (job, rec) => !job.cancelled && !!rec && rec.id === job.id && rec.state === "running";
function jobValid(job) {
  return inOrder(async () => isCurrent(job, await loadRecord()));
}
const staleError = () => Object.assign(new Error("この保存は中断されました（以後は OpenRef へ送りません）"), { stale: true });

/* 進み具合（popup の状態行の文）か、記録の一部を書く。工程が有効なときだけ（中断・追い越しの後は書かない） */
function progress(job, patch) {
  if (typeof patch === "string") patch = { line: patch, cls: "" };
  return inOrder(async () => {
    const rec = await loadRecord();
    if (!isCurrent(job, rec)) return false;
    await saveRecord(Object.assign(rec, patch));
    return true;
  });
}
/* 終わりの記録（結果の文・分類・確定した DOI）とバッジ */
function finish(job, result) {
  return inOrder(async () => {
    const rec = await loadRecord();
    if (!isCurrent(job, rec)) return false;
    Object.assign(rec, result, { failClass: job.failClass, savedDoi: job.savedDoi, retry: retryInfo(job), endedAt: Date.now(), seen: false });
    // バッジは記録より先に（記録を見た popup がバッジを消す = その後に ✓ / ! を付け直さない）
    setBadge(result.state === "done" && job.ok ? "ok" : "attention");
    await saveRecord(rec);
    return true;
  });
}

/* connectd へ書き込む要求（保存・添付・PDF 取込・サーバ側の取得）。送る直前に工程の有効性を確かめ、無効なら送らない */
async function toConnectd(job, path, opts) {
  if (!(await jobValid(job))) throw staleError();
  return api(path, opts);
}

/* ================= バッジ（popup を閉じていても結果が分かる）=================
   走行中 = …・成功（PDF が OpenRef にある）= ✓・要確認（PDF なし・要確認ラベル・失敗・中断）= !。
   全タブ共通（タブ単位にすると、利用者がそのタブで別のページへ移ったときに消える）。結果を見た popup が消す */
const BADGE = { running: ["…", "#5f6368"], ok: ["✓", "#1e8e3e"], attention: ["!", "#e37400"] };
function setBadge(kind) {
  const [text, color] = BADGE[kind];
  try {
    Promise.resolve(chrome.action.setBadgeBackgroundColor({ color })).catch(() => {});
    Promise.resolve(chrome.action.setBadgeText({ text })).catch(() => {});
  } catch (e) { /* action が無い環境 */ }
}

/* 延命（KEEPALIVE_MS の説明）。工程の開始で始め、終わり・中断で止める。戻り値 = 止める関数 */
function keepAlive() {
  const timer = setInterval(() => {
    Promise.resolve(chrome.runtime.getPlatformInfo()).catch(() => {});
  }, KEEPALIVE_MS);
  return () => clearInterval(timer);
}

/* ================= 工程の開始・中断・起動時の直し =================
   入力（popup の openref-save）: タブの id と URL・解析の結果 page・content script を入れられたか・フォルダ・前置き。
   全サイト許可は popup の申告を使わず、ここで確かめ直す。前置きは使える物だけ（OpenRefShared.usableProxy） */
function makeJob(input, allSites) {
  return {
    id: Date.now().toString(36) + "-" + Math.random().toString(36).slice(2, 8),
    // documentId = popup が content script を入れた文書（Chrome 106 以降）。ページ内 fetch をこの文書に固定する（sendToUserTab）
    tab: { id: input.tabId, url: input.tabUrl || "", documentId: typeof input.documentId === "string" && input.documentId ? input.documentId : null },
    page: input.page,
    contentScriptOk: !!input.contentScriptOk,
    collectionId: input.collectionId == null ? "" : String(input.collectionId),
    proxyTemplate: OpenRefShared.usableProxy(input.proxy),
    allSites: !!allSites,
    savedDoi: null, // /capture/save が確定した DOI（応答に doi キーがあればそれ。無い旧 connectd だけページ抽出の DOI）。層 2 が使う
    failClass: null, // この保存の失敗の分類（classLine が記録する。popup の「プロキシ経由で開く」・確認の窓のボタンを出すかに使う）
    retryStage: null, // 分類が止まった段（pdf | pmc | doi・classLine が決める）。確認の窓で開く先とやり直す段
    itemKey: null, // 保存した item の key（確認の窓のやり直しは、この item に添付する）
    pdfPosted: false, // PDF タブの道で PDF を /capture/pdf へ送った（書誌の前に落ちたときの結果の印 = errorResult）
    retry: null, // 確認の窓の工程だけ: 前の保存の記録が持つやり直しの材料（startRetry）
    ok: false, // 成功（PDF が OpenRef にある・要確認ではない）= バッジ ✓
    cancelled: false, // 中断した（上限・利用者の「中断」・次の保存の追い越し）= 以後は記録も送信も背景タブもしない
    startedAt: 0,
  };
}

/* 走っている工程（この service worker の中で 1 つ）。工程の状態は job が持つ。ここは「中断の印を付ける・延命を止める」ための参照だけ */
let liveJob = null;
const jobRunning = () => !!liveJob && !liveJob.cancelled;

/* 工程を止める（記録は書かない）: 中断の印・延命を止める・工程が開いた背景タブを閉じる（登録済みの物だけ = background.js）・
   応答ヘッダの見張りを外す。
   走っている処理は止まらない（connectd へ送る直前・背景タブを開く直前の確かめで止まる） */
function stopJob(job) {
  if (job.stopped) return; // 2 回目以降は何もしない（後から始まった保存の背景タブを閉じない）
  job.stopped = true;
  job.cancelled = true;
  if (job.stop) job.stop();
  if (job.wake) job.wake(); // 確認の窓を待っている工程は、待ちをすぐ終える（窓は下の closeAllRegistered が閉じる）
  clearTimeout(job.limitTimer); // 上限のタイマーも止める（中断した工程の上限が後から来て、もう一度止めにかからない）
  if (liveJob === job) {
    liveJob = null;
    try {
      Promise.resolve(chrome.alarms.clear(JOB_ALARM)).catch(() => {});
    } catch (e) { /* noop */ }
  }
  // 応答ヘッダの見張り（listener とセッション規則）は、背景タブを閉じ終えてから外す（先に外すと、閉じる前のタブの
  // attachment が利用者のダウンロードになりうる）
  Promise.resolve(closeAllRegistered()).catch(() => {}).then(() => stopPdfWatches(job));
}
const busyText = (rec) =>
  "別の保存が進行中です（" + Math.max(0, Math.round((Date.now() - rec.startedAt) / 1000)) + " 秒前に開始）。終わってから保存してください";

/* 保存を始める。走行中の記録があれば断る（開始が上限より古い記録は中断として扱い、断らない）。
   opts（確認の窓 = startRetry）: run = 走らせる工程（既定 runSave）・expectId = 記録がこの id の終わった保存のままのときだけ始める・
   needAllSites = 全サイト許可が無ければ始めない・setup(job) = 工程に材料を渡す・carry = 記録に引き継ぐ題名と書誌の行・line = 最初の行。
   戻り値 {ok: true, id, done（工程の Promise）} か {ok: false, busy, error} */
async function startJob(input, opts = {}) {
  if (!input || typeof input.tabId !== "number" || !input.page || typeof input.page !== "object") {
    return { ok: false, error: "保存の入力が不正です" };
  }
  let allSites = false;
  try {
    allSites = await chrome.permissions.contains(OpenRefShared.ALL_SITES);
  } catch (e) { /* 確かめられない = 無いとみなす */ }
  if (opts.needAllSites && !allSites) {
    return { ok: false, error: "窓で確かめるには全サイトへのアクセス許可が要ります（拡張の設定 = アイコンを右クリック → オプション で許可できます）" };
  }
  const job = makeJob(input, allSites);
  if (opts.setup) opts.setup(job);
  const carry = opts.carry || {};
  const accepted = await inOrder(async () => {
    const rec = await loadRecord();
    if (rec && rec.state === "running" && Date.now() - rec.startedAt < JOB_LIMIT_MS) return { ok: false, busy: true, error: busyText(rec) };
    // 押したボタンの結果がもう今の記録ではない（別の保存が終わった後・やり直しを始めた後）= 始めない
    if (opts.expectId && (!rec || rec.id !== opts.expectId || rec.state !== "done")) return { ok: false, error: STALE_RETRY };
    job.startedAt = Date.now();
    const wrote = await saveRecord({ id: job.id, tabId: job.tab.id, tabUrl: job.tab.url, startedAt: job.startedAt, state: "running",
      line: opts.line || "保存を始めます…", cls: "", title: carry.title || null, ids: carry.ids || null, failClass: null, savedDoi: null,
      retry: null, endedAt: null, seen: false });
    // 記録を書けなければ始めない（始めても、工程は自分の記録が無い = 何も表示されず、connectd へも送らない）
    if (!wrote) return { ok: false, error: "保存の記録（chrome.storage.session）を書けないため、保存を始められません" };
    // 開始が上限より古い記録を上書きした: 同じ service worker で古い工程がまだ生きていれば止める（背景タブ・ページ内 fetch もしない）
    if (liveJob && liveJob !== job) stopJob(liveJob);
    liveJob = job;
    return { ok: true };
  });
  if (!accepted.ok) return accepted;
  setBadge("running");
  const stop = keepAlive();
  job.stop = stop; // 中断したときにも延命を止める
  const limitWhy = "保存が " + Math.round(JOB_LIMIT_MS / 60000) + " 分の上限を超えました";
  job.limitTimer = setTimeout(() => abortJob(job, limitWhy), JOB_LIMIT_MS);
  try {
    // service worker が工程の途中で止まっても、上限の時刻に起こして記録を中断に直す（起動時の直し）
    Promise.resolve(chrome.alarms.create(JOB_ALARM, { when: job.startedAt + JOB_LIMIT_MS })).catch(() => {});
  } catch (e) { /* noop */ }
  const done = (opts.run || runSave)(job).finally(() => {
    stop();
    clearTimeout(job.limitTimer);
    stopPdfWatches(job); // 背景タブの finally が外しているはず。漏れの保険
    if (liveJob === job) {
      liveJob = null;
      try {
        Promise.resolve(chrome.alarms.clear(JOB_ALARM)).catch(() => {});
      } catch (e) { /* noop */ }
    }
  });
  return { ok: true, id: job.id, done };
}

/* 中断の行。why が無い = 利用者が「中断」を押した（再試行は勧めない）。中断の前に送った /capture/attachment で PDF が付いていることもある */
const abortLine = (why) => why
  ? "✗ 中断: " + why + "。書誌や PDF は保存済みのことがあります（OpenRef で確かめてください）。もう一度 Capture してください"
  : "✗ 中断しました。書誌や PDF は保存済みのことがあります（OpenRef で確かめてください）";

/* 中断（上限・利用者の「中断」）: 以後この工程は記録も送信も背景タブもしない。記録は自分の id のときだけ「中断」にする
   （上限のタイマーが、後から始まった別の保存の記録を中断にしない） */
function abortJob(job, why) {
  stopJob(job);
  return inOrder(async () => {
    const rec = await loadRecord();
    if (!rec || rec.id !== job.id || rec.state !== "running") return false;
    Object.assign(rec, { state: "aborted", cls: "err", line: abortLine(why), endedAt: Date.now(), seen: false });
    setBadge("attention");
    await saveRecord(rec);
    return true;
  });
}

/* service worker の起動時: 前の service worker が残した「走行中」の記録を「中断」に直す（工程はメモリと一緒に消えている）。
   cond があれば、キューの中で読み直した記録が cond を満たすときだけ直す（currentRecord・利用者の中断 = 読んだ後に始まった
   別の保存を中断にしない）。戻り値 = 直した後の記録 */
function recoverRecord(why, cond) {
  return inOrder(async () => {
    const rec = await loadRecord();
    if (!rec || rec.state !== "running" || (cond && !cond(rec))) return rec;
    Object.assign(rec, { state: "aborted", cls: "err", line: abortLine(why), endedAt: Date.now(), seen: false });
    setBadge("attention");
    await saveRecord(rec);
    return rec;
  });
}
const SW_RESTART = "拡張の背景処理（service worker）が保存の途中で止まりました";

const LIMIT_WHY = "保存が " + Math.round(JOB_LIMIT_MS / 60000) + " 分の上限を超えました";
const isStale = (rec) => rec.state === "running" && Date.now() - rec.startedAt >= JOB_LIMIT_MS;

/* popup が開いたときに読む今の記録（openref-job）。開始が上限より古い「走行中」は中断に直してから返す
   （直すのは、キューの中で読み直しても同じ id で古いときだけ） */
async function currentRecord() {
  const rec = await inOrder(loadRecord);
  if (!rec || !isStale(rec)) return rec;
  if (liveJob && liveJob.id === rec.id) {
    await abortJob(liveJob, LIMIT_WHY);
    return inOrder(loadRecord);
  }
  return recoverRecord(LIMIT_WHY, (r) => r.id === rec.id && isStale(r));
}

/* popup の「中断」（openref-abort）: 走っている工程なら中断の印を付けて止め、記録を中断にする。工程がこの service worker に
   居なければ（止まって起き直した）記録だけ直す。どちらも id が同じ走行中の記録だけ。戻り値 = 直した後の記録 */
async function abortById(id) {
  const why = null; // 利用者の中断（abortLine）
  if (liveJob && liveJob.id === id) {
    await abortJob(liveJob, why);
    return inOrder(loadRecord);
  }
  return recoverRecord(why, (r) => r.id === id);
}

/* 上限の alarm（JOB_ALARM）: 走っている工程が上限を過ぎていれば中断する。service worker が止まっていた場合は、この alarm で
   起きたときの起動時の直し（background.js）が記録を中断にしている */
function onJobAlarm() {
  if (liveJob && !liveJob.cancelled && Date.now() - liveJob.startedAt >= JOB_LIMIT_MS) return abortJob(liveJob, LIMIT_WHY);
  return Promise.resolve(false);
}

/* popup が結果を表示した（openref-seen）: 記録に「見た」の印を付ける。別のタブの popup が、まだ誰も見ていない結果を 1 行で出すのに使う */
function markSeen(id) {
  return inOrder(async () => {
    const rec = await loadRecord();
    if (!rec || rec.id !== id || rec.state === "running" || rec.seen) return rec;
    rec.seen = true;
    await saveRecord(rec);
    return rec;
  });
}

/* ================= タブとの境目 ================= */

/* callback 版の sendMessage を Promise にする: lastError → {ok:false, error}・応答なし → "no response" */
function reply(send) {
  return new Promise((resolve) => {
    send((res) => {
      if (chrome.runtime.lastError) resolve({ ok: false, error: chrome.runtime.lastError.message, lost: true });
      else resolve(res || { ok: false, error: "no response" });
    });
  });
}
function sendToTab(msg, tabId) {
  return reply((cb) => chrome.tabs.sendMessage(tabId, msg, cb));
}

/* 利用者のタブ（保存を押したときの文書）へのメッセージ。popup が content script を入れた文書の documentId があれば、その文書に
   固定して送る（chrome.tabs.sendMessage の options.documentId・Chrome 106 以降）。同じ文書の中の移動（pushState・断片）では文書は
   変わらず、別のページへ移ると文書が無くなって送れない = 移ったとみなす。documentId が無い（古い Chrome・解析が URL だけ）ときは
   URL で確かめる（userTabUnchanged）。取りに行く URL は工程が持つ値で固定（タブの今の URL からは決めない） */
async function sendToUserTab(job, msg) {
  if (job.tab.documentId) {
    const r = await reply((cb) => chrome.tabs.sendMessage(job.tab.id, msg, { documentId: job.tab.documentId }, cb));
    if (r.lost) return { ok: false, error: NOT_REACHED + "（" + r.error + "）", lost: true };
    return r;
  }
  if (!(await userTabUnchanged(job))) return { ok: false, error: MOVED, lost: true };
  return sendToTab(msg, job.tab.id);
}

/* 利用者のタブがまだ保存を押したページにあるか（断片 # は見ない・規則は pdfbytes.js の samePage）。別のページへ移っていたら、そのタブからは取らない
   （別の論文のページの session で取らない）。別オリジンへ移ると activeTab が外れて URL が読めない = 移ったとみなす */
async function userTabUnchanged(job) {
  try {
    const t = await chrome.tabs.get(job.tab.id);
    return OpenRefShared.samePage(t.url, job.tab.url);
  } catch (e) {
    return false; // タブが閉じられた
  }
}
const MOVED = "利用者のタブが別のページへ移ったため未試行";
/* documentId で送って届かなかった（別のページへ移った・タブを閉じた・ページの側の失敗のどれか。Chrome のエラーの文で分けない） */
const NOT_REACHED = "保存を押したページへ届きませんでした";
/* 時間制限の付いた待ちの失敗を、ほかの失敗と同じ応答の形にする（timeout = 分類の印） */
const timedOut = (e) => ({ ok: false, error: String((e && e.message) || e), timeout: true });

function b64ToBytes(b64) {
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return bytes;
}

/* PDF バイト列の取得: ページ内 fetch（認証済み session・正しい referrer）→ 拡張（service worker）fetch。
   戻り値 {bytes, how} か {bytes: null, error, html, kind}（html = どれかが PDF の代わりに HTML を受けた・kind = 失敗の印） */
async function fetchPdfBytes(job, url) {
  const errors = [];
  let html = false;
  let kind = null;
  if (job.cancelled) throw staleError(); // 中断した工程は利用者のタブから取らない
  if (job.contentScriptOk) {
    progress(job, "PDF を取得中…（ページ内 fetch）");
    const r1 = await withTimeout(sendToUserTab(job, { type: "openref-fetch-pdf", url }), FIRST_FETCH_TIMEOUT_MS, "ページ内 fetch").catch(timedOut);
    if (r1.ok) return { bytes: b64ToBytes(r1.b64), how: "ページ内" };
    errors.push("ページ内 fetch: " + r1.error);
    html = html || !!r1.html;
    kind = r1.timeout ? "timeout" : fetchKind(job, r1);
  }
  if (job.cancelled) throw staleError(); // ページ内 fetch を待つ間に中断された
  progress(job, "PDF を取得中…（拡張から fetch）");
  const r2 = await withTimeout(bgFetchPdf(url, false), FIRST_FETCH_TIMEOUT_MS, "拡張 fetch").catch(timedOut);
  if (r2.ok) return { bytes: b64ToBytes(r2.b64), how: "拡張" };
  errors.push("拡張 fetch: " + r2.error);
  return { bytes: null, error: errors.join(" / "), html: html || !!r2.html, kind: kind || (r2.timeout ? "timeout" : fetchKind(job, r2)) };
}

/* ================= 失敗の分類 =================
   段ごとの理由（今の表示）はそのまま見せ、分類が確かなときだけ「分類と次の一手」を 1 行足す（迷ったら出さない =
   誤った分類より無いほうがよい）。印（kind）は失敗を作った所で付ける: mismatch / login / challenge / stuck / no_pdf。
   後の段（取り直し）で取れたら、前の段の印は消す（成功した段の印を最終の分類に持ち込まない） */
const NEXT_STEP = {
  mismatch: "分類: 別の論文の PDF だった → 意図した PDF なら DL して OpenRef へドロップ",
  login: "分類: 購読なし・ログインが要る → ブラウザで PDF を開いて（ログインして）から、その PDF タブで Capture",
  challenge: "分類: 確認ページ（bot 対策）のまま → ブラウザで一度 PDF を開いて確認を通してから、その PDF タブで Capture",
  stuck: "分類: PDF の URL がログイン・購入・確認などのページで止まった → ブラウザで PDF を開いてから、その PDF タブで Capture",
  no_pdf: "分類: ページと DOI ページに PDF の宣言が見つからない → ブラウザで PDF を開ければその PDF タブで Capture、無ければ DL して OpenRef へドロップ",
  timeout: "分類: サイトの応答が時間内に来なかった → 時間を置いてもう一度 Capture するか、ブラウザで PDF を開いてその PDF タブで Capture",
};
/* connectd の時間切れで工程を止めたときの次の一手（どの要求かで言い分ける。分類は timeout のまま） */
const CONNECTD_TIMEOUT_NEXT = {
  save: "分類: OpenRef の応答が時間内に来なかった → 少し待ってから OpenRef で書誌が登録されたかを確かめ、無ければもう一度 Capture",
  pdf: "分類: OpenRef の応答が時間内に来なかった → 少し待ってから OpenRef で PDF が付いたかを確かめ、付いていなければもう一度 Capture",
};
/* ================= 結果の行 =================
   1 行目 = 「書誌 ✓ ・ PDF ✓」か「書誌 ✓ ・ PDF ✗（理由）」。書誌が要確認（識別子なし・要確認 stub）なら「書誌 ？（要確認）」、
   書誌を保存する前に工程が落ちたら「書誌 ✗ ・ PDF —」。理由 = 失敗の分類の短い言い方・分類が無ければ「取得できず」など。
   2 行目 = 登録 / 既存 と key（PDF が付いたら、付いた経路の文を同じ行に続ける）。その後に書誌の注記（照合の理由・要確認・撤回）と
   次の一手（分類の行）。PDF が付かなかったときの段ごとの理由（前の版の PDF の文そのまま）は畳む。
   記録には line（全部）と head（畳まずに見せる部分 = line の先頭）を書く。popup は line が head + 改行で始まるときだけ残りを畳む
   （head の無い記録 = 前の版が残した記録・走行中・中断は、line をそのまま出す）。
   色（cls）はバッジと同じ規則: PDF が OpenRef にあり要確認でもない = done（緑・バッジ ✓）・ほかの終わり = warn（橙・バッジ !）・
   工程の失敗 = err（赤） */
const PDF_WHY = {
  mismatch: "別の論文だった",
  login: "ログインが要る",
  challenge: "確認ページのまま",
  stuck: "途中のページで止まった",
  no_pdf: "PDF が見つからない",
  timeout: "時間切れ",
};
const PDF_REFUSED = "書誌は保存済み。PDF は添付しませんでした — ";
const PDF_FAILED = "書誌は保存済み。PDF は取得できませんでした — ";
const UNKNOWN_ATTACH = "PDF 転送の応答が不明";
const BIB_REVIEW = "？（要確認）";
const DUP_ON_STUB = "同一ファイルが要確認 stub に添付済み"; // connectd の capture_pdf_ingest が duplicate の相手が要確認 stub のときに返す reason の頭
const headline = (bib, pdf) => "書誌 " + bib + " ・ PDF " + pdf;
const joinHead = (lines) => lines.filter(Boolean).join("\n");

/* 終わった保存（書誌は保存できた）の記録の欄。pdfOk = PDF が OpenRef にある（要確認で job.ok を落とす前の値）・
   pdfMsg = PDF の文（前の版の「— 」の後ろ）・why = 分類より先に使う理由（識別子なし）・notes = 書誌の注記 */
function doneResult(job, { bib, key, pdfOk, pdfMsg, why = null, notes = [] }) {
  let pdf;
  let next = "";
  let fold = "";
  if (pdfOk) {
    pdf = "✓";
    key += " — " + pdfMsg;
  } else {
    const c = job.failClass;
    pdf = pdfMsg.startsWith(UNKNOWN_ATTACH) ? "？（応答が不明）"
      : "✗（" + (why || PDF_WHY[c] || (pdfMsg.startsWith(PDF_REFUSED) ? "論文を確認できず" : "取得できず")) + "）";
    fold = pdfMsg;
    // 分類の行（次の一手）は畳まずに見せる（PDF の文の最後の行 = classLine・窓の時間切れ）
    const tail = c && NEXT_STEP[c] ? "\n" + NEXT_STEP[c] : null;
    if (tail && fold.endsWith(tail)) {
      next = NEXT_STEP[c];
      fold = fold.slice(0, -tail.length);
    }
  }
  const head = joinHead([headline(bib, pdf), key, ...notes, next]);
  return { state: "done", line: fold ? head + "\n" + fold : head, head, cls: job.ok ? "done" : "warn" };
}
/* 工程の失敗（runSave / runRetry の catch）の記録の欄。text = 前の版の行（「✗ 元の例外の文」と次の一手）。畳む物は無い。
   印: ✓ = 確かめた・✗ = 試して失敗した（connectd が失敗を答えた）・？ = 分からない（時間切れ・届いたか分からない）・— = 扱っていない。
   書誌の前に落ちたとき、PDF タブの道（/capture/pdf = PDF を送った）なら PDF は「—」ではなく ✗ / ？ */
function errorResult(job, e, text) {
  const late = !!(e && e.connectdTimeout);
  const answered = !!(e && typeof e.status === "number"); // connectd が HTTP の失敗で答えた（送った物は届いた）
  const unknownAnswer = !!(e && e.unknownAnswer); // connectd は答えたが、読めない形（null など）
  const first = job.itemKey
    ? headline("✓", late ? "？（時間切れ）" : "✗")
    : !job.pdfPosted
      ? headline(late ? "？（時間切れ）" : "✗", "—")
      : late ? headline("？（時間切れ）", "？")
      : answered ? headline("✗", "✗")
      : unknownAnswer ? headline("？", "？（応答が不明）")
      : headline("？", "？（届いたか不明）");
  const head = first + "\n" + text;
  return { state: "error", line: head, head, cls: "err" };
}

/* この工程を先へ進めない失敗: 中断・追い越し（stale）か、connectd の時間切れ */
const halts = (e) => !!e && (!!e.stale || !!e.connectdTimeout);

/* ページ内 / 拡張の fetch の失敗応答（content.js・background.js の httpStatus と転送の着き先 url）の印:
   401 か、着き先がログイン画面 → login。403 は bot 対策でも返るので印を付けない */
function fetchKind(job, res) {
  if (!res) return null;
  if (res.httpStatus === 401) return "login";
  if (res.url && OpenRefShared.isLoginUrl(res.url, job.proxyTemplate)) return "login";
  return null;
}

/* 工程の中の失敗（Error）の印: 作った所で付けた kind か、connectd の 409（/capture/attachment の同一性照合 = 別論文） */
const kindOf = (e) => (e && (e.kind || (e.status === 409 ? "mismatch" : null))) || null;
const kinded = (err, kind) => Object.assign(err, { kind: kind || null });

/* /capture/fetch-oa の失敗文の印。connectd の応答は文だけなので、core の pdffetch.rs が出す決まった言い回しだけを見る:
   - 「<host> が HTTP 401」（http_get の `"{} が HTTP {code}"`）→ login
   - no_pdf = 各段が「所在なし」の言い回しだけで、PDF を見つけて退けた行（judge の「…・添付せず」）が無い:
     pmc（fetch_pdf_for の "PMCID なし" / "は OA サブセット外" / "だが本体 PDF なし" / "は AWS Open Data に未収載"）・
     unpaywall（"OA 版なし" / "DOI なし" / "OPENREF_EMAIL 未設定"）・crossref（doi_fallback_download の "crossref: link[] に PDF なし"）。
     doi.org の着地ページの段は bot 対策と区別できないので見ない（ブラウザの DOI ページ経由の no_pdf と合わせて使う）
   言い回しが変わったら印が付かないだけ（誤った分類にはならない） */
function oaKind(e) {
  const m = String((e && e.message) || e);
  if (/が HTTP 401(?!\d)/.test(m)) return "login";
  if (!m.includes("添付せず") &&
      /pmc: (PMCID なし|PMC\d+ は OA サブセット外|PMC\d+ は OA（[^）]*）だが本体 PDF なし|PMC\d+ は AWS Open Data に未収載)/.test(m) &&
      /unpaywall: (OA 版なし|DOI なし|OPENREF_EMAIL 未設定)/.test(m) &&
      m.includes("crossref: link[] に PDF なし")) return "no_pdf";
  return null;
}

/* 段ごとの印（page = 論文ページの PDF・pmc = PMC 本文経由・server = /capture/fetch-oa・doiTab = DOI ページ経由）から分類を 1 つ。
   mismatch > login > challenge > stuck > timeout。no_pdf は、PMC 本文経由が走っておらず（PMC には宣言された PDF がありうる）、
   ページ・サーバ側の連鎖・DOI ページ（ブラウザで開いた着地ページ）の 3 つとも宣言が無いと言ったときだけ
   （どれかが走っていない・別の理由なら分類しない。「PDF が存在しない」とは言わない） */
function classifyFailure(k) {
  const all = [k.page, k.pmc, k.server, k.doiTab];
  for (const c of ["mismatch", "login", "challenge", "stuck", "timeout"]) if (all.includes(c)) return c;
  if (k.pmc === undefined && k.page === "no_pdf" && k.server === "no_pdf" && k.doiTab === "no_pdf") return "no_pdf";
  return null;
}
const classLine = (job, k) => {
  const c = classifyFailure(k);
  job.failClass = c; // 最終行を作る所でだけ呼ばれる（呼び手は return）= 最後の値がこの保存の分類
  job.retryStage = RETRY_CLASSES.includes(c) ? retryStageOf(job, k, c) : null;
  return c ? "\n" + NEXT_STEP[c] : "";
};

/* OA 経路の失敗文から末尾の一般案内（core の定型文）を落とし、tried 連鎖だけ見せる */
function oaReason(e) {
  const m = String((e && e.message) || e);
  const i = m.indexOf("。paywall");
  return i > 0 ? m.slice(0, i) : m;
}

/* /capture/attachment が添付を拒否した: 409 = 別論文（connectd の同一性照合・found_doi）/
   422 unverified = リンク拾いの PDF で目的 DOI を確認できない（照合不能を含む）。どちらも error は connectd の文をそのまま見せる */
const refused = (e) => e.status === 409 || (e.status === 422 && e.body && e.body.status === "unverified");

/* PDF のバイト列を /capture/attachment へ（source = PDF URL の出どころ・via = 取った経路 page / doi-tab） */
const postAttachment = (job, key, bytes, source, via) =>
  toConnectd(job, "/capture/attachment?key=" + encodeURIComponent(key) + "&source=" + encodeURIComponent(source === "sciencedirect" ? "link" : source || "") + "&via=" + via, {
    method: "POST",
    headers: { "Content-Type": "application/pdf" },
    body: bytes,
  });

/* ================= PDF を付ける段 =================
   識別子ありの item に PDF を付ける道は「段の並び」を 1 本の実行器（runStages）が順に試す:
   保存の道 = 論文ページ → PMC 本文 → OpenRef 側（/capture/fetch-oa）→ DOI ページ（saveStages）/ 確認の窓のやり直し = 止まった段 1 つ
   （retryStages）。段を足す・外すのは並びの 1 か所（全サイト許可の有無もここで段を外す = saveStages）。
   段 = {kind（分類の印の名前 = page / pmc / server / doiTab）, run(job, key, failed) → {line}（付いた = 結果の文）か {fail, kind}（次の段へ）
   か {refused: e}（connectd が添付を拒んだ = 止める）, sep（失敗の行の区切り・前に行があるときだけ）, failText(e)・kindOf(e)（run が投げた
   例外を行と印にする段だけが持つ）, stopOnRefuse（投げた例外が拒否なら止める）, skip（走らせない段の行 = 「未試行」）}。
   中断・追い越し・connectd の時間切れ（halts）はどの段でも投げ直す。failText の無い段（論文ページ）が投げた例外も、そのまま工程の
   失敗（runSave / runRetry の「✗ <元の例外の文>」）へ投げ直す（段の中で拾う範囲は、段ごとの try だったときの範囲のまま）。
   拒否で止めるのは、先へ進んで成功すると「別論文だった」事実が表示から消えるため（保存の道の DOI ページは最後の段なので行に残す） */
async function runStages(job, key, stages, footer = "") {
  const kinds = {}; // 段ごとの失敗の印（classifyFailure）。走らなかった段は印を持たない
  let failed = ""; // 失敗の行（段ごとの理由をつないだ物）
  for (const st of stages) {
    if (st.skip) {
      failed += (failed ? st.sep : "") + st.skip;
      continue;
    }
    let r;
    try {
      r = await st.run(job, key, failed);
    } catch (e) {
      if (halts(e) || !st.failText) throw e;
      r = st.stopOnRefuse && refused(e) ? { refused: e } : { fail: st.failText(e), kind: (st.kindOf || kindOf)(e) };
    }
    if (r.refused) {
      return PDF_REFUSED + String(r.refused.message || r.refused) + classLine(job, { [st.kind]: kindOf(r.refused) });
    }
    if (r.line !== undefined) return r.line;
    failed += (failed ? st.sep : "") + r.fail;
    kinds[st.kind] = r.kind;
  }
  return PDF_FAILED + failed + footer + classLine(job, kinds);
}

/* 取得（ページ内 fetch → 拡張 fetch）→ HTML が返り nav が真なら PDF タブで開き直す。{got（fetchPdfBytes の形）, kind, source}。
   PDF タブで取れたら最初の取得の印（ログイン画面への転送など）は持ち込まない・別オリジンに着いたら source = link */
async function fetchWithPdfTab(job, url, source, nav) {
  let got = await fetchPdfBytes(job, url);
  let kind = got.kind;
  if (!got.bytes && got.html && nav) {
    try {
      const t = await fetchPdfViaNavigation(job, url, job.tab.id, job.contentScriptOk);
      got = { bytes: b64ToBytes(t.b64), how: t.how };
      kind = null;
      if (t.moved) source = "link";
    } catch (e) {
      if (halts(e)) throw e;
      got.error += " / PDF タブ: " + String(e.message || e);
      kind = kindOf(e) || kind;
    }
  }
  return { got, kind, source };
}

/* 論文ページが示す PDF（保存の道）/ 止まった PDF の段（やり直し）。url が無ければ「PDF リンクなし」。
   retry = やり直しの書き方（進み具合と 2 本目の文に経路とホスト・不明な応答はそのまま結果）。
   保存の道の書き方は、進み具合が大きさ・2 本目の文に経路を書かない・不明な応答と転送失敗は次の段へ */
function pageStage(url, { nav, retry }) {
  return {
    kind: "page",
    sep: "",
    // failText なし: 取得（fetchPdfBytes）が投げた想定外の例外は行にせず、工程の失敗へそのまま（前と同じ）
    async run(job, key) {
      if (!url) return { fail: "ページに PDF リンクなし", kind: "no_pdf" }; // PubMed ページ等 = 次の段へ
      const { got, kind, source } = await fetchWithPdfTab(job, url, job.page.pdfSource, nav);
      if (!got.bytes) return { fail: "ブラウザ経由で取れず（" + got.error + "）", kind };
      const how = retry ? got.how + " fetch・" + hostOf(url) : got.how + " fetch";
      let res;
      try {
        res = await sendPdf(job, key, got.bytes, source, "page", retry ? how : kb(got.bytes.length));
      } catch (e) {
        if (halts(e)) throw e; // connectd の時間切れは先の段へ進まない（後から付く PDF と 2 本にしない）
        if (refused(e)) return { refused: e }; // 実行器が「添付しませんでした」で止める
        return { fail: String(e.message || e), kind }; // 「転送失敗（…）」（sendPdf が包んだ文）
      }
      let line;
      try {
        line = attachedLine(job, res, how, retry);
      } catch (e) {
        // 応答を読めない（connectd が null を返した等）: 前と同じく、保存の道は「転送失敗（…）」で次の段へ（応答の解釈も送信の try の中だった）・
        // やり直しは元の例外の文で終わる（postBytes の解釈は送信の try の外だった）
        return { fail: retry ? String(e.message || e) : "転送失敗（" + String(e.message || e) + "・" + kb(got.bytes.length) + "）", kind };
      }
      if (line) return { line };
      return retry ? { line: unknownAttach(res) } : { fail: "転送結果 " + res.status, kind };
    },
  };
}
/* PMC 本文（PubMed の full-text links の PMC を背景タブで開く）。拒否で止まる */
const pmcStage = () => ({
  kind: "pmc",
  sep: " / ",
  stopOnRefuse: true,
  failText: (e) => "PMC 本文経由: " + String(e.message || e),
  run: async (job, key) => ({ line: await viaDoiTab(job, key, job.savedDoi, { url: job.page.pmcUrl, pmid: job.page.pmid }) }),
});
/* OpenRef 側の連鎖（/capture/fetch-oa = PMC AWS Open Data → Unpaywall → Crossref link → doi.org 着地ページ）。失敗の行は core の定型文を落とした連鎖 */
const serverStage = () => ({
  kind: "server",
  sep: "\n",
  stopOnRefuse: false,
  failText: (e) => "OpenRef 側: " + oaReason(e),
  kindOf: oaKind,
  async run(job, key, failed) {
    progress(job, failed + " → OpenRef 側で取得を試行…（PMC / Unpaywall / Crossref / DOI 着地ページ）");
    const oa = await toConnectd(job, "/capture/fetch-oa", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ key }),
    });
    // 付いたと言えるのは、取得元の名前を返した応答だけ（null・形の違う応答 = 付いたと言わず、失敗の行で次の段へ）
    if (!oa || typeof oa !== "object" || typeof oa.source !== "string" || !oa.source) throw new Error("応答が不明（" + JSON.stringify(oa) + "）");
    job.ok = true;
    return { line: "PDF 添付 ✓（取得元: " + oa.source + "）" };
  },
});
/* DOI ページ（背景タブ・前置きがあれば前置き経由 = viaDoiTab）。skip = 走らせないときの行（保存の道の saveStages が決める） */
const doiTabStage = ({ stopOnRefuse, skip = null }) => ({
  kind: "doiTab",
  sep: "\n",
  stopOnRefuse,
  skip,
  failText: (e) => "DOI ページ経由: " + String(e.message || e),
  run: async (job, key) => ({ line: await viaDoiTab(job, key, job.savedDoi) }),
});
const SAVE_FOOTER = "\n誌面で PDF を開いた状態でもう一度 Capture するか、DL して OpenRef へドロップしてください";

/* 保存の道の段の並び。全サイト許可を見るのはここだけ: 論文ページの PDF を PDF タブで開き直す（宣言された PDF だけ = ページ上の
   リンク拾いは user が押していないリンクなので開かない）・PMC 本文・DOI ページ（無ければ「未試行」の行） */
function saveStages(job) {
  const page = job.page;
  const doi = job.savedDoi;
  return [
    pageStage(page.pdfUrl, { nav: job.allSites && OpenRefShared.declaredPdf(page), retry: false }),
    page.pmcUrl && page.pmid && job.allSites ? pmcStage() : null,
    serverStage(),
    doiTabStage({
      stopOnRefuse: false,
      skip: !doi ? "DOI ページ経由: DOI なしのため未試行"
        : !job.allSites ? "DOI ページ経由: 全サイトへのアクセス許可が無いため未試行（拡張の設定 = アイコンを右クリック → オプション で許可できます）"
        : null,
    }),
  ].filter(Boolean);
}

/* 識別子ありの item に PDF を付ける（保存の道）。戻り値 = 結果の行の PDF の部分 */
function attachPdf(job, key) {
  return runStages(job, key, saveStages(job), SAVE_FOOTER);
}

/* /capture/save・/capture/pdf の応答（同形）: 題名を記録し、「著者 (年) · 誌名」を返す（ids 欄は呼び手が書く） */
function showItem(job, res) {
  progress(job, { title: plain(res.title) || job.page.title || "" });
  return (res.first_author || "?") + (res.year ? " (" + res.year + ")" : "") + (res.journal ? " · " + res.journal : "");
}

/* 識別子なし・PDF タブ: バイト列を /capture/pdf へ（本文の DOI/PMID で書誌確定）。
   {done: true, line, head, cls} = 処理完了、{done: false, error} = PDF を取れなかったので従来の /capture/save（題名のみ）へ。
   その場合は同じ PDF を再 fetch しない（attachPdf は呼ばない） */
async function ingestPdfFlow(job) {
  const page = job.page;
  const got = await fetchPdfBytes(job, page.pdfUrl || page.url);
  if (!got.bytes) {
    progress(job, "PDF を取得できず（" + got.error + "）→ 題名のみで保存します…");
    return { done: false, error: got.error };
  }
  progress(job, "PDF を OpenRef へ転送中…（" + kb(got.bytes.length) + "・本文から DOI/PMID を抽出して書誌を確定）");
  const qs = new URLSearchParams();
  if (job.collectionId) qs.set("collection_id", job.collectionId);
  if (page.title) qs.set("title", page.title);
  if (page.url) qs.set("url", OpenRefShared.plainUrl(page.url, job.proxyTemplate)); // 要確認 stub が保存する URL = 出版社の URL（取得はプロキシのまま）
  job.pdfPosted = true; // ここから先の失敗は「PDF を送った」（errorResult の印）
  const res = await toConnectd(job, "/capture/pdf?" + qs.toString(), {
    method: "POST",
    headers: { "Content-Type": "application/pdf" },
    body: got.bytes,
  });
  // 届いたが読めない応答（JSON の null など）は「届いたか不明」ではない
  if (!res || typeof res !== "object") throw Object.assign(new Error("OpenRef の応答が不明（" + JSON.stringify(res) + "）"), { unknownAnswer: true });
  const label = showItem(job, res);
  progress(job, { ids: res.trashed ? "（ゴミ箱内の item）" : label });
  const k = res.key;
  // 2 行目（前の版の行から先頭の印 ✓ / ＝ / ？ を外した物）。照合の理由（reason）は 同一 PDF・要確認のときだけ（前の版と同じ）
  const keyLines = {
    added: "登録 " + k + " — PDF 本文の識別子で書誌を確定して添付（" + got.how + " fetch）",
    attached: "既存 " + k + " に PDF を添付（本文の識別子が既存 item と一致）",
    attached_additional: "既存 " + k + " に 2 本目として添付（既存添付と別ファイル）",
    duplicate: "同一 PDF が既に添付済み（" + k + (res.trashed ? "・ゴミ箱内。完全削除すれば取り込み直せます" : "") + "）",
    needs_review: "要確認として登録 " + k,
    stub: "要確認として登録 " + k + "（PDF は添付済み。詳細ペインの編集で DOI/PMID を入力すると書誌が入ります）",
  };
  const known = Object.prototype.hasOwnProperty.call(keyLines, res.status);
  const review = res.status === "needs_review" || res.status === "stub";
  const notes = [];
  if (res.reason && (review || res.status === "duplicate")) notes.push(res.reason);
  if (res.retraction) notes.push("⚠ 撤回論文の可能性: " + res.retraction);
  // 同一 PDF の相手の書誌: ゴミ箱の中（connectd の trashed の最小形）・要確認 stub（connectd の reason の決まった言い回し）・
  // 生きている item（summarize_item の has_pdf が真）。どれとも言えない（古い connectd・stub かを確かめられなかった）は言い切らない
  const dupBib = res.status !== "duplicate" ? null
    : res.trashed === true ? "？（ゴミ箱の中）"
    : typeof res.reason === "string" && res.reason.startsWith(DUP_ON_STUB) ? BIB_REVIEW
    : res.has_pdf === true ? "✓"
    : "？";
  // バッジ ✓ = PDF が OpenRef にあり、書誌が確かなとき（同一 PDF の相手が確かな item のときだけ。stub の status と同じく、相手が要確認 /
  // ゴミ箱 / 分からないなら !）
  job.ok = ["added", "attached", "attached_additional"].includes(res.status) || dupBib === "✓";
  // 要確認 stub も PDF は添付済み（書誌が確定していない）。知らない status は書誌も PDF も分からない
  const head = joinHead([
    known ? headline(review ? BIB_REVIEW : dupBib || "✓", "✓") : headline("？", "？（応答が不明）"),
    // 知らない status は status と key を並べる（どちらも無い = 応答そのもの。「undefined undefined」と出さない）
    known ? keyLines[res.status] : [res.status, k].filter((x) => typeof x === "string" && x).join(" ") || "応答: " + JSON.stringify(res),
    ...notes,
  ]);
  return { done: true, line: head, head, cls: job.ok ? "done" : "warn" };
}

/* ================= 応答ヘッダで PDF の到着を知る（全サイト許可のある人だけ）=================
   工程が開いた背景タブについてだけ、最上位の読込（main_frame）の応答ヘッダを見る。content-type が PDF か、
   content-disposition が attachment（で PDF でありうる物）の応答が来たら、その URL を「PDF の所在」として採る。
   - 見るのは工程が開いたタブだけ（listener はタブを開いたときにそのタブに絞って付け、タブを閉じる・工程が終わる・中断で外す）。
     利用者のタブ・ページの中の iframe（補足資料の PDF などを論文の PDF と取り違えないため）は見ない。
   - 同じタブにだけ効く DNR のセッション規則で、PDF / attachment の応答を拡張内の空のページへ転送する。背景タブがダウンロードを
     始めたり（attachment）、PDF ビューアを読み込んだりしないように。応答ヘッダを条件にする規則は Chrome 128 以降なので、それより
     古い Chrome では規則を入れず検出だけにする（ダウンロードは止まらない）。規則を入れられなくても工程は続ける。
   - セッション規則は service worker が止まっても残る（ブラウザを閉じるまで）。工程の終わり・中断・背景タブを閉じるときに外し、
     service worker の起動時に自分の id の範囲の規則を全部外す（removeAllPdfRules）。
   - 検出した URL は取り直しの先として使うだけ。中身が PDF か（%PDF）・目的の論文か（/capture/attachment の DOI 照合）は今と同じ
     所で確かめる。転送した後のタブの URL（拡張内のページ）は、PDF の URL にも記録の行にもログイン画面の判定にも使わない。
   - 状態はタブごとの見張り（watch）に持ち、工程のオブジェクトに結び付ける（job.pdfWatches）。service worker の大域には置かない。 */
const PDF_STOP_PAGE = "/stopped.html"; // 転送先（web_accessible_resources・use_dynamic_url = サイトが静的な URL で拡張の有無を調べられない・script なし）
const PDF_RULE_MIN = 7001; // この工程が入れるセッション規則の id の範囲（起動時の後始末はこの範囲だけを外す）
const PDF_RULE_SPAN = 1000;
const PDF_RULE_MIN_CHROME = 128; // 応答ヘッダを条件にする DNR 規則（RuleCondition.responseHeaders）が使える版
let pdfRuleSeq = 0; // id の払い出し（同時に開く背景タブは 2 つまで = 範囲が回っても重ならない）
/* attachment で content-type が何も言っていない物（これらと、content-type の無い物だけを PDF の候補にする） */
const GENERIC_TYPES = ["application/octet-stream", "binary/octet-stream", "application/download", "application/force-download",
  "application/x-download", "application/unknown"];

/* Chrome の版（UA の「Chrome/N」。headless は「HeadlessChrome/N」なので語の境目を見ない）。読めなければ 0 = 規則を入れない */
function chromeMajor() {
  try {
    const m = /Chrome\/(\d+)/.exec(String(navigator.userAgent || ""));
    return m ? Number(m[1]) : 0;
  } catch (e) {
    return 0;
  }
}
/* content-disposition の filename（filename* を先に。無ければ null） */
function dispositionFilename(disp) {
  const star = /filename\*\s*=\s*(?:[^']*'[^']*')?"?([^";]+)"?/i.exec(disp);
  if (star) {
    try {
      return decodeURIComponent(star[1].trim());
    } catch (e) {
      return star[1].trim();
    }
  }
  const plainName = /filename\s*=\s*(?:"([^"]*)"|([^;]*))/i.exec(disp);
  const v = plainName ? (plainName[1] !== undefined ? plainName[1] : plainName[2] || "").trim() : "";
  return v || null;
}
/* この応答が PDF（の所在）か。headers = webRequest の responseHeaders（名前と値の大文字小文字は揺れる）。
   attachment は、content-type が PDF か、何も言っていない（GENERIC_TYPES・無し）ときだけ候補にし、ファイル名（無ければ URL の
   最後の段）に PDF 以外の拡張子があれば採らない（zip・補足資料のダウンロード）。候補の中身は取った後に %PDF で確かめる */
function isPdfResponse(headers, url) {
  let type = "";
  let disp = "";
  for (const h of headers || []) {
    const name = String((h && h.name) || "").toLowerCase();
    const value = h && h.value != null ? String(h.value) : "";
    if (name === "content-type") type = value;
    else if (name === "content-disposition") disp = value;
  }
  const media = type.split(";")[0].trim().toLowerCase();
  if (media === "application/pdf" || media === "application/x-pdf") return true;
  if (disp.split(";")[0].trim().toLowerCase() !== "attachment") return false;
  if (media && !GENERIC_TYPES.includes(media)) return false;
  let last = "";
  try {
    last = decodeURIComponent(new URL(url).pathname.split("/").pop() || "");
  } catch (e) { /* URL として読めない = 拡張子で判断しない */ }
  const ext = /\.([a-z0-9]{1,8})$/i.exec(dispositionFilename(disp) || last);
  return !ext || ext[1].toLowerCase() === "pdf";
}

const NO_WATCH = { active: false, found: () => null, onFound: () => () => {}, stop: () => Promise.resolve() };
/* 見張りを付けられるか（全サイト許可があり、webRequest が使える）。付けられないときは今の動き（0.2.8）のまま */
const canWatchHeaders = (job) =>
  !!job.allSites && typeof chrome !== "undefined" && !!chrome.webRequest && !!chrome.webRequest.onHeadersReceived;

/* 拡張内の空のページ（転送の後のタブの URL）か。use_dynamic_url を付けているので、拡張の id の部分は静的な id とは限らない
   （ブラウザの起動ごとの id になりうる）= スキームとパスで見る */
function isStopPage(url) {
  try {
    const u = new URL(url);
    return u.protocol === "chrome-extension:" && u.pathname === PDF_STOP_PAGE;
  } catch (e) {
    return false;
  }
}

/* タブ tabId の見張りを始める（webRequest の listener と DNR のセッション規則）。戻り値 = {active, found(), onFound(cb), stop()}。
   found() = 検出した PDF の URL（最初の 1 件）か null。onFound(cb) = 検出したときに cb を 1 回呼ぶ（戻り値 = 取り消す関数。
   読込の待ちと競わせる = waitForTabLoad）。stop() は何度呼んでもよい（listener と規則を外す） */
async function watchPdfHeaders(job, tabId) {
  if (!canWatchHeaders(job) || job.cancelled) return NO_WATCH;
  let found = null;
  let ruleId = null;
  let stopped = false;
  const waiters = new Set();
  const onHeaders = (d) => {
    // filter でも絞っているが、ここでも確かめる（このタブの最上位の読込・http(s)・2xx だけ）
    if (stopped || !d || d.tabId !== tabId || d.type !== "main_frame" || (d.frameId != null && d.frameId !== 0)) return;
    if (!/^https?:/i.test(d.url || "") || !(d.statusCode >= 200 && d.statusCode < 300)) return;
    if (found || !isPdfResponse(d.responseHeaders, d.url)) return; // 採るのは最初の 1 件だけ
    found = d.url;
    for (const cb of Array.from(waiters)) {
      waiters.delete(cb);
      try {
        cb(found);
      } catch (e) { /* 待ち手の失敗は見張りに持ち込まない */ }
    }
  };
  try {
    chrome.webRequest.onHeadersReceived.addListener(onHeaders, { urls: ["http://*/*", "https://*/*"], types: ["main_frame"], tabId }, ["responseHeaders"]);
  } catch (e) {
    return NO_WATCH; // listener を付けられない = 今の動き
  }
  const dnr = chrome.declarativeNetRequest;
  if (dnr && typeof dnr.updateSessionRules === "function" && chromeMajor() >= PDF_RULE_MIN_CHROME) {
    const id = PDF_RULE_MIN + (pdfRuleSeq++ % PDF_RULE_SPAN);
    try {
      await pdfRuleCleanup;
      await dnr.updateSessionRules({
        removeRuleIds: [id],
        addRules: [{
          id,
          priority: 1,
          action: { type: "redirect", redirect: { extensionPath: PDF_STOP_PAGE } },
          condition: {
            tabIds: [tabId],
            resourceTypes: ["main_frame"],
            // 検出（isPdfResponse）より広い: このタブは何もダウンロードしない（zip の attachment も止める。採るかは検出が決める）
            responseHeaders: [
              { header: "content-type", values: ["application/pdf*", "application/x-pdf*"] },
              { header: "content-disposition", values: ["attachment*"] },
            ],
          },
        }],
      });
      ruleId = id;
    } catch (e) { /* 規則を入れられない（古い Chrome・上限など）= 検出だけ（ダウンロードは止まらない） */ }
  }
  const watch = {
    active: true,
    found: () => (stopped || job.cancelled ? null : found),
    onFound: (cb) => {
      if (found && !stopped) { // もう検出している（前の待ちの間に届いた）= すぐ知らせる
        Promise.resolve().then(() => cb(found));
        return () => {};
      }
      waiters.add(cb);
      return () => waiters.delete(cb);
    },
    stop: async () => {
      if (stopped) return;
      stopped = true;
      waiters.clear();
      if (job.pdfWatches) job.pdfWatches.delete(watch);
      try {
        chrome.webRequest.onHeadersReceived.removeListener(onHeaders);
      } catch (e) { /* noop */ }
      if (ruleId != null) {
        try {
          await chrome.declarativeNetRequest.updateSessionRules({ removeRuleIds: [ruleId] });
        } catch (e) { /* 起動時の後始末（removeAllPdfRules）が外す */ }
      }
    },
  };
  (job.pdfWatches || (job.pdfWatches = new Set())).add(watch);
  // 規則を入れる間に中断された: すぐ外す（stopJob は見張りを登録する前に走り終えている）
  if (job.cancelled) {
    await watch.stop();
    return NO_WATCH;
  }
  return watch;
}

/* 工程の見張りを全部外す（工程の終わり・中断） */
function stopPdfWatches(job) {
  const all = job && job.pdfWatches ? Array.from(job.pdfWatches) : [];
  return Promise.all(all.map((w) => w.stop().catch(() => {})));
}

/* service worker の起動時: 前の service worker の工程が残した規則（この工程の id の範囲）を全部外す。戻り値 = 外した数。
   after = 先に済ませること（背景タブを閉じ終える）。起動の直後に始まった工程が規則を入れるのは、この後始末が済んでから
   （pdfRuleCleanup = 入れたばかりの規則を外さない） */
let pdfRuleCleanup = Promise.resolve(0);
function removeAllPdfRules(after) {
  const run = (async () => {
    try {
      await after;
    } catch (e) { /* 閉じられなくても規則は外す */ }
    try {
      const dnr = chrome.declarativeNetRequest;
      if (!dnr || typeof dnr.getSessionRules !== "function") return 0;
      const ids = (await dnr.getSessionRules()).map((r) => r.id).filter((id) => id >= PDF_RULE_MIN && id < PDF_RULE_MIN + PDF_RULE_SPAN);
      if (ids.length) await dnr.updateSessionRules({ removeRuleIds: ids });
      return ids.length;
    } catch (e) {
      return 0;
    }
  })();
  pdfRuleCleanup = run;
  return run;
}

/* 工程の背景タブを開く。見張りを付けられるときは、空のタブを開いて見張りを付けてから目的の URL へ移す（開いた直後の応答を
   取りこぼさない）。付けられないときは今と同じく目的の URL で開く。戻り値 = 開いたタブ（見張りは openWatch が付ける） */
function createBackgroundTab(job, url) {
  return chrome.tabs.create({ url: canWatchHeaders(job) ? "about:blank" : url, active: false });
}
async function openWatch(job, tabId, url) {
  if (!canWatchHeaders(job)) return NO_WATCH;
  const watch = await watchPdfHeaders(job, tabId);
  try {
    if (job.cancelled) throw staleError(); // 中断された工程は目的の URL へ移さない
    await chrome.tabs.update(tabId, { url });
  } catch (e) {
    // 移せなかった（利用者が空のタブを閉じた・URL が不正）: 呼び手は見張りを受け取らない = ここで外してから投げ直す
    await watch.stop();
    throw e;
  }
  return watch;
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

/* /capture/attachment の応答 → 結果の文（付いた・既に付いていた = job.ok）。分からない応答は null。
   withHow = 2 本目の文にも経路を書く（背景タブ・窓・やり直し。保存の道の論文ページの段は書かない） */
function attachedLine(job, res, how, withHow) {
  const mark = res.doi_check === "unknown" ? "・DOI 未確認" : "";
  if (res.status === "attached" || res.status === "attached_additional" || res.status === "skipped") job.ok = true;
  if (res.status === "attached") return "PDF 添付 ✓（" + how + "・" + kb(res.bytes) + mark + "）";
  if (res.status === "attached_additional") return "PDF を 2 本目として添付 ✓（既存添付と別ファイル・" + (withHow ? how + "・" : "") + kb(res.bytes) + mark + "）";
  if (res.status === "skipped") return "PDF は既に添付済み（同一ファイル）";
  return null;
}
const unknownAttach = (res) => UNKNOWN_ATTACH + "（" + JSON.stringify(res) + "）";
/* 進み具合を書いて /capture/attachment へ送る。拒否（refused）・中断・connectd の時間切れはそのまま投げ、ほかの失敗は「転送失敗（…）」で投げる */
async function sendPdf(job, key, bytes, source, via, progressText) {
  progress(job, "PDF を OpenRef へ転送中…（" + progressText + "）");
  try {
    return await postAttachment(job, key, bytes, source, via);
  } catch (e) {
    if (refused(e) || halts(e)) throw e;
    throw new Error("転送失敗（" + String(e.message || e) + "・" + kb(bytes.length) + "）");
  }
}

/* base64 の PDF を /capture/attachment へ（背景タブ = via doi-tab）。成功なら表示用メッセージ、失敗なら throw（拒否は connectd の error 文のまま） */
function postPdf(job, key, b64, how, source) {
  return postBytes(job, key, b64ToBytes(b64), how, source, "doi-tab");
}
async function postBytes(job, key, bytes, how, source, via) {
  const res = await sendPdf(job, key, bytes, source, via, how);
  return attachedLine(job, res, how, true) || unknownAttach(res);
}

/* タブの読込完了を待つ。complete の後も meta refresh の連鎖で再び loading になることがあるので、
   complete から少し静かな時間が続いたら解決する（中継ページのままかどうかは呼び出し側が content script に聞く）。
   watch（応答ヘッダの見張り）を渡すと、PDF の到着の検出とも競わせ、先に来た方で解決する（規則の無い Chrome では、行き先が
   ダウンロードに回るとタブは about:blank のまま complete が来ない = 検出で進む） */
function waitForTabLoad(tabId, timeoutMs, watch = NO_WATCH) {
  return new Promise((resolve, reject) => {
    let lastUrl = null;
    let settle = null;
    let done = false;
    const finish = (err) => {
      if (done) return;
      done = true;
      offFound();
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
      // 見張りを付ける間の空のタブ（about:blank → 目的の URL へ移す = createBackgroundTab）の読込は着地として待たない
      if (info.status === "complete" && !(t && t.url === "about:blank")) {
        clearTimeout(settle);
        settle = setTimeout(() => finish(), 800);
      }
    };
    const onRem = (id) => {
      if (id === tabId) finish(new Error("タブが閉じられました"));
    };
    const offFound = watch.onFound(() => finish());
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
   content script の fetch はページオリジンの CORS を受ける（別オリジンの配信ホスト = AIC の storage. 等。
   `ACAO: *` の署名付き URL は content.js が Cookie なしで再試行する）ので、
   全サイト権限を持つ SW の fetch（CORS を受けない）へ必ず fallback する。「HTML なら 1 回だけ追従」は
   タブ内 = meta refresh か同一オリジンの .pdf パスがちょうど 1 つ / SW = meta refresh のみ（規則差は background.js に明記）。
   注意: 追従先（meta refresh の飛び先 = 任意ホスト）へも user の cookie 付きで fetch する。
   タブが利用者のタブなら、保存を押したときの文書に固定して送る（sendToUserTab。移っていたら送れない = 拡張 fetch へ）。
   失敗の Error は、どちらかが PDF の代わりに HTML を受けていれば `html = true`（fetchPdfViaNavigation へ回す目印）。 */
async function fetchPdfViaTabThenBg(job, tabId, url, injected) {
  const errors = [];
  let html = false;
  let f = null;
  if (job.cancelled) throw staleError(); // 中断した工程は利用者のタブから取らない
  if (injected) {
    const msg = { type: "openref-fetch-pdf", url, follow: true };
    // 利用者のタブなら保存を押したときの文書に固定する（sendToUserTab）。背景タブは工程が開いた物
    f = await withTimeout(tabId === job.tab.id ? sendToUserTab(job, msg) : sendToTab(msg, tabId), FETCH_TIMEOUT_MS, "タブ内 fetch");
    if (f.ok) return { b64: f.b64, how: "タブ内" };
    errors.push("タブ内: " + f.error);
    html = html || !!f.html;
  }
  if (job.cancelled) throw staleError(); // タブ内 fetch を待つ間に中断された
  const g = await withTimeout(bgFetchPdf(url, true), FETCH_TIMEOUT_MS, "拡張 fetch");
  if (g.ok) return { b64: g.b64, how: "拡張" };
  errors.push("拡張: " + g.error);
  const err = new Error(errors.join(" / "));
  err.html = html || !!g.html;
  err.kind = (injected && fetchKind(job, f)) || fetchKind(job, g);
  throw err;
}

/* タブの読込の世代（onUpdated の loading / complete のたびに +1）。「次のページへ移ったか」を URL ではなく世代で見る
   （同じ URL の reload = PMC の proof-of-work の後も拾う。解析の往復中に始まった遷移も取りこぼさない） */
function watchTab(tabId) {
  let gen = 0;
  const onUpd = (id, info) => {
    if (id === tabId && info.status) gen++;
  };
  chrome.tabs.onUpdated.addListener(onUpd);
  return { gen: () => gen, stop: () => chrome.tabs.onUpdated.removeListener(onUpd) };
}

/* cond() が ms 以内に真になるか（250 ms ごとに見る） */
async function waitUntil(cond, ms) {
  const end = Date.now() + ms;
  while (!cond()) {
    if (Date.now() >= end) return false;
    await sleep(250);
  }
  return true;
}

/* 宣言された PDF の URL が HTML（中継ページ）を返すとき: user が PDF のリンクを押したのと同じく背景タブで開き、
   サイト自身の JS を走らせてから取り直す。中継ページの例 = PMC の「Preparing to download …」（proof-of-work を計算して
   cookie `cloudpmc-viewer-pow`（5 時間）を立て reload。cookie が無い間は fetch に HTML しか返らない）・
   ScienceDirect の pdfft（JS で pdf.sciencedirectassets.com の署名付き URL へ転送）。出版社名では分岐しない。
   取り直しはタブの**新しい読込が完了するたびに 1 回**、タブの今の URL を元の論文タブ（cookie・referrer。別オリジンの
   署名 URL は content.js が Cookie なしで再試行）→ 拡張の順で fetch し、%PDF が返った時点で終える。
   HTML のまま NAV_QUIET_MS 次へ移らなければ中継ではない（ログイン・購入頁）とみなして早めに諦める。HTTP エラー等はすぐ投げる。
   中身が目的の論文かは /capture/attachment の DOI 照合が決める。PDF が宣言 URL と別オリジンに着いたら `moved` を返し、
   呼び手は照合を厳格（source=link）にする（転送の連鎖だけを同一性の根拠にしない）。成否にかかわらずタブを閉じる。
   応答ヘッダの見張り（watchPdfHeaders）が PDF の到着を検出したら、タブの読込を待たずにその URL を先に取り直す（attachment の PDF =
   Chrome がダウンロードに回してタブは動かない型も、規則で拡張内の空のページへ転送したうえで拾える）。検出が無ければ今の取り直し。
   別オリジンに着いたか（moved）は、検出した URL（無ければタブの URL）のオリジンで決める。
   制限: 見張りが無い（全サイト許可なし・webRequest なし）とき、Content-Disposition: attachment の PDF は Chrome がダウンロードに回す
   （タブは空のまま）。 */
async function fetchPdfViaNavigation(job, url, contextTabId, contextInjected) {
  if (job.cancelled || !(await jobValid(job))) throw staleError(); // 中断・追い越された工程は新しい背景タブを開かない
  progress(job, "PDF を背景タブで開いて、サイトの中継ページの処理を待っています…（" + hostOf(url) + "）");
  const t = await createBackgroundTab(job, url);
  const w = watchTab(t.id);
  let hw = NO_WATCH;
  try {
    await hiddenTab(t.id, true);
    hw = await openWatch(job, t.id, url);
    const deadline = Date.now() + DOI_TAB_TIMEOUT_MS;
    let tried = -1; // 最後に取り直した読込の世代
    let quietSince = 0;
    let lastError = "";
    let quiet = false; // HTML のまま NAV_QUIET_MS 次へ移らなかった（ログイン・購入頁）
    let lastUrl = "";
    let lastKind = null;
    let usedHit = null; // 取り直した検出の URL（同じ URL を 2 回取りに行かない）
    while (Date.now() < deadline) {
      await sleep(500);
      const hit = hw.found();
      if (hit && hit !== usedHit) {
        usedHit = hit;
        try {
          const got = await fetchPdfViaTabThenBg(job, contextTabId, hit, contextInjected);
          return { b64: got.b64, how: "PDF タブ経由・" + got.how, moved: new URL(hit).origin !== new URL(url).origin };
        } catch (e) {
          if (!e.html) throw e;
          // PDF ではなかった（attachment の別のファイル・HTML）。タブは止めた後なので、今の取り直しで待って終える
          lastError = String(e.message || e);
          lastUrl = hit;
          lastKind = e.kind;
          quietSince = Date.now();
          tried = w.gen();
          continue;
        }
      }
      if (w.gen() === tried) {
        if (Date.now() - quietSince >= NAV_QUIET_MS) {
          quiet = true;
          break;
        }
        continue;
      }
      const current = await chrome.tabs.get(t.id);
      if (isStopPage(current.url)) {
        // 規則が止めた（検出に採らなかった attachment・取り直して PDF でなかった物）= タブはもう動かない。検出が後から
        // 届いていれば次の回で取りに行く。拡張内のページの URL は取り直しにもログイン画面の判定にも使わない
        if (hw.found() && hw.found() !== usedHit) continue;
        quiet = true;
        break;
      }
      // 読込中は待つ（中継ページの complete で取り直して HTML なら、次の読込を待つだけ）
      if (current.status !== "complete" || !/^https?:/.test(current.url || "")) continue;
      tried = w.gen();
      try {
        const got = await fetchPdfViaTabThenBg(job, contextTabId, current.url, contextInjected);
        return { b64: got.b64, how: "PDF タブ経由・" + got.how, moved: new URL(current.url).origin !== new URL(url).origin };
      } catch (e) {
        if (!e.html) throw e;
        lastError = String(e.message || e);
        lastUrl = current.url;
        lastKind = e.kind;
        quietSince = Date.now();
      }
    }
    // 失敗の分類: タブや取得の着き先がログイン画面・401 → login（証拠があるときだけ）。HTML のまま動かない → stuck
    // （ログイン・購入・確認のどれかは分からない）。期限切れ（読込を繰り返す）だけなら分類しない
    throw kinded(new Error(
      "PDF タブが PDF へ届きませんでした（中継ページのまま・ログイン / アクセス確認が要る場合はブラウザで PDF を開いてください）" +
        (lastError ? " — " + lastError : "")
    ), OpenRefShared.isLoginUrl(lastUrl, job.proxyTemplate) || lastKind === "login" ? "login" : quiet ? "stuck" : null);
  } finally {
    w.stop();
    try { await chrome.tabs.remove(t.id); } catch (e) { /* already closed */ }
    await hw.stop(); // タブを閉じてから規則を外す（外した後に閉じる前のタブがダウンロードを始めないように）
    await hiddenTab(t.id, false);
  }
}

/* DOI ページを背景タブで開き、着地ページで PDF リンクを探して取得。
   成否にかかわらずタブを閉じる。中継ページ（meta refresh・識別子も PDF の宣言も無いページ = ブラウザ確認や JS の転送）は
   サイト自身の遷移を待って最大 4 回やり直す。宣言された PDF が HTML を返したら fetchPdfViaNavigation で取り直す。 */
async function viaDoiTab(job, key, doi, pmc = null) {
  const route = pmc ? "PMC 本文経由" : "DOI ページ経由";
  // PMC のリンクは対象 PMID の正準の URL だけ（content.js が持つ値と同じ規則 = pdfbytes.js の pmcLinkFor。別の PMID は断る）
  const pmcUrl = pmc ? OpenRefShared.pmcLinkFor(pmc.url, pmc.pmid) : null;
  if (pmc && !pmcUrl) throw new Error("対象 PMID の PMC リンクではありません");
  if (job.cancelled || !(await jobValid(job))) throw staleError(); // 中断・追い越された工程は新しい背景タブを開かない
  progress(job, route + "：背景タブで PDF を探しています…");
  // 前置きがあれば DOI ページは前置き経由で開く（学外から購読の PDF に届くように）。PMC 本文・順番・待ち時間・照合は変えない
  const proxyTemplate = job.proxyTemplate;
  const doiUrl = OpenRefShared.doiUrl;
  const startUrl = pmc ? pmcUrl : (proxyTemplate && OpenRefShared.proxied(proxyTemplate, doiUrl(doi))) || doiUrl(doi);
  const t = await createBackgroundTab(job, startUrl);
  const w = watchTab(t.id);
  const started = Date.now();
  let hw = NO_WATCH;
  try {
    await hiddenTab(t.id, true);
    hw = await openWatch(job, t.id, startUrl);
    let pdfUrl = null;
    // meta | self | atypon | sciencedirect（宣言された所在 = この論文の PDF だけを受ける。link = ページ上の .pdf リンク拾いは本体とは限らない）
    let pdfSource = null;
    let host = "";
    let refresh = null;
    let injected = false;
    let heuristicOnly = false;
    let relay = false; // 最後に見たページが中継ページのまま
    let landing = ""; // 最後に読み込んだ着地ページの URL（失敗の分類のログイン画面の判定に使う）
    for (let i = 0; i < 4; i++) {
      const remain = DOI_TAB_TIMEOUT_MS - (Date.now() - started);
      if (remain <= 0) throw new Error("DOI ページの読込が " + Math.round(DOI_TAB_TIMEOUT_MS / 1000) + " 秒以内に終わりません");
      const url = await waitForTabLoad(t.id, remain, pmc ? NO_WATCH : hw);
      // DOI が直接 PDF に着いた（応答ヘッダの検出）: タブは規則で拡張内のページへ移っているので、タブの URL ではなく検出した
      // URL を PDF の所在にする（取るのは拡張 fetch = 今の「URL が .pdf」の型と同じ）。PMC 本文経由は PMC のページで識別子を
      // 確かめるまで PDF を受けない（検出は使わない）
      const hit = pmc ? null : hw.found();
      if (hit) {
        pdfUrl = hit;
        pdfSource = "self";
        host = hostOf(hit);
        landing = hit;
        injected = false;
        break;
      }
      if (isStopPage(url)) {
        // 規則が止めた応答（PDF として採らなかった attachment）。拡張内のページの URL は行にもログイン画面の判定にも使わない
        throw new Error(pmc ? "PMC 本文の識別子を確認できません（PMC のページがファイルのダウンロードを返しました）"
          : "着地ページが PDF ではないファイル（ダウンロード）を返しました");
      }
      landing = url || "";
      const seen = w.gen(); // この読込の世代。以後に増えたら次のページへ移り始めた
      host = hostOf(url || "");
      try {
        await withTimeout(chrome.scripting.executeScript({ target: { tabId: t.id }, files: ["pdfbytes.js", "content.js"] }), STEP_TIMEOUT_MS, "content script の注入");
        injected = true;
      } catch (e) {
        injected = false;
        if (w.gen() > seen) { // 注入の間に次のページへ移った（中継ページの転送）→ やり直す
          relay = true;
          continue;
        }
        // 注入不可（Chrome の PDF ビューア・エラーページ・chrome:// 等）。URL が .pdf のファイルなら拡張 fetch で取る
        // （見るのは拡張子だけ = pdfbytes.js の hasPdfExtension。/doi/pdf/ 等の形は見ない）
        if (pmc) throw new Error("PMC 本文の識別子を確認できません: " + String(e.message || e));
        if (url && OpenRefShared.hasPdfExtension(url)) {
          pdfUrl = url;
          pdfSource = "self";
          break;
        }
        throw new Error("着地ページ（" + host + "）に content script を注入できません（PDF ビューア / エラーページの可能性: " + String(e.message || e) + "）");
      }
      const a = await withTimeout(sendToTab({ type: "openref-analyze", proxy: proxyTemplate }, t.id), STEP_TIMEOUT_MS, "着地ページの解析");
      if (!a.ok) {
        if (w.gen() > seen) { // 解析の往復中に次のページへ移った（message port closed）→ やり直す
          relay = true;
          continue;
        }
        throw new Error("着地ページ（" + host + "）の解析に失敗: " + a.error);
      }
      const d = a.data;
      relay = !d.pmid && !d.doi && !d.pdfUrl;
      if (!relay) {
        if (pmc && (host !== "pmc.ncbi.nlm.nih.gov" || d.pmid !== pmc.pmid ||
            (doi && d.doi && d.doi.toLowerCase() !== doi.toLowerCase()))) {
          throw new Error("PMC の本文識別子が保存対象と一致しません");
        }
        if (OpenRefShared.declaredPdf(d)) {
          pdfUrl = d.pdfUrl;
          pdfSource = d.pdfSource;
          break;
        }
        if (d.pdfUrl) heuristicOnly = true; // link 由来 = 宣言なし。使わない
      }
      const r = await withTimeout(sendToTab({ type: "openref-refresh-url" }, t.id), STEP_TIMEOUT_MS, "中継判定");
      refresh = r.ok ? r.url : null;
      if (refresh) {
        // 中継ページ（meta refresh）: 飛び先の読込を待ってやり直す
        progress(job, "中継ページ（" + host + "）→ 着地を待っています…");
        await sleep(700); // linkinghub.elsevier.com は "2; url=…" = 2 秒後に飛ぶ
        continue;
      }
      // 識別子も PDF の宣言も無いページ = ブラウザ確認（PMC の reCAPTCHA の確認ページ等）や JS の転送の途中。
      // サイト自身の遷移を待ってやり直す（確認を解く操作はしない。動かなければ下の誠実なエラーで終える）
      const left = DOI_TAB_TIMEOUT_MS - (Date.now() - started);
      if (!relay || i === 3 || left <= 0) break;
      progress(job, "中継ページ（" + host + "）→ 転送を待っています…");
      if (!(await waitUntil(() => w.gen() > seen, Math.min(left, RELAY_WAIT_MS)))) break;
    }
    // 失敗の分類: 着地ページがログイン画面なら login。PMC の確認ページ（reCAPTCHA）のままは challenge。
    // DOI ページの「宣言なし」は、読み込めた着地ページ（中継ページのままではない）のときだけ no_pdf
    const loginLanding = OpenRefShared.isLoginUrl(landing, proxyTemplate);
    if (pmc && relay) {
      throw kinded(new Error("PMC 本文の識別子を確認できません（" + host + " が確認ページ / 中継ページのまま。PMC のページを一度ブラウザで開いてから取り込み直してください）"),
        loginLanding ? "login" : "challenge");
    }
    if (!pdfUrl) {
      throw kinded(new Error(
        "着地ページ（" + host + "）に PDF の宣言（citation_pdf_url 等）がありません" +
          (heuristicOnly ? "（ページ上の .pdf リンクは本体とは限らないため使いません）" : "") +
          (refresh || relay ? "（中継ページのまま）" : "")
      ), loginLanding ? "login" : refresh || relay ? null : "no_pdf");
    }
    progress(job, "PDF を取得中…（" + host + "・認証済み session）");
    let got;
    try {
      got = await fetchPdfViaTabThenBg(job, t.id, pdfUrl, injected);
    } catch (e) {
      if (!e.html) throw e;
      // 宣言された PDF の URL が中継ページを返した（PMC の proof-of-work・ScienceDirect の pdfft 等）
      got = await fetchPdfViaNavigation(job, pdfUrl, t.id, injected);
      if (got.moved) pdfSource = "link"; // 別オリジンに着いた PDF は DOI の一致を必須にする
    }
    return await postPdf(job, key, got.b64, route + "・" + got.how + " fetch・" + host, pdfSource);
  } finally {
    w.stop();
    try {
      await chrome.tabs.remove(t.id);
    } catch (e) { /* 既に閉じている */ }
    await hw.stop(); // タブを閉じてから規則を外す
    hiddenTab(t.id, false);
  }
}

/* ================= 保存の全工程（保存ボタンの後）================= */
async function runSave(job) {
  const page = job.page;
  try {
    let pdfFetchError = null;
    if (OpenRefShared.shouldIngestPdf(page, job.contentScriptOk)) {
      const r = await ingestPdfFlow(job);
      if (r.done) {
        await finish(job, { state: "done", line: r.line, head: r.head, cls: r.cls });
        return;
      }
      pdfFetchError = r.error;
    }

    const ev = page.evidence || null;
    progress(job,
      !page.doi && !page.pmid && ev && ev.title
        ? "識別子なし → ページの書誌メタ（" + ev.title.source + "）で Crossref の候補と照合中…"
        : "書誌を確定中…（PubMed/Crossref）"
    );
    const saved = await toConnectd(job, "/capture/save", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        doi: page.doi,
        pmid: page.pmid,
        title: page.title,
        // 保存される URL（connectd は題名のみの document にだけ使う）はプロキシのホストを戻した出版社の URL
        url: OpenRefShared.plainUrl(page.url, job.proxyTemplate),
        collection_id: job.collectionId ? Number(job.collectionId) : null,
        // ページが自分について宣言した書誌メタ（識別子が無いときの照合の証拠だけ。旧 connectd は知らないキーを無視する）
        evidence: ev,
      }),
    });
    progress(job, { ids: showItem(job, saved) + (saved.status === "existing" ? "（既存）" : "（新規登録）") });

    let pdfMsg = "";
    let why = null; // PDF が付かなかった理由のうち、分類より先に言う物（識別子なし）
    // 層 2 用の DOI: 応答に doi キーがあれば resolver の値（null でも）を正とし、ページ抽出の DOI へは fallback しない
    job.savedDoi = "doi" in saved ? saved.doi || null : page.doi || null;
    job.itemKey = saved.key || null;
    const hasIds = !!(saved.doi || page.doi || page.pmid);
    if (saved.has_pdf) {
      pdfMsg = "PDF は既に添付済み";
      job.ok = true;
    } else if (pdfFetchError !== null) {
      // PDF タブだったが取れなかった（再 fetch はしない）
      pdfMsg = "PDF は取得できませんでした（" + pdfFetchError + "）。DL して OpenRef へドロップしてください";
    } else if (!hasIds) {
      // 識別子なしで本体の PDF があり得るページ（shouldIngestPdf）は ingestPdfFlow で済んでいる（取れなければ上の枝）。
      // ここに来るのはページ上のリンク拾いか PDF なし = 添付しない
      pdfMsg = page.pdfUrl
        ? "ページ上の PDF リンクは論文本体とは限らないため添付しません（識別子なし）。PDF を開いた状態で Capture してください"
        : "PDF リンクなし（識別子なしのため OA 経路は対象外）";
      why = "識別子なし";
    } else {
      pdfMsg = await attachPdf(job, saved.key);
    }
    const pdfOk = job.ok; // PDF が OpenRef にある（下で要確認のために job.ok を落とす前）
    if (!hasIds) job.ok = false; // 識別子の無い保存は要確認（ラベル付き）
    const notes = [];
    // 識別子の無い保存の理由（ページの書誌メタで結べた照合 / 結べず題名のみ + 要確認）。旧 connectd は reason を返さない
    if (saved.reason) notes.push((hasIds ? "" : "要確認ラベルを付けました: ") + saved.reason);
    if (saved.retraction) notes.push("⚠ 撤回論文の可能性: " + saved.retraction);
    await finish(job, doneResult(job, {
      bib: hasIds ? "✓" : BIB_REVIEW,
      key: (saved.status === "existing" ? "既存 " : "登録 ") + saved.key,
      pdfOk, pdfMsg, why, notes,
    }));
  } catch (e) {
    let line = "✗ " + String(e.message || e);
    if (e && e.connectdTimeout) {
      job.failClass = "timeout";
      line += "\n" + CONNECTD_TIMEOUT_NEXT[e.path === "/capture/save" ? "save" : "pdf"];
    }
    await finish(job, errorResult(job, e, line));
  }
}

/* ================= 確認の窓（全サイト許可のある人だけ・利用者がボタンを押したときだけ）=================
   分類が challenge / stuck / login で終わった保存について、止まった段の URL を前面の窓で開き、確認やログインを利用者に済ませて
   もらう。拡張は窓のページを操作しない（content script を入れない・入力しない・クリックしない）。窓のタブに対してするのは、
   応答ヘッダの見張りと DNR の規則（背景タブと同じ範囲）・URL と読込の状態を知ること・閉じることだけ。
   - 自動では開かない（失敗のたびに窓を出さない）。popup のボタン（openref-retry）でだけ始まる。
   - 1 つの工程として走る（同時に 1 件・記録・バッジ・中断・延命・上限・connectd へ送る直前の有効性の確かめ）。新しい item は
     作らず、/capture/save も呼ばない。前の保存の記録が持つ item の key に添付する。
   - 開く先とやり直しの材料は、終わった保存の記録（retry）だけから取る（popup の値や利用者のタブの今の中身から組み立て直さない）。
     前置き（大学プロキシ）は押したときの設定を使う（あれば前置き経由で開き、ログインの後に取り込む）。
   - 終わり方は 2 つ: ①窓に PDF が届く = 見張りが拾い、窓を閉じてから取り直して添付する。②利用者が窓を閉じる = 止まった段を
     1 回だけやり直す（ログインの後に記事のページへ戻すサイト・PMC の確認ページは窓に PDF が届かない）。どちらもボタン 1 回に
     つき 1 回で、繰り返さない。待つのは WINDOW_WAIT_MS まで（過ぎたら窓を閉じ、前の分類と次の一手で終える）。
   - 窓のタブは背景タブと同じ登録（hiddenTab）に入れる: 工程の終わり・中断・service worker の起動時に閉じる。最後のタブを閉じると
     窓も閉じる。走っている工程のタブなので 90 秒の期限では閉じない（background.js の alarm）。
   - 同一性は今と同じ（/capture/attachment の DOI 照合）。宣言の出どころが信じられるのは、ページが宣言した URL と、そこから
     サイトの処理で移った先（利用者が操作しない背景タブ）だけ。窓の中では利用者が自由に移動できるので、届いた PDF を宣言の
     出どころにするのは、宣言された PDF の URL そのもの（前置きのホスト名方式を戻し、クエリと断片を除いてオリジンとパスが一致）が
     届いたときだけ。ほかは同じオリジンでも source=link（DOI の一致が要る）。DOI ページ・PMC の段の窓は、いつも source=link。 */
/* 窓で確認やログインを待つ上限（工程全体の上限 JOB_LIMIT の内側）。機関の SSO・2 段階認証（メールのコード・パスキー）の途中で
   窓を閉じないよう長めに取る。操作に応じて延ばすことはしない（利用者が窓を閉じればすぐ終わり、「この保存を中断」でも止まる） */
const WINDOW_WAIT_MS = 300000;
/* 上限の言い方（分で割り切れれば分・ほかは秒） */
const waitText = (ms) => (ms % 60000 === 0 ? ms / 60000 + " 分" : Math.round(ms / 1000) + " 秒");
const RETRY_STAGES = ["pdf", "pmc", "doi"];
const INCOGNITO_RETRY = "シークレットのタブの保存は窓で確かめられません（窓は通常の窓で開くため、確認やログインが引き継がれません）";
const STALE_RETRY = "この結果はもう新しくありません（別の保存が終わったか、やり直しを始めた後です）。popup を開き直してください";
const WINDOW_DEFAULT = { width: 1000, height: 760 };

/* 分類が止まった段: pdf = ページが宣言した PDF の URL・pmc = PMC 本文（challenge はここでしか出ない）・doi = DOI ページ。
   分類を作った段を先に見て、どれでもなければ（サーバ側の連鎖の 401 など）宣言された PDF → DOI ページの順。開ける所が無ければ null */
function retryStageOf(job, k, c) {
  const page = job.page;
  const declared = OpenRefShared.declaredPdf(page);
  if (k.page === c && declared) return "pdf";
  if (k.pmc === c && page.pmcUrl && page.pmid) return "pmc";
  if (k.doiTab === c && job.savedDoi) return "doi";
  if (declared) return "pdf";
  return job.savedDoi ? "doi" : null;
}

/* 終わった保存の記録に残す、確認の窓の材料（ボタンを出す条件を満たすときだけ。ほかは null = ボタンは出ない） */
function retryInfo(job) {
  if (!job.allSites || !RETRY_CLASSES.includes(job.failClass) || !RETRY_STAGES.includes(job.retryStage) || !job.itemKey) return null;
  const page = job.page;
  return {
    key: job.itemKey,
    stage: job.retryStage,
    cls: job.failClass,
    doi: job.savedDoi || null,
    pmid: page.pmid || null,
    pmcUrl: page.pmcUrl || null,
    pdfUrl: page.pdfUrl || null,
    pdfSource: page.pdfSource || null,
    url: page.url || null,
    documentId: job.tab.documentId,
    contentScriptOk: job.contentScriptOk,
  };
}

/* 窓で開く URL（止まった段の URL。前置きがあれば前置き経由。PMC は前置きを付けない = 層 2 と同じ） */
function windowUrl(job) {
  const S = OpenRefShared;
  const tpl = job.proxyTemplate;
  const stage = job.retry.stage;
  if (stage === "pmc") return job.page.pmcUrl;
  const target = stage === "pdf" ? S.plainUrl(job.page.pdfUrl, tpl) : S.doiUrl(job.savedDoi);
  return (tpl && S.proxied(tpl, target)) || (stage === "pdf" ? job.page.pdfUrl : target);
}

/* 窓の大きさ: popup が知らせた画面の 8 割（受けられない値なら既定）。位置は Chrome に任せる */
function windowSize(size) {
  const ok = (n) => typeof n === "number" && isFinite(n) && n >= 400;
  if (!size || !ok(size.w) || !ok(size.h)) return WINDOW_DEFAULT;
  return { width: Math.round(Math.min(size.w * 0.8, 1400)), height: Math.round(Math.min(size.h * 0.8, 1000)) };
}

/* popup のボタン（openref-retry）: 記録 id の保存が、確認の窓を出せる形で終わっていれば、窓の工程を始める */
async function startRetry(id, size) {
  const rec = await inOrder(loadRecord);
  const r = rec && rec.id === id && rec.state === "done" ? rec.retry : null;
  if (!r || !RETRY_CLASSES.includes(rec.failClass) || !RETRY_STAGES.includes(r.stage) || typeof r.key !== "string" || !r.key) {
    return { ok: false, error: STALE_RETRY };
  }
  // シークレットのタブの保存: 窓は通常のプロファイルで開く（cookie が分かれ、窓で済ませた確認やログインが引き継がれない）= 開かない
  try {
    if ((await chrome.tabs.get(rec.tabId)).incognito) return { ok: false, error: INCOGNITO_RETRY };
  } catch (e) { /* タブが閉じられた = 窓で確かめて元の論文タブ以外（拡張 fetch）から取る */ }
  let proxy = null;
  try {
    proxy = (await chrome.storage.local.get(OpenRefShared.PROXY_KEY))[OpenRefShared.PROXY_KEY] || null;
  } catch (e) { /* 読めない = 前置きなし */ }
  const input = {
    tabId: rec.tabId,
    tabUrl: rec.tabUrl,
    documentId: r.documentId,
    page: { doi: r.doi, pmid: r.pmid, pmcUrl: r.pmcUrl, pdfUrl: r.pdfUrl, pdfSource: r.pdfSource, url: r.url, title: null },
    contentScriptOk: r.contentScriptOk,
    proxy,
  };
  return startJob(input, {
    run: runRetry,
    expectId: id,
    needAllSites: true,
    carry: { title: rec.title, ids: rec.ids },
    line: "確認の窓を開いています…",
    setup: (job) => {
      job.retry = r;
      job.itemKey = r.key;
      job.savedDoi = r.doi || null;
      job.windowSize = windowSize(size);
    },
  });
}

/* 確認の窓の工程（startRetry が startJob に渡す）。結果の行は保存と同じ形（書誌 ✓ ・ PDF … / 既存 <key>） */
async function runRetry(job) {
  try {
    const pdfMsg = await confirmInWindow(job);
    await finish(job, doneResult(job, { bib: "✓", key: "既存 " + job.itemKey, pdfOk: job.ok, pdfMsg }));
  } catch (e) {
    let line = "✗ " + String(e.message || e);
    if (e && e.connectdTimeout) {
      job.failClass = "timeout";
      line += "\n" + CONNECTD_TIMEOUT_NEXT.pdf;
    }
    await finish(job, errorResult(job, e, line));
  }
}

/* 窓の終わりを待つ: found（見張りが PDF の到着を拾った）・closed（窓かタブが閉じられた）・timeout・stopped（中断）の先に来た物 */
function waitWindow(job, tabId, windowId, hw) {
  return new Promise((resolve) => {
    let done = false;
    const end = (v) => {
      if (done) return;
      done = true;
      offFound();
      chrome.tabs.onRemoved.removeListener(onTab);
      if (chrome.windows && chrome.windows.onRemoved) chrome.windows.onRemoved.removeListener(onWin);
      clearTimeout(timer);
      if (job.wake === wake) job.wake = null;
      resolve(v);
    };
    const onTab = (id) => {
      if (id === tabId) end({ how: "closed" });
    };
    const onWin = (id) => {
      if (id === windowId) end({ how: "closed" });
    };
    const wake = () => end({ how: "stopped" });
    const offFound = hw.onFound((url) => end({ how: "found", url }));
    chrome.tabs.onRemoved.addListener(onTab);
    if (chrome.windows && chrome.windows.onRemoved) chrome.windows.onRemoved.addListener(onWin);
    const timer = setTimeout(() => end({ how: "timeout" }), WINDOW_WAIT_MS);
    job.wake = wake;
    if (job.cancelled) wake();
    // 待ちを始める前に閉じられていた
    Promise.resolve(chrome.tabs.get(tabId)).catch(() => end({ how: "closed" }));
  });
}

/* 窓を開いて待ち、終わり方に応じて取り込む。戻り値 = 結果の行の PDF の部分（失敗なら分類と次の一手つき） */
async function confirmInWindow(job) {
  const key = job.itemKey;
  const openUrl = windowUrl(job);
  if (job.cancelled || !(await jobValid(job))) throw staleError();
  // 窓は空のページで開き、見張りを付けてから目的の URL へ移す（開いた直後の応答を取りこぼさない = 背景タブと同じ）
  const win = await chrome.windows.create(Object.assign({ url: "about:blank", type: "popup", focused: true }, job.windowSize || WINDOW_DEFAULT));
  let tabId = win && Array.isArray(win.tabs) && win.tabs[0] ? win.tabs[0].id : null;
  if (typeof tabId !== "number") {
    try {
      const ts = await chrome.tabs.query({ windowId: win.id });
      tabId = ts && ts[0] ? ts[0].id : null;
    } catch (e) { /* 下で閉じる */ }
  }
  if (typeof tabId !== "number") {
    try { await chrome.windows.remove(win.id); } catch (e) { /* 既に閉じている */ }
    throw new Error("開いた窓のタブを確かめられません");
  }
  let hw = NO_WATCH;
  try {
    await hiddenTab(tabId, true);
    if (job.cancelled) throw staleError(); // 登録の前に中断された = 中断の後始末はこの窓を知らない（finally が閉じる）
    progress(job, "窓を開きました（" + hostOf(openUrl) + "）。確認やログインはその窓で済ませ、済んだら窓を閉じてください。PDF が開けばそのまま取り込みます（" +
      waitText(WINDOW_WAIT_MS) + "で窓を閉じます）");
    hw = await openWatch(job, tabId, openUrl);
    if (!canWatchHeaders(job)) await chrome.tabs.update(tabId, { url: openUrl }); // 見張れない（webRequest なし）= 終わり方 ② だけ
    const end = await waitWindow(job, tabId, win.id, hw);
    if (job.cancelled || end.how === "stopped") throw staleError();
    // どの終わり方でも、先に窓を閉じてから見張りを外す（外した後に窓が何かをダウンロードしないように）
    try { await chrome.tabs.remove(tabId); } catch (e) { /* 利用者が閉じた */ }
    await hw.stop();
    if (end.how === "timeout") {
      job.failClass = job.retry.cls; // 前の分類のまま（ボタンはまた出る）
      job.retryStage = job.retry.stage;
      return "書誌は保存済み。窓で " + waitText(WINDOW_WAIT_MS) + "待ちましたが PDF は届きませんでした（窓を閉じました）\n" + NEXT_STEP[job.failClass];
    }
    if (end.how === "found") return await takeWindowPdf(job, key, end.url);
    progress(job, "窓が閉じられました → 止まった段をもう一度試します…");
    return await retryStageOnce(job, key);
  } finally {
    try { await chrome.tabs.remove(tabId); } catch (e) { /* 既に閉じている */ }
    await hw.stop();
    await hiddenTab(tabId, false);
  }
}

/* 窓に届いた PDF（見張りが拾った URL）を取り直して添付する。窓のタブは規則で拡張内の空のページへ移っていて、そこからは
   取れない（ページのオリジンも cookie も無い）ので、元の論文タブ（保存を押したときの文書がまだあれば）→ 拡張 fetch の順。
   窓で済ませたログインの cookie はブラウザの同じ cookie にあるので、どちらからも届く */
async function takeWindowPdf(job, key, hit) {
  progress(job, "PDF が窓に届きました → 取り込みます…（" + hostOf(hit) + "）");
  const kind = job.retry.stage === "pmc" ? "pmc" : job.retry.stage === "doi" ? "doiTab" : "page";
  let got;
  try {
    got = await fetchPdfViaTabThenBg(job, job.tab.id, hit, job.contentScriptOk);
  } catch (e) {
    if (halts(e)) throw e;
    return "書誌は保存済み。窓に届いた PDF を取り込めませんでした — " + String(e.message || e) + classLine(job, { [kind]: kindOf(e) });
  }
  try {
    return await postBytes(job, key, b64ToBytes(got.b64), "窓で確認・" + got.how + " fetch・" + hostOf(hit), windowSource(job, hit),
      job.retry.stage === "pdf" ? "page" : "doi-tab");
  } catch (e) {
    if (halts(e)) throw e;
    return PDF_REFUSED + String(e.message || e) + classLine(job, { [kind]: kindOf(e) });
  }
}

/* 窓に届いた PDF の出どころ。宣言された PDF の段で、届いた URL が宣言された PDF の URL そのもの（前置きのホスト名方式は戻し、
   クエリと断片を除いたオリジンとパスが一致）のときだけ宣言の出どころ。ほか（同じオリジンの別のパス = 窓の中で利用者が開いた
   補足資料・投稿規定・関連論文でありうる・別オリジン・DOI ページ・PMC の段）は link = connectd が DOI の一致を求める */
function windowSource(job, hit) {
  if (job.retry.stage !== "pdf") return "link";
  const where = (u) => {
    try {
      const x = new URL(OpenRefShared.plainUrl(u, job.proxyTemplate));
      return x.origin + x.pathname;
    } catch (e) {
      return null;
    }
  };
  const w = where(hit);
  return w && w === where(job.page.pdfUrl) ? job.page.pdfSource : "link";
}

/* 利用者が窓を閉じた: 止まった段を 1 回だけやり直す（確認やログインが済んでいれば通る）。段は保存の道と同じ物を 1 つだけ並べる。
   PDF の段は窓で開いた URL（前置きがあれば前置き経由 = ログインの cookie はそこにある）を、今と同じ順（ページ内 fetch → 拡張 fetch →
   HTML なら PDF タブ）で取る。やり直しはどの段も拒否で止まり、失敗の行に「誌面で…」を足さない */
function retryStages(job) {
  const stage = job.retry.stage;
  if (stage === "pmc") return [pmcStage()];
  if (stage === "doi") return [doiTabStage({ stopOnRefuse: true })];
  return [pageStage(windowUrl(job), { nav: true, retry: true })];
}
function retryStageOnce(job, key) {
  return runStages(job, key, retryStages(job));
}
