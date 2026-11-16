/* OpenRef Capture — service worker。
   ①保存の工程（job.js）: popup の保存ボタンが openref-save を送ると、ここで工程を走らせる（popup を閉じても続く）。
     popup は開いたときに openref-job で今の記録を読む（この service worker を起こす = 起動時の直しが先に済む）。
   ②拡張 fetch（bgFetchPdf）: ページ内 fetch（content script）が CORS 等で失敗した時の fallback。
     activeTab 権限（または背景タブ経路で付与された全サイト権限）により、当該オリジンに fetch できる。
     service worker の fetch はページオリジンの CORS を受けない（content script の fetch は受ける）。
   ③工程が開いた背景タブの後始末（hiddenTab で登録）。
     タブ id を chrome.storage.session に持ち、(a) この SW の起動時（前の SW の工程はメモリと一緒に消えている = 残りは孤児）
     (b) alarms による期限（90 秒）で閉じる。期限が来ても、この SW で工程が走っていれば閉じずに期限を延ばす（工程の背景タブは
     20 + 120 + 140 秒に達しうる。工程の終わり・中断・SW 起動時の後始末が閉じる）。期限で閉じるのは、工程の居ない孤児だけ。
     閉じるのは **登録済みの id だけ**（ブラウザ再起動で tab id が振り直されても user のタブを閉じない — storage.session は
     ブラウザ再起動で消えるので登録も消える）。工程が走っている間は job.js の延命が SW を止めない。
   ④工程の上限の alarm（job.js の JOB_ALARM）: SW が止まっていても上限の時刻に起こす（起動時の直しで記録を中断にする）。
   ⑤工程が背景タブに入れた DNR のセッション規則（job.js の watchPdfHeaders）: SW が止まっても残るので、起動時に全部外す。
   ⑥確認の窓（job.js の startRetry）: 窓のタブも③の登録に入れる（工程の終わり・中断・SW の起動時に閉じる = 最後のタブを閉じると窓も閉じる）。 */
"use strict";

importScripts("pdfbytes.js", "job.js"); // OpenRefShared（content.js と同じ規則）・保存の工程

const TAB_KEY = "openref_hidden_tabs"; // {tabId: openedAt(ms)}
const TAB_TTL_MS = 90 * 1000;
const ALARM_PREFIX = "openref-hidden-tab-"; // + tabId

/* storage.session の読み書きは 1 本の直列キューで行う（onRemoved / onAlarm / onMessage が同時に走っても
   read-modify-write が交差しない = 単一の書き手） */
let queue = Promise.resolve();
function serialized(fn) {
  const run = queue.then(fn);
  queue = run.catch(() => {});
  return run;
}

async function loadTabs() {
  try {
    const v = await chrome.storage.session.get(TAB_KEY);
    return v[TAB_KEY] || {};
  } catch (e) {
    return {};
  }
}
async function saveTabs(tabs) {
  try {
    await chrome.storage.session.set({ [TAB_KEY]: tabs });
  } catch (e) { /* session storage 不可でも alarms で閉じる */ }
}
function registerTab(tabId) {
  return serialized(async () => {
    const tabs = await loadTabs();
    tabs[tabId] = Date.now();
    await saveTabs(tabs);
    try {
      chrome.alarms.create(ALARM_PREFIX + tabId, { when: Date.now() + TAB_TTL_MS });
    } catch (e) { /* alarms 不可 */ }
  });
}
/* 登録を外す。close=true のときは **登録されていた場合だけ** タブを閉じる */
function unregisterTab(tabId, close) {
  return serialized(async () => {
    const tabs = await loadTabs();
    const wasRegistered = Object.prototype.hasOwnProperty.call(tabs, String(tabId));
    if (wasRegistered) {
      delete tabs[tabId];
      await saveTabs(tabs);
    }
    try {
      chrome.alarms.clear(ALARM_PREFIX + tabId);
    } catch (e) { /* noop */ }
    if (close && wasRegistered) chrome.tabs.remove(Number(tabId)).catch(() => {});
  });
}
/* 登録済みの背景タブを全部閉じる。戻り値の Promise はタブを閉じ終えてから解決する（工程の DNR 規則 = ダウンロード止めを
   外すのはその後 = 閉じる前の背景タブの attachment が利用者のダウンロードにならないように） */
function closeAllRegistered() {
  return serialized(async () => {
    const tabs = await loadTabs();
    await saveTabs({});
    const closing = [];
    for (const id of Object.keys(tabs)) {
      try {
        chrome.alarms.clear(ALARM_PREFIX + id);
      } catch (e) { /* noop */ }
      closing.push(Promise.resolve(chrome.tabs.remove(Number(id))).catch(() => {}));
    }
    await Promise.all(closing);
  });
}
/* 工程（job.js）が開いた背景タブの登録（open = true）と解除。SW の中なのでメッセージを通さず直接呼ぶ */
function hiddenTab(tabId, open) {
  return open ? registerTab(tabId) : unregisterTab(tabId, false);
}

