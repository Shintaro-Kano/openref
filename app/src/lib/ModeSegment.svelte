<script lang="ts">
  /* モードのセグメント [LIBRARY | NOTE | SHELF]（3 つの左バーの最上段）。OpenNote が無効なら [LIBRARY | SHELF]。
     呼び名は固有の名前 = 日本語 / 英語の表示で同じ（t() を通さない）。押下の規則は mode.svelte.ts の switchMode
     （本棚が 1 つも無ければ SHELF = フォルダを選んで本棚にする・取り消し = 元のモードのまま）。
     幅は字の長さに応じて分ける（flex-basis: auto = 3 つでも既定の幅 230 で LIBRARY が切れない・余りは等分）。
     それでも入りきらない狭さ（最小の 180）は省略（…）にして title に全部（左バーの行の名前と同じ作法） */
  import { t } from "./i18n.svelte";
  import { app } from "./state.svelte";
  import { frontMode, switchMode, type Mode } from "./mode.svelte";
  import { sh } from "./shelf/shelf.svelte";

  const cur = $derived(frontMode());
  const modes = $derived<[Mode, string][]>([["library", "LIBRARY"], ...(app.wikiOn ? [["wiki", "NOTE"] as [Mode, string]] : []), ["shelf", "SHELF"]]);
  const title = (m: Mode, name: string) => (m === "shelf" && !sh.shelves.length ? `${name} — ${t("本棚はまだありません（押すと、フォルダを選んで本棚として開きます）")}` : name);
</script>

<div class="mode-seg" role="group" aria-label={t("表示の切替")}>
  {#each modes as [m, name] (m)}
    <button class="mode-seg-btn" class:mode-on={cur === m} aria-pressed={cur === m} title={title(m, name)} onclick={() => void switchMode(m)}><span class="mode-seg-name">{name}</span></button>
  {/each}
</div>

<style>
  .mode-seg {
    display: flex; gap: 0; margin: 0 2px 8px; padding: 2px; border: 1px solid var(--border); border-radius: 7px; background: var(--bg);
    flex-shrink: 0;
  }
  .mode-seg-btn {
    flex: 1 1 auto; width: auto; min-width: 0; justify-content: center; padding: 4px 0; border: 0; border-radius: 5px; background: transparent;
    display: flex; font-size: 11px; font-weight: 600; letter-spacing: 0.3px; color: var(--dim); cursor: pointer;
  }
  /* width: auto = 左バーの .ls-nav button の width: 100%（3 つとも同じ幅に縮む）を外す。字は中の span で省略する
     （.ls-nav button は display: flex = ボタン自身の text-overflow は効かない） */
  .mode-seg-name { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .mode-seg-btn:hover { color: var(--text); background: transparent; }
  .mode-seg-btn.mode-on { background: var(--panel); color: var(--accent); box-shadow: 0 0 0 1px var(--border); }
</style>
