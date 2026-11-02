// OpenNote の検索の動詞（tag の切替・消す・結果を開く）。wiki.svelte.ts から移しただけ（B-33）。
// 検索の本体（hasQuery / searchSoon / runSearch と debounce・世代）は wiki.svelte.ts に残す = 監視の読み直し（initWiki の wiki-changed）と
// その解除が使う（ここへ移すと wiki.svelte.ts → ここの import = 循環）。ここは wiki.svelte.ts を読む一方向

import { openWikiPage, searchSoon, wk } from "./wiki.svelte";

/** tag の選択を切り替える（Tags 箱・結果の tag）。選ぶと結果一覧を出す */
export function toggleTag(tag: string) {
  const low = tag.toLowerCase();
  const i = wk.qTags.findIndex((x) => x.toLowerCase() === low);
  if (i >= 0) wk.qTags.splice(i, 1);
  else wk.qTags.push(tag);
  searchSoon(0);
}

/** 検索を消して木（ページ）に戻る（Esc・クリア） */
export function clearSearch() {
  wk.q = "";
  wk.qTags = [];
  searchSoon(0);
}

/** 結果のページを開いて結果一覧を畳む（条件は残す = 検索欄に戻ると再び出る） */
export async function openHit(path: string, newTab = false) {
  if (!newTab) wk.showResults = false;
  await openWikiPage(path, newTab);
}
