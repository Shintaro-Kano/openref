<script lang="ts">
  /* チャットの応答 1 ブロックの md（確定した本文だけ）。描画は既存の MdView（markdown-it + DOMPurify + shadow DOM）を通す =
     独自の innerHTML を持たない。`[[名前]]` は vault のページ一覧で引き当て（chat.ts の chatLinks）、リンクの遷移は Wiki の navigate */
  import { untrack } from "svelte";
  import MdView from "./MdView.svelte";
  import { chatLinks } from "./chat";
  import type { WikiPageData } from "./types";
  import { navigate, readPage, wk } from "./wiki.svelte";

  let { text }: { text: string } = $props();

  // `[[…]]` の解決は木（wk.pages）が変わるたびに引き直すが、結果が同じなら page を作り直さない = 描き直さない
  const links = $derived(text.includes("[[") ? chatLinks(text, wk.pages) : []);
  const linkKey = $derived(JSON.stringify(links));
  // 相対リンクは vault の root から解く（path = 空 = 直下のページとして扱う）
  const page = $derived.by<WikiPageData>(() => {
    void linkKey;
    return {
      path: "",
      abs_path: "",
      kind: "md",
      title: "",
      size: text.length,
      mtime: 0,
      content: text,
      links: untrack(() => links),
    };
  });
</script>

<div class="wk-ch-md">
  <MdView {page} root={wk.status?.root ?? ""} refs={wk.refDirs} onNavigate={navigate} {readPage} />
</div>

<style>
  .wk-ch-md { font-size: 13px; min-width: 0; }
</style>
