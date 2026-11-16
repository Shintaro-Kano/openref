/* OpenRef Capture — 設定（options_ui）。
   ①大学プロキシの前置き（実験的）を 1 つだけ chrome.storage.local に持つ。
     検査は pdfbytes.js の OpenRefShared.proxyCheck（popup の proxied と同じ規則）。受けられなければ理由を出して保存しない。
     保存したら送り先のホストを出す。空で保存すると消す = 使わない
   ②全サイト許可のスイッチ。manifest の optional_host_permissions（全サイト）を chrome.permissions.request / remove で
     与える・外す（manifest の宣言は変えない）。スイッチの状態は保存せず、いつも実際の権限（permissions.contains）から読む。
     外したら「拒否した」記録（DECLINED_KEY）を付ける = popup が次の保存で許可を聞き直さない。与えたら記録を消す。
     保存の工程は開始のときに許可を確かめ直す（job.js の startJob）= 変えた許可は次の保存から効く */
"use strict";

const PROXY_KEY = OpenRefShared.PROXY_KEY; // popup・保存の工程と同じキー（pdfbytes.js）
const { ALL_SITES, DECLINED_KEY } = OpenRefShared; // popup の全サイト許可と同じ範囲・同じ拒否の記録
const SAMPLE = OpenRefShared.PROXY_SAMPLE; // 保存したときに見せる例（DOI ページ・前置きの検査が組む例と同じ）

const $ = (id) => document.getElementById(id);
const show = (msg, cls) => {
  $("status").textContent = msg;
  $("status").className = cls || "";
};

async function load() {
  try {
    const v = await chrome.storage.local.get(PROXY_KEY);
    $("proxy").value = typeof v[PROXY_KEY] === "string" ? v[PROXY_KEY] : "";
  } catch (e) {
    show("設定を読めません: " + String(e.message || e), "err");
  }
}

async function save() {
  const t = $("proxy").value.trim();
  try {
    if (!t) {
      await chrome.storage.local.remove(PROXY_KEY);
      show("前置きを消しました（使いません）", "ok");
      return;
    }
    const check = OpenRefShared.proxyCheck(t);
    if (!check.host) {
      show(check.error + "（保存していません）", "err");
      return;
    }
    await chrome.storage.local.set({ [PROXY_KEY]: t });
    $("proxy").value = t;
    show("保存しました。送り先: " + check.host + "\n例: DOI ページ " + SAMPLE + " は次の URL で開きます\n" + OpenRefShared.proxied(t, SAMPLE), "ok");
  } catch (e) {
    show("保存できません: " + String(e.message || e), "err");
  }
}

$("form").addEventListener("submit", (ev) => {
  ev.preventDefault();
  return save();
});

const showSites = (msg, cls) => {
  $("allsites-status").textContent = msg;
  $("allsites-status").className = cls || "";
};
/* スイッチを実際の権限に合わせる（読めなければ押せないまま） */
async function loadAllSites() {
  try {
    $("allsites").checked = !!(await chrome.permissions.contains(ALL_SITES));
    $("allsites").disabled = false;
  } catch (e) {
    $("allsites").disabled = true;
    showSites("全サイトへのアクセス許可を確かめられません: " + String(e.message || e), "err");
  }
}
/* スイッチを押した: オン = 許可を求める（押した直後 = ユーザー操作のうちに request する。前に await を挟まない）・オフ = 外す。
   どちらも終わったら実際の権限から読み直す（拒否された・外せなかった場合はスイッチが元に戻る） */
async function toggleAllSites() {
  const want = $("allsites").checked;
  $("allsites").disabled = true;
  try {
    if (want) {
      if (await chrome.permissions.request(ALL_SITES)) {
        await chrome.storage.local.remove(DECLINED_KEY);
        showSites("許可しました。次の保存から、PDF が取れないとき背景タブで探します", "ok");
      } else {
        showSites("許可されませんでした（変えていません）", "err");
      }
    } else if (await chrome.permissions.remove(ALL_SITES)) {
      await chrome.storage.local.set({ [DECLINED_KEY]: true });
      showSites("許可を外しました。次の保存から背景タブを使いません（保存のときにも聞きません）", "ok");
    } else {
      showSites("許可を外せませんでした", "err");
    }
  } catch (e) {
    showSites("変えられません: " + String(e.message || e), "err");
  }
  await loadAllSites();
}
$("allsites").addEventListener("change", () => toggleAllSites());
// chrome://extensions など、ほかの所で許可が変わったときもスイッチを合わせる
try {
  chrome.permissions.onAdded.addListener(() => loadAllSites());
  chrome.permissions.onRemoved.addListener(() => loadAllSites());
} catch (e) { /* 古い Chrome = 開いたときの値だけ */ }

load();
loadAllSites();