// (a) SW 起動時: 前の SW の工程が開いたまま残した背景タブを閉じ、「走行中」のまま残った記録を「中断」に直す。
//     工程が背景タブに入れた DNR のセッション規則（ダウンロード止め）は service worker が止まっても残るので、ここで全部外す
removeAllPdfRules(closeAllRegistered()); // 背景タブを閉じ終えてから規則を外す（起動直後の工程はこの後始末の後に規則を入れる）
recoverRecord(SW_RESTART);

// (b) 期限（登録済みの id だけ閉じる）。走っている工程の背景タブは閉じずに期限を延ばす
chrome.alarms.onAlarm.addListener((alarm) => {
  if (alarm.name === JOB_ALARM) {
    onJobAlarm();
    return;
  }
  if (!alarm.name.startsWith(ALARM_PREFIX)) return;
  if (jobRunning()) {
    try {
      chrome.alarms.create(alarm.name, { when: Date.now() + TAB_TTL_MS });
    } catch (e) { /* alarms 不可 */ }
    return;
  }
  unregisterTab(alarm.name.slice(ALARM_PREFIX.length), true);
});

// 背景タブが（ユーザー操作等で）閉じられたら登録を外す（閉じない）
chrome.tabs.onRemoved.addListener((tabId) => {
  unregisterTab(tabId, false);
});


/* SW 側の 1 回追従は **meta refresh だけ**（content.js は加えて「同一オリジンの .pdf パスがちょうど 1 つ」も見る。
   SW には DOMParser が無く同じ規則を忠実に再現できないので、規則を狭めて差を無くす方を選んだ）。
   meta refresh の規則そのもの（コメント・noscript を無視・delay > 10 秒は追わない）は pdfbytes.js の 1 つを content.js と共有する。 */
function nextPdfUrlFromHtml(html, base) {
  return OpenRefShared.metaRefreshUrl(html, base);
}

async function fetchPdf(url, follow, hopped) {
  const r = await fetch(url, { credentials: "include" });
  if (!r.ok) {
    // HTTP の状態と転送の着き先は保存の工程（job.js）の失敗の分類が使う（content.js と同じ）
    throw Object.assign(new Error("HTTP " + r.status), { httpStatus: r.status, url: r.url || url });
  }
  const buf = await r.arrayBuffer();
  const b = new Uint8Array(buf);
  if (!OpenRefShared.isPdfBytes(b)) { // %PDF の判定は content.js と同じ 1 つ（pdfbytes.js）
    if (follow && !hopped) {
      const html = new TextDecoder().decode(buf.slice(0, 2 * 1024 * 1024));
      const next = nextPdfUrlFromHtml(html, r.url || url);
      if (next && next !== url) return fetchPdf(next, true, true);
    }
    const err = new Error("PDF ではない応答（" + buf.byteLength + " bytes" + (hopped ? "・中継先も HTML" : "") + "）");
    err.html = true; // 保存の工程（job.js）が「PDF タブで開いて取り直す」経路に回す目印（content.js と同じ）
    err.url = r.url || url;
    throw err;
  }
  return OpenRefShared.bytesToB64(b);
}

/* 拡張 fetch（工程が直接呼ぶ）。戻り値は content.js の openref-fetch-pdf の応答と同じ形
   {ok: true, b64} か {ok: false, error, html, httpStatus, url}（html・httpStatus・url は失敗の分類が使う） */
function bgFetchPdf(url, follow) {
  return fetchPdf(url, !!follow, false).then(
    (b64) => ({ ok: true, b64 }),
    (e) => ({ ok: false, error: String(e), html: !!e.html, httpStatus: e.httpStatus || 0, url: e.url || null })
  );
}

/* 受けるのは拡張自身のページ（popup）からのメッセージだけ（content script = sender.tab のあるものは受けない） */
const fromExtensionPage = (sender) =>
  !!sender && sender.id === chrome.runtime.id && !sender.tab && typeof sender.url === "string" && sender.url.startsWith(chrome.runtime.getURL(""));

chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
  if (!msg || !fromExtensionPage(sender)) return false;
  // openref-save = 保存ボタン・openref-retry = 確認の窓のボタン（記録 id の保存を窓で確かめて取り込み直す。材料は記録から取る）
  if (msg.type === "openref-save" || (msg.type === "openref-retry" && typeof msg.id === "string")) {
    (msg.type === "openref-save" ? startJob(msg.input) : startRetry(msg.id, msg.size)).then(
      (r) => sendResponse({ ok: r.ok, id: r.id || null, busy: !!r.busy, error: r.error || null }),
      (e) => sendResponse({ ok: false, error: String((e && e.message) || e) })
    );
    return true; // async
  }
  // openref-job = 今の記録・openref-abort = 走っている保存の中断（popup の「中断」）・openref-seen = 結果を表示した
  const answer = msg.type === "openref-job" ? currentRecord()
    : msg.type === "openref-abort" && typeof msg.id === "string" ? abortById(msg.id)
    : msg.type === "openref-seen" && typeof msg.id === "string" ? markSeen(msg.id)
    : null;
  if (!answer) return false;
  answer.then(
    (record) => sendResponse({ ok: true, record }),
    (e) => sendResponse({ ok: false, error: String((e && e.message) || e) })
  );
  return true; // async
});
