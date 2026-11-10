<script lang="ts">
  /* 本棚のヘッダーの検索欄（書名・フォルダ名 = NFKC + AND・/ でフォーカス）。QuickAdd・＋新規は出さない（本棚に登録は無い）。
     語はタブの Tab.shelf.query が正。打鍵は少し待ってから書く（数千冊の本棚では数千枚のカードを描き直す） */
  import { t } from "../i18n.svelte";
  import { composing } from "../input";
  import { app, frontShelf } from "../state.svelte";

  const s = $derived(frontShelf());
  let q = $state("");
  let timer: ReturnType<typeof setTimeout> | undefined;

  // タブが変わった・フォルダへ移って語が外れた = 欄を写す（打鍵の待ちは捨てる）
  $effect(() => {
    app.tab;
    q = s?.query ?? "";
    clearTimeout(timer);
  });

  function put(v: string) {
    const tab = frontShelf();
    if (!tab || tab.query === v) return;
    tab.query = v;
    tab.scrollTop = 0;
  }

  function onInput() {
    clearTimeout(timer);
    const v = q;
    timer = setTimeout(() => put(v), 150);
  }

  function clear() {
    clearTimeout(timer);
    q = "";
    put("");
  }
</script>

<div class="sb-wrap" role="search">
  <input
    id="sh-search"
    class="search"
    placeholder={t("書名・フォルダ名で検索")}
    title={t("書名・フォルダ名（/ でフォーカス）")}
    spellcheck="false"
    bind:value={q}
    oninput={onInput}
    onkeydown={(e) => {
      if (e.key === "Escape" && !composing(e)) {
        clear();
        (e.currentTarget as HTMLInputElement).blur();
      }
    }} />
  {#if q}<button class="sb-x" title={t("消去")} onclick={clear}>×</button>{/if}
</div>
