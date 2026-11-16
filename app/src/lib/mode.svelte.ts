// モードの切替 = 左バーの最上段のセグメント [LIBRARY | NOTE | SHELF]（ModeSegment）の押下。
// タブの中身は常に 1 つ（一覧 / OpenNote / 本棚）= 前面のタブの中身を替える（PDF・本の読み手のタブからは新しいタブ = 中身を変えない）。
// 各モードで見せていた物は、別のモードへ行っても持ったまま、戻すとそこへ戻る:
//   - LIBRARY = 一覧のビュー・検索語・絞り込み・選択は app.* に持ったまま・位置は tab.scrollTop（leaveListForNote / resumeList）。
//     消えたフォルダ / ラベルなら「すべて」
//   - SHELF = tab.shelfAway に本棚（root・フォルダ・並び・検索語・位置）を覚える（NOTE へ行っても LIBRARY へ行っても）。
//     ui_tabs に書くのは今までどおり OpenNote のタブの物だけ（一覧のタブの戻る先は、その起動の間だけ）
//   - NOTE = 今までどおり毎回ホーム（ワークスペースは rootForNew = 最後に前面だった OpenNote の物）
// 左バーの幅はモードごと（SidebarGrip・ui_sidebar_w）= ここは触らない
import { app, clearChecks, leaveListForNote, openTab, resumeList, shelfHome, tabOpsAllowed, type Tab } from "./state.svelte";
import type { ShelfTab } from "./shelf/tree";
import { shelfTarget } from "./shelf/shelf.svelte";
import { canLeave, rootForNew } from "./wiki/wiki.svelte";

export type Mode = "library" | "wiki" | "shelf";

/** タブ t のモード（OpenNote が無効なら Wiki のタブも LIBRARY = frontKind と同じ）。PDF・本の読み手のタブは null（セグメントは裏にある） */
function modeOf(t: Tab): Mode | null {
  if (t.viewer || t.shelfBook) return null;
  if (t.wiki && app.wikiOn) return "wiki";
  return t.shelf ? "shelf" : "library";
}

/** 前面のタブのモード（セグメントの押された側）。PDF・本の読み手のタブは LIBRARY（下は一覧） */
export function frontMode(): Mode {
  const t = app.tabs.find((x) => x.id === app.tab);
  return (t && modeOf(t)) ?? "library";
}

const wikiHome = () => ({ path: null, root: rootForNew(), scrollTop: 0, editing: false });

/** セグメントの押下。処理中・覆い（窓・エディタ・取込）の間はタブの切替と同じく受け付けない。
    SHELF の行き先 = 覚えた本棚 → 履歴の新しい本棚 → 本棚が無ければ「別のフォルダを本棚として開く…」の選択（取り消し = 何も変えない） */
export async function switchMode(to: Mode) {
  const id = app.tab;
  if (!tabOpsAllowed()) return; // 覆いの下の押下は何もしない = 処理中の押下も取り消さない
  // 同じタブで同じ行き先を処理中（フォルダの選択の窓が開くまで・走査・保存を待つ間）の二度押しは無視（選択の窓が 2 回開いた）
  if (pending.get(id)?.to === to) return;
  const me = { to };
  pending.set(id, me); // 同じタブで後から押した物が勝つ = 前の押下は待ちから戻っても何もしない（遅れて返った SHELF で画面を奪わない）
  try {
    await switchNow(to, () => pending.get(id) === me);
  } finally {
    if (pending.get(id) === me) pending.delete(id);
  }
}
/** タブ id → 処理中の押下（印を 1 つにすると、遅い SHELF の間の NOTE / LIBRARY や別のタブの操作まで飲み込んだ）。
    印はタブごと = 別のタブの操作は妨げない */
const pending = new Map<number, { to: Mode }>();

/** live = この押下がまだ同じタブの最新か（後から押した物があれば false = 何も変えない） */
async function switchNow(to: Mode, live: () => boolean) {
  const tab = app.tabs.find((x) => x.id === app.tab);
  if (!tab || !tabOpsAllowed() || (to === "wiki" && !app.wikiOn)) return;
  const from = modeOf(tab);
  if (from === to) return;
  // 行き先の本棚を先に決める（フォルダの選択を取り消した・切り替えられない = 元のモードのまま）
  let shelf: ShelfTab | null = null;
  if (to === "shelf") {
    shelf = await shelfTarget(from === null ? null : tab);
    if (!shelf || !live() || app.tab !== tab.id || modeOf(tab) !== from) return;
  }
  if (from === null) {
    // PDF・本の読み手のタブ = 新しいタブ（下の一覧は「すべて」）
    await openTab({ kind: "all" }, to === "wiki" ? wikiHome() : null, shelf);
    return;
  }
  // 離れる前: OpenNote = 編集中の未保存を保存（できなければ移らない）/ 一覧 = メモを保存して閉じ・位置を覚える（タブの切替と同じ）
  if (from === "wiki" && !(await canLeave())) return;
  if (from === "library" && !(await leaveListForNote(tab))) return;
  if (!live() || app.tab !== tab.id || modeOf(tab) !== from) return; // 保存を待つ間に後から押された・別のタブへ移った・中身が変わった = 効かせない
  if (from === "shelf") tab.shelfAway = tab.shelf;
  tab.shelf = null;
  tab.wiki = null;
  if (to === "wiki") tab.wiki = wikiHome(); // 前面は OpenNote でない = 最後の OpenNote の root（画面の root）= 写しはそのまま
  else if (to === "shelf") {
    tab.shelf = shelf;
    tab.shelfAway = null;
    clearChecks(); // 本棚へ入る = 一覧の選択を外す（文脈のチップが見えない文献を指さない = 見出しのメニューから入るときと同じ）
  } else await resumeList(tab);
}
