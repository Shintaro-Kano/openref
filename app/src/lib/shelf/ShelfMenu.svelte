<script lang="ts">
  /* 見出しのメニュー: ✓ 文献 / 本棚の履歴 / 別のフォルダを本棚として開く… / 本棚のとき collapse・Finder・履歴から外す。
     文献はフォルダを選ばない = 1 クリック。無いフォルダは灰色で押せない（外すのはその本棚を開いている間だけ = 無いフォルダのタブは開ける） */
  import { untrack } from "svelte";
  import { t } from "../i18n.svelte";
  import { app, frontShelf } from "../state.svelte";
  import { backToLibrary, forgetShelf, pickShelf, revealBook, setCollapse, sh, switchShelf } from "./shelf.svelte";

  const s = $derived(frontShelf());
  const cur = $derived(s ? sh.shelves.find((x) => x.root === s.root) : undefined);

  // タブが変わったら閉じる（他のメニューは switchTab が閉じる。state は shelf.svelte を import できない = 循環）。
  // 残すと PDF のタブの上にメニューが残り、そのタブの中身に本棚を足す操作になる
  $effect(() => {
    app.tab;
    untrack(() => (sh.menu = null));
  });

  function onKey(e: KeyboardEvent) {
    if (e.key !== "Escape") return;
    e.preventDefault();
    e.stopPropagation(); // App の Esc（チェックの解除等）に渡さない
    sh.menu = null;
  }
</script>

<svelte:window onclick={() => (sh.menu = null)} onkeydowncapture={(e) => sh.menu && onKey(e)} />

{#if sh.menu}
  <div class="lmenu sh-menu" style="top:{sh.menu.y}px;left:{sh.menu.x}px" role="menu" tabindex="-1" onclick={(e) => e.stopPropagation()} onkeydown={() => {}}>
    <button class="lmenu-item" role="menuitemradio" aria-checked={!s} onclick={() => void backToLibrary()}>
      <span><span class="lmenu-check">{s ? "" : "✓"}</span>{t("文献")}</span>
    </button>
    <div class="lmenu-sep"></div>
    <div class="sh-menu-label">{t("本棚")}</div>
    {#each sh.shelves as r (r.root)}
      <button
        class="lmenu-item"
        class:sh-menu-gone={!r.exists}
        role="menuitemradio"
        aria-checked={s?.root === r.root}
        title={r.exists ? r.root : `${r.root} — ${t("見つかりません")}`}
        disabled={!r.exists}
        onclick={() => void switchShelf(r.root)}>
        <span class="sh-menu-name"><span class="lmenu-check">{s?.root === r.root ? "✓" : ""}</span>{r.name}</span>
      </button>
    {/each}
    <button class="lmenu-item" onclick={() => void pickShelf()}>
      <span><span class="lmenu-check"></span>{t("別のフォルダを本棚として開く…")}</span>
    </button>
    {#if s && cur}
      {@const root = s.root}
      <div class="lmenu-sep"></div>
      <button class="lmenu-item" role="menuitemcheckbox" aria-checked={cur.collapse_single} onclick={() => void setCollapse(root, !cur.collapse_single)}>
        <span><span class="lmenu-check">{cur.collapse_single ? "✓" : ""}</span>{t("PDF が 1 つだけのフォルダを 1 冊として表示")}</span>
      </button>
      <button class="lmenu-item" onclick={() => revealBook(root)}>
        <span><span class="lmenu-check"></span>{t("Finder で表示")}</span>
      </button>
      <button class="lmenu-item lmenu-danger" onclick={() => void forgetShelf(root)}>
        <span><span class="lmenu-check"></span>{t("この本棚を履歴から外す")}</span>
      </button>
    {/if}
  </div>
{/if}

<style>
  .lmenu.sh-menu { width: 330px; max-height: calc(100vh - 80px); overflow-y: auto; }
  .sh-menu-label { padding: 2px 8px 2px 30px; font-size: 11px; color: var(--dim); }
  .sh-menu-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }
  .lmenu-item.sh-menu-gone { color: var(--dim); opacity: 0.55; cursor: default; }
  .lmenu-item.sh-menu-gone:hover { background: transparent; }
</style>
