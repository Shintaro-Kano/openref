<script lang="ts">
  /* OpenNote: 画像・動画・音声・vault 内 PDF・その他のファイルの表示。
     - 画像: 幅合わせ ⇄ 等倍
     - 動画 mp4 / m4v / mov・音声 mp3 / m4a / wav: render/media.ts の部品（md の `![[…]]` と同じ）。再生に失敗したら外部で開く案内
     - PDF: 先頭 5 バイトが `%PDF-` と確かめてから WebKit 内蔵の PDF 表示（<iframe src="asset://…">）。asset protocol は中身で
       Content-Type を決める = 中身が HTML の x.pdf を iframe に載せると asset://localhost の origin で script が走るため（違えば外部で開くだけ）。
       読めなかった（403・読み出しの失敗）ときは「PDF として表示できません」でなく「読めませんでした（理由）」の案内。
       sandbox は付けない（sandbox="" だと内蔵 PDF 表示が描かれない = 2026-10-31 に WKWebView で確認）。[外部で開く] は常に出す（既存 PdfViewer は使わない）
     - その他（webm / mkv 等も）: 外部のアプリで開くボタン
     「外部で開く」は onNavigate({kind:"open", abs})（骨組みが wiki_open_path を呼ぶ）。props は契約 WikiViewProps */
  import type { WikiViewProps } from "./types";
  import { t } from "../i18n.svelte";
  import { assetUrl, viewKindOf } from "./render/asset";
  import { buildMedia, checkPdf, type PdfCheck } from "./render/media";

  let { page, onNavigate }: WikiViewProps = $props();

  const abs = $derived(page.abs_path);
  const src = $derived(assetUrl(page.abs_path));
  const name = $derived(page.path.slice(page.path.lastIndexOf("/") + 1));
  const kind = $derived.by(() => {
    const k = page.media ?? viewKindOf(page.path);
    return k === "image" || k === "video" || k === "audio" || k === "pdf" ? k : "other";
  });
  let fit = $state(true);

  // PDF: 中身が `%PDF-` で始まると確かめてから iframe に載せる（中身が HTML の x.pdf を asset://localhost の origin で走らせない）。
  // null = 確認中・pdf = iframe・other = PDF ではない / unreadable = 読めなかった（どちらも外部で開くだけ）。pdfOkFor = その結果を確かめた src。
  // 描画は pdfOkFor === src のときだけ（部品が使い回されて page だけ差し替わると、effect が走る前の flush で前の src の結果のまま
  // 新しい src の iframe を作りうる）
  let pdfOk = $state<PdfCheck | null>(null);
  let pdfOkFor = $state<string | null>(null);
  const pdfState = $derived(pdfOkFor === src ? pdfOk : null);
  $effect(() => {
    if (kind !== "pdf") return;
    const u = src;
    pdfOk = null;
    pdfOkFor = null;
    let live = true;
    void checkPdf(u).then((ok) => {
      if (!live) return;
      pdfOk = ok;
      pdfOkFor = u;
    });
    return () => {
      live = false;
    };
  });

  function openExternal() {
    onNavigate({ kind: "open", abs });
  }

  /** 動画・音声の部品を差し込む（page が変われば作り直す） */
  function player(node: HTMLElement, p: { kind: "video" | "audio"; src: string; abs: string; name: string }) {
    const mount = (q: typeof p) => {
      node.replaceChildren(
        buildMedia(document, {
          kind: q.kind,
          src: q.src,
          abs: q.abs,
          name: q.name,
          labels: { openExternal: t("外部で開く"), playFailed: t("ここでは再生できません。外部のアプリで開いてください") },
        }),
      );
    };
    const click = (e: Event) => {
      const b = (e.target as Element | null)?.closest?.("[data-wk-open]");
      if (!b) return;
      e.preventDefault();
      onNavigate({ kind: "open", abs: b.getAttribute("data-wk-open") ?? abs });
    };
    node.addEventListener("click", click);
    mount(p);
    return {
      update: mount,
      destroy: () => {
        node.removeEventListener("click", click);
        // 再生中の要素を止めてから外す（asset の Range 読込を残さない）
        node.querySelectorAll("video, audio").forEach((m) => {
          (m as HTMLMediaElement).pause();
          m.removeAttribute("src");
          (m as HTMLMediaElement).load();
        });
      },
    };
  }
