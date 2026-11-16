<script lang="ts">
  /* OpenNote の「フォルダの設定…」の窓の中身（名前のメニューから開く）: 参照フォルダ（一覧・追加…・外す）と
     除外フォルダ（一覧・索引に入れる）。枠・覆い・Esc・焦点の出入りは WikiDialogs の Modal と holdFocus（ほかの窓と同じ型）。
     動詞は今ある物だけ: 参照フォルダ = refdirs.svelte.ts（native のフォルダ選択・画面の root の物）・除外フォルダ = setExcluded
     （木の右クリックの「索引に入れる」と同じ道 = wiki_exclude_set → 背景の走査 → wiki-changed で木と一覧が読み直る）。
     除外フォルダの一覧は木と同じ wk.excluded（wiki_list の excluded = いちばん上の除外フォルダ・vault 相対）= command は足さない。
     パスの出し方は今のまま: 参照フォルダ = 絶対パス（右ペインのホームと同じく先頭側を省略・title に全体）・除外フォルダ = vault 相対 + / */
  import { untrack } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import { t } from "../i18n.svelte";
  import { setExcluded, wk } from "./wiki.svelte";
  import { addRefDir, removeRefDir } from "./refdirs.svelte";

  let { onclose }: { onclose: () => void } = $props();

  /** 押した行（"ref:" / "ex:" + パス）= 二度押しで 2 回呼ばない。参照フォルダの [外す] は応答で一覧が替わる = 応答まで。
      除外フォルダの「索引に入れる」は一覧が追うのが背景の走査の後（約 1.6 秒）= 一覧から消えるまで押せない（失敗したら戻す） */
  const pending = new SvelteSet<string>();
  async function once(key: string, f: () => Promise<boolean | void>, keepOnOk = false) {
    if (pending.has(key)) return;
    pending.add(key);
    let ok = false;
    try {
      ok = (await f()) !== false;
    } finally {
      if (!(keepOnOk && ok)) pending.delete(key);
    }
  }
  // 除外フォルダの一覧が追ったら（その行が消えたら）押した印を消す
  $effect(() => {
    const ex = new Set(wk.excluded);
    untrack(() => {
      for (const k of [...pending]) if (k.startsWith("ex:") && !ex.has(k.slice(3))) pending.delete(k);
    });
  });

  /** 開いたら [閉じる] へ焦点（WKWebView はクリックで焦点を移さない = 窓の中に焦点を入れる・何も変えない既定のボタン） */
  function focusIt(node: HTMLElement) {
    queueMicrotask(() => node.focus());
  }
</script>

<div class="modal-head"><h3 id="wk-dlg-h">{t("フォルダの設定")}</h3></div>
<div class="modal-body wk-fs">
  <section class="wk-fs-sec" aria-labelledby="wk-fs-ref-h">
    <h4 id="wk-fs-ref-h">{t("参照フォルダ")}<span class="wk-fs-n">{wk.refDirs.length}</span></h4>
    <p class="wk-fs-note">{t("ワークスペースの外のフォルダ。その中の PDF・html・画像へのリンクを内蔵の表示で開きます（読み取り専用・索引には入りません）")}</p>
    {#each wk.refDirs as d (d)}
      <div class="wk-fs-row">
        <span class="wk-fs-path wk-fs-abs" title={d}>&lrm;{d}&lrm;</span>
        <button title={d} disabled={pending.has("ref:" + d)} onclick={() => void once("ref:" + d, () => removeRefDir(d))}>{t("外す")}</button>
      </div>
    {:else}
      <p class="wk-fs-empty">{t("参照フォルダはありません")}</p>
    {/each}
    <div class="wk-fs-acts"><button onclick={() => void addRefDir()}>{t("追加…")}</button></div>
  </section>
  <section class="wk-fs-sec" aria-labelledby="wk-fs-ex-h">
    <h4 id="wk-fs-ex-h">{t("除外フォルダ")}<span class="wk-fs-n">{wk.excluded.length}</span></h4>
    <p class="wk-fs-note">{t("索引に入れないフォルダ（木の右クリックの「索引から除く」で足す）。木には薄い行で残ります")}</p>
    {#each wk.excluded as d (d)}
      <div class="wk-fs-row">
        <span class="wk-fs-path" title={d}>{d}/</span>
        <button title={d} disabled={pending.has("ex:" + d)} onclick={() => void once("ex:" + d, () => setExcluded(d, false), true)}>{t("索引に入れる")}</button>
      </div>
    {:else}
      <p class="wk-fs-empty">{t("除外フォルダはありません")}</p>
    {/each}
  </section>
</div>
<div class="modal-foot">
  <button use:focusIt onclick={onclose}>{t("閉じる")}</button>
</div>

<style>
  .wk-fs { display: flex; flex-direction: column; gap: 16px; }
  .wk-fs-sec h4 { margin: 0 0 4px; font-size: 12.5px; font-weight: 600; display: flex; gap: 6px; align-items: baseline; }
  .wk-fs-n { color: var(--dim); font-size: 11px; font-weight: 400; }
  .wk-fs-note { margin: 0 0 6px; font-size: 11.5px; color: var(--dim); line-height: 1.5; }
  .wk-fs-empty { margin: 2px 0 6px; font-size: 12px; color: var(--dim); }
  .wk-fs-row { display: flex; align-items: center; gap: 8px; padding: 3px 0; font-size: 12px; }
  .wk-fs-path { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  /* 絶対パスは先頭側を省略して末尾（フォルダ名）を残す（右ペインのホームの参照フォルダと同じ rtl の型・両端の &lrm;） */
  .wk-fs-abs { direction: rtl; text-align: left; }
  /* 行のボタン = 前の右ペインのホームの [外す] / [追加…]（WikiInfo の .wk-btn）と同じ見た目 */
  .wk-fs-row button, .wk-fs-acts button {
    flex: none; border: 1px solid var(--border); border-radius: 6px; padding: 2px 8px; background: var(--bg); font-size: 11.5px; color: var(--text);
  }
  .wk-fs-row button:hover:not(:disabled), .wk-fs-acts button:hover { border-color: var(--accent); }
  .wk-fs-row button:disabled { opacity: 0.5; cursor: default; }
  .wk-fs-acts { margin-top: 4px; }
</style>
