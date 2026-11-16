<script lang="ts">
  /* OpenNote の文献の引用の窓（[文献]・⌘⌥K・「/」の「文献を引用…」）。蔵書を探して、選んだ文献をエディタの選択 / caret に
     `[選択](openref://open?key=KEY)`（選択なしは `[](…)`）で入れる。状態と検索は cite.svelte.ts。
     ↑↓ = 候補・Enter = 入れる・Esc / 外側の押下 = 閉じる（IME の確定 Enter・取消 Esc は入力欄の物）。
     窓を開いたときの編集から離れたら（別のページ・別のワークスペース・編集を閉じた）閉じる */
  import { t } from "../i18n.svelte";
  import { composing } from "../input";
  import { chooseCite, cite, closeCite, setCiteQuery } from "./cite.svelte";
  import { ed, focusEditor } from "./edit.svelte";
  import { moveTab, trapTab } from "../focusTrap";

  let input: HTMLInputElement | undefined = $state();
  let list: HTMLUListElement | undefined = $state();

  // WKWebView では autofocus が効かないことがある = 明示
  $effect(() => {
    input?.focus();
  });

  // 編集から離れた = 閉じる（入れる先が無い）
  $effect(() => {
    if (ed.path !== cite.path || ed.root !== cite.root) closeCite();
  });

  function move(d: number) {
    const n = cite.results.length;
    if (!n) return;
    cite.sel = (cite.sel + d + n) % n;
    list?.children[cite.sel]?.scrollIntoView({ block: "nearest" });
  }

  function cancel() {
    closeCite();
    focusEditor();
  }

  function onKey(e: KeyboardEvent) {
    // 窓の中のキーは窓の物（⌘ のキー = 保存・タブ操作は素通し）
    if (!e.metaKey) e.stopPropagation();
    if (composing(e)) return;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      move(e.key === "ArrowDown" ? 1 : -1);
    } else if (e.key === "Enter") {
      e.preventDefault();
      const it = cite.results[cite.sel];
      if (it) chooseCite(it);
    } else if (e.key === "Escape") {
      e.preventDefault();
      cancel();
    } else if (e.key === "Tab" && !e.metaKey && !e.ctrlKey && !e.altKey) {
      // 窓の中を回る: 上の stopPropagation で骨格の受け手（window）に届かない = 同じ moveTab を直に呼ぶ（WikiLightbox と同じ）
      moveTab(e, e.currentTarget as HTMLElement);
    }
  }

  const authorLine = (it: (typeof cite.results)[number]) =>
    [it.firstAuthor, it.year != null ? String(it.year) : "", it.container].filter(Boolean).join(" · ");
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="wk-cite-back" onmousedown={(e) => e.target === e.currentTarget && cancel()}>
  <div class="wk-cite" role="dialog" aria-modal="true" aria-label={t("文献を引用")} tabindex="-1" onkeydown={onKey} use:trapTab>
    <input
      bind:this={input}
      class="wk-cite-input"
      placeholder={t("題名・著者・誌名・年・DOI・PMID")}
      value={cite.query}
      oninput={(e) => {
        if (!composing(e as unknown as InputEvent)) setCiteQuery((e.currentTarget as HTMLInputElement).value);
      }}
      oncompositionend={(e) => setCiteQuery((e.currentTarget as HTMLInputElement).value)}
    />
    <p class="wk-cite-hint">
      {#if cite.selText.trim()}
        {t("選んだ文字をリンクの文字にします")}: <b>{cite.selText.length > 40 ? cite.selText.slice(0, 40) + "…" : cite.selText}</b>
      {:else}
        {t("選択なし: 文字の無い文献リンク（番号だけの引用）を入れます")}
      {/if}
    </p>
    {#if cite.results.length}
      <ul class="wk-cite-list" bind:this={list} role="listbox">
        {#each cite.results as it, i (it.key)}
          <li role="option" aria-selected={i === cite.sel}>
            <button
              type="button"
              class="wk-cite-row"
              class:wk-cite-on={i === cite.sel}
              onmouseenter={() => (cite.sel = i)}
              onmousedown={(e) => e.preventDefault()}
              onclick={() => chooseCite(it)}
            >
              <span class="wk-cite-title">
                {#if it.retracted}<span class="wk-cite-retracted" title={t("撤回の印がある文献です")}>{t("撤回")}</span>{/if}{it.title}
              </span>
              <span class="wk-cite-meta">{authorLine(it)}</span>
            </button>
          </li>
        {/each}
      </ul>
    {:else if cite.busy}
      <p class="wk-cite-empty">{t("探しています…")}</p>
    {:else if cite.note === "missing"}
      <p class="wk-cite-empty">{t("蔵書にありません（登録は DOI / PubMed の URL をエディタに貼ってから）")}</p>
    {:else if cite.note}
      <p class="wk-cite-empty wk-cite-err">{cite.note}</p>
    {:else if cite.query.trim()}
      <p class="wk-cite-empty">{t("見つかりません")}</p>
    {/if}
  </div>
</div>

<style>
  .wk-cite-back { position: fixed; inset: 0; z-index: 60; background: rgba(0, 0, 0, 0.18); display: flex; justify-content: center; align-items: flex-start; }
  .wk-cite {
    margin-top: 12vh; width: min(600px, calc(100vw - 32px)); max-height: 70vh; display: flex; flex-direction: column;
    background: var(--panel); color: var(--text); border: 1px solid var(--border); border-radius: 10px; box-shadow: 0 12px 36px rgba(0, 0, 0, 0.25); overflow: hidden;
  }
  .wk-cite-input { margin: 12px 12px 4px; padding: 6px 10px; font-size: 14px; border: 1px solid var(--border); border-radius: 6px; background: var(--bg); color: var(--text); }
  .wk-cite-input:focus { outline: none; border-color: var(--accent); }
  .wk-cite-hint { margin: 2px 14px 8px; font-size: 11.5px; color: var(--dim); overflow-wrap: anywhere; }
  .wk-cite-hint b { color: var(--text); font-weight: 600; }
  .wk-cite-list { list-style: none; margin: 0; padding: 0 0 6px; overflow: auto; border-top: 1px solid var(--border); }
  .wk-cite-row { display: flex; flex-direction: column; gap: 2px; width: 100%; text-align: left; padding: 6px 14px; background: none; border: 0; color: var(--text); cursor: pointer; }
  .wk-cite-row.wk-cite-on { background: var(--accent-soft); }
  .wk-cite-title { font-size: 13px; line-height: 1.4; display: -webkit-box; -webkit-line-clamp: 2; line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; }
  .wk-cite-meta { font-size: 11.5px; color: var(--dim); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .wk-cite-retracted { display: inline-block; margin-right: 6px; padding: 0 5px; border-radius: 4px; font-size: 10.5px; color: #fff; background: var(--danger); vertical-align: 1px; }
  .wk-cite-empty { margin: 0; padding: 10px 14px 14px; font-size: 12px; color: var(--dim); border-top: 1px solid var(--border); }
  .wk-cite-err { color: var(--danger); overflow-wrap: anywhere; }
</style>