</script>

<div class="wk-mv">
  <!-- ファイル名と [外部で開く] は骨組みのツールバー（WikiPage の wk-bar）が出す = ここは画像の等倍 / 幅合わせだけ -->
  {#if kind === "image"}
    <div class="wk-mv-bar">
      <button class="wk-mv-btn" onclick={() => (fit = !fit)}>{fit ? t("等倍") : t("幅に合わせる")}</button>
    </div>
  {/if}
  {#if kind === "image"}
    <div class="wk-mv-img" class:wk-mv-fit={fit}>
      <img {src} alt={name} />
    </div>
  {:else if kind === "video" || kind === "audio"}
    {#key page.path}
      <div class="wk-mv-player wk-mv-{kind}" use:player={{ kind, src, abs, name }}></div>
    {/key}
  {:else if kind === "pdf" && pdfState?.kind === "pdf"}
    <p class="wk-mv-hint">{t("PDF が表示されないときは外部で開いてください")}</p>
    {#key page.path}
      <iframe class="wk-mv-pdf" {src} title={name}></iframe>
    {/key}
  {:else if kind === "pdf" && pdfState?.kind === "unreadable"}
    <div class="wk-mv-other">
      <p>{t("このファイルを読めませんでした。外部のアプリで開いてください")} ({pdfState.reason})</p>
      <button class="wk-mv-btn" onclick={openExternal}>{t("外部で開く")}</button>
    </div>
  {:else if kind === "pdf" && pdfState?.kind === "other"}
    <div class="wk-mv-other">
      <p>{t("このファイルは PDF として表示できません。外部のアプリで開いてください")}</p>
      <button class="wk-mv-btn" onclick={openExternal}>{t("外部で開く")}</button>
    </div>
  {:else if kind === "pdf"}
    <p class="wk-mv-hint">…</p>
  {:else}
    <div class="wk-mv-other">
      <p>{t("このファイルは外部のアプリで開きます")}</p>
      <button class="wk-mv-btn" onclick={openExternal}>{t("外部で開く")}</button>
    </div>
  {/if}
</div>

<style>
  .wk-mv { display: flex; flex-direction: column; gap: 8px; min-height: 100%; padding: 10px 14px; box-sizing: border-box; }
  .wk-mv-bar { display: flex; align-items: center; gap: 8px; font-size: 12px; color: var(--dim); }
  .wk-mv-btn {
    font: inherit; font-size: 12px; padding: 2px 10px; border: 1px solid var(--border); border-radius: 5px;
    background: var(--panel); color: var(--text); cursor: pointer;
  }
  .wk-mv-btn:hover { border-color: var(--accent); }
  .wk-mv-img { overflow: auto; }
  .wk-mv-img img { display: block; max-width: none; }
  .wk-mv-img.wk-mv-fit img { max-width: 100%; height: auto; }
  .wk-mv-player :global(video) { display: block; max-width: 100%; max-height: 75vh; background: #000; }
  .wk-mv-player :global(audio) { display: block; width: 100%; max-width: 640px; }
  .wk-mv-player :global(.wk-media-bar) { display: none; }
  .wk-mv-player :global(.wk-media-fail) { padding: 16px; border: 1px dashed var(--border); border-radius: 6px; color: var(--dim); }
  .wk-mv-player :global(.wk-media-open) {
    font: inherit; font-size: 12px; padding: 2px 10px; border: 1px solid var(--border); border-radius: 5px;
    background: var(--panel); color: var(--text); cursor: pointer;
  }
  .wk-mv-hint { font-size: 12px; color: var(--dim); }
  .wk-mv-pdf { flex: 1; width: 100%; min-height: 70vh; border: 1px solid var(--border); border-radius: 6px; background: #fff; }
  .wk-mv-other { padding: 24px; text-align: center; color: var(--dim); display: flex; flex-direction: column; gap: 10px; align-items: center; }
</style>
