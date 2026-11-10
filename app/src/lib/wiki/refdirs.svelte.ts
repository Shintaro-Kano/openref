// OpenNote の参照フォルダの追加 / 外す（WikiInfo・WikiDialogs）。wiki.svelte.ts から移しただけ。
// 一覧の読込（loadRefDirs）と判定（isRefPath）は wiki.svelte.ts に残す = 起動・状態の読込・wiki-state が使う（ここへ移すと循環）。
// ここは wiki.svelte.ts を読む一方向。参照フォルダは root ごと = 画面の root の物を足す / 外す（args.root）。
// 待つ間（フォルダ選択のダイアログ）に画面の root が変わったら、返った一覧は頼んだ root の物 = 画面には入れない（戻ったときに読み直す）

import { invoke } from "./ipc";
import { t } from "../i18n.svelte";
import { toast } from "../state.svelte";
import { shownRoot, wk } from "./wiki.svelte";

/** [追加…] = Rust のフォルダ選択ダイアログ → 検査して設定に足す（取り消しは何もしない）。
    set_aside = 設定が解釈できなかったので Rust が元の値をその鍵に退避した（toast 1 つにまとめて知らせる） */
export async function addRefDir() {
  const root = shownRoot();
  try {
    const r = await invoke<{ dirs: string[] | null; added: string | null; set_aside?: string | null }>("wiki_ref_dir_add", { args: root ? { root } : {} });
    if (Array.isArray(r.dirs) && root === shownRoot()) wk.refDirs = r.dirs;
    if (r.added && r.set_aside) {
      toast(`⚠ ${t("参照フォルダを追加しました")}: ${r.added} (${t("元の設定は解釈できなかったので退避しました")}: ${r.set_aside})`, 10000);
    } else if (r.added) {
      toast(`✓ ${t("参照フォルダを追加しました")}: ${r.added}`);
    }
  } catch (e) {
    toast(`✗ ${t("参照フォルダを追加できません")}: ${String(e).slice(0, 200)}`, 8000);
  }
}

/** [外す]。配信（asset scope）は再起動まで続くが、リンクはすぐ外部で開くようになる */
export async function removeRefDir(path: string) {
  const root = shownRoot();
  try {
    const r = await invoke<{ dirs: string[]; restart: boolean }>("wiki_ref_dir_remove", { args: root ? { path, root } : { path } });
    if (root === shownRoot()) wk.refDirs = r.dirs ?? [];
    toast(r.restart ? t("参照フォルダを外しました（配信を完全に止めるには OpenRef を再起動してください）") : t("参照フォルダを外しました"), 6000);
  } catch (e) {
    toast(`✗ ${t("参照フォルダを外せません")}: ${String(e).slice(0, 200)}`, 8000);
  }
}
