<script lang="ts">
  /* OpenNote: リンクの右クリックのメニュー（状態と項目 = linkmenu.svelte.ts）。App に 1 つ（ページ・分割のプレビュー・チャットの応答で共通）。
     描く位置は右クリックの点・画面の端では内側へ（右端は左へずらす・下端は上へ開く）。
     閉じる = 項目を押した・外側の押下（左 / 中ボタン）・外側の右クリック（contextmenu の capture = ページの受け口より先に閉じ、リンクなら
     そこで開き直す。右ボタンの mousedown では閉じない = WebView が mousedown と contextmenu をどちらの順に送っても、開いたばかりのメニューを閉じない）・
     キー（Esc は奪う = 背後の Esc を走らせない）・窓のフォーカスが外れた・メニューを出した部品を動かすスクロール・窓の大きさの変化 */
  import { composing } from "../input";
  import { closeLinkMenu, linkMenu, type LinkMenuItem } from "./linkmenu.svelte";

  let box: HTMLDivElement | undefined = $state();

  // 画面の端で内側へ（描いた大きさを測ってから）
  $effect(() => {
    const m = linkMenu.cur;
    if (!m || !box) return;
    const r = box.getBoundingClientRect();
    const x = Math.max(4, Math.min(m.x, window.innerWidth - r.width - 4));
    const y = m.y + r.height > window.innerHeight - 4 ? Math.max(4, m.y - r.height) : m.y;
    box.style.left = `${x}px`;
    box.style.top = `${y}px`;
  });

  // 開いている間だけ外側の押下・キー・スクロール等を見張る（capture = 中の部品が止めても届く）
  $effect(() => {
    if (!linkMenu.cur) return;
    const outside = (e: Event) => !box?.contains(e.target as Node);
    const down = (e: MouseEvent) => {
      if (e.button !== 2 && outside(e)) closeLinkMenu();
    };
    const context = (e: MouseEvent) => {
      if (outside(e)) closeLinkMenu();
    };
    const scroll = (e: Event) => {
      const o = linkMenu.cur?.origin;
      const t = e.target;
      if (!o || !(t instanceof Node) || t === document || t.contains(o)) closeLinkMenu();
    };
    const key = (e: KeyboardEvent) => {
      if (composing(e)) return;
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
      }
      closeLinkMenu();
    };
    const close = () => closeLinkMenu();
    window.addEventListener("mousedown", down, true);
    window.addEventListener("contextmenu", context, true);
    window.addEventListener("keydown", key, true);
    window.addEventListener("scroll", scroll, true);
    window.addEventListener("blur", close);
    window.addEventListener("resize", close);
    return () => {
      window.removeEventListener("mousedown", down, true);
      window.removeEventListener("contextmenu", context, true);
      window.removeEventListener("keydown", key, true);
      window.removeEventListener("scroll", scroll, true);
      window.removeEventListener("blur", close);
      window.removeEventListener("resize", close);
    };
  });

  function pick(it: LinkMenuItem) {
    closeLinkMenu();
    void it.run();
  }
</script>

{#if linkMenu.cur}
  {@const m = linkMenu.cur}
  <div class="wk-lm" role="menu" tabindex="-1" bind:this={box} style:left="{m.x}px" style:top="{m.y}px" oncontextmenu={(e) => e.preventDefault()}>
    {#if m.head}
      <div class="wk-lm-head" title={m.head}>{m.head}</div>
    {/if}
    {#each m.items as it, i (i)}
      {#if it}
        <button role="menuitem" onclick={() => pick(it)}>{it.label}</button>
      {:else}
        <div class="wk-lm-sep" role="separator"></div>
      {/if}
    {/each}
  </div>
{/if}

<style>
  .wk-lm {
    position: fixed; z-index: 60; min-width: 180px; max-width: 320px; padding: 4px; border: 1px solid var(--border); border-radius: 8px;
    background: var(--panel); box-shadow: 0 8px 24px rgba(0, 0, 0, 0.18); display: flex; flex-direction: column;
  }
  .wk-lm button { text-align: left; padding: 6px 10px; border-radius: 5px; font-size: 12.5px; color: var(--text); }
  .wk-lm button:hover { background: var(--accent-soft); }
  .wk-lm-head {
    padding: 4px 10px 6px; font-size: 11.5px; color: var(--dim); overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .wk-lm-sep { height: 1px; margin: 4px 6px; background: var(--border); }
</style>
