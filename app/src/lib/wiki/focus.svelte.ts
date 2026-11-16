// OpenNote の集中モード（⌘⌥S・ページのツールバーのボタン）。左の木・右ペイン・OpenNote のヘッダーを一時的に隠し、本文を
// 読みやすい幅で中央に置く（隠すのは App.svelte の class と WikiPage の CSS = 表示だけ・作り直さない = 木と右ペインのスクロール位置と中身を保つ）。表示の切り替えだけ = 右ペインの開閉（togglePane = ui_wiki_info）・
// チャットの柱の開閉（setDockOpen = ui_chat_dock）は呼ばない（設定に書くと戻したときの配置が変わる）。状態は覚えない（再起動で通常）。
// 集中中は ⌘⌥I（右ペイン）と `/`（検索欄へ = ヘッダーを隠している）を効かせない・⌘⌥C（柱）は今どおり

export const focusMode = $state({ on: false });

/** 隠す所（ヘッダー・木・右ペイン）。入るときに、この中にある focus を外す（見えない検索欄が ↓ / Enter / 打った字を受け続けない。
    OpenNote の検索欄は blur で自分の一覧を畳む = 検索の部品を知らずに済む） */
const HIDDEN = ".tabs-page > header, main > .wk-sidebar, main > .wk-info";

export function toggleFocus() {
  if (!focusMode.on && typeof document !== "undefined") {
    const a = document.activeElement as HTMLElement | null;
    if (a?.closest?.(HIDDEN)) a.blur();
  }
  focusMode.on = !focusMode.on;
}
