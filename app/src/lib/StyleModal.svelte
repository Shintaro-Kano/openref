<script lang="ts">
  import { t } from "./i18n.svelte";
  /* 引用スタイルの設定（Paperpile の Set citation style）。CSL リポジトリ約 1 万誌を検索 → 選択 → 保存。
     選択したスタイルは Rust がリポジトリから取得・キャッシュ（dependent は親で解決）し、
     チェック中の先頭 1 件でライブプレビューする。リポジトリに無い誌は手元の .csl を読み込む。
     「DOI / URL を常に含める」（Paperpile の Always include DOI and URL）もここで切り替え、プレビューに反映する。
     class は sm- 接頭辞（app.css は単一グローバル）。 */
  import { untrack } from "svelte";
  import { open as openDialog } from "@tauri-apps/plugin-dialog";
  import { api, type StyleEntry } from "./api";
  import { LOCALES, forgetStyle, formatBibliography, formatCitation, missingLocales, searchStyles, styleXml } from "./cite";
  import { composing } from "./input";
  import { app, checkedKeys, saveCitePrefs, toast } from "./state.svelte";

  const open = $derived(app.modal === "citeStyle");

  let query = $state("");
  // 約 1 万行。深い proxy にしない（打鍵ごとの検索で全行を読むため）
  let catalog = $state.raw<StyleEntry[] | null>(null);
  let catalogErr = $state("");
  let chosen = $state("");
  let locale = $state("");
  let doiAlways = $state(false);
  let cursor = $state(0);
  let saving = $state(false);
  /** id / lang / doi = どの選択で作ったプレビューか（選択直後に前のスタイルの結果を出さないため） */
  let preview = $state<{
    id: string;
    lang: string;
    doi: boolean;
    cite: string;
    bib: string;
    base: string;
    missing: string[];
  } | null>(null);
  let previewErr = $state("");
  let previewBusy = $state(false);
  let sample = $state<{ csl: any; label: () => string } | null>(null);
  let inputEl = $state<HTMLInputElement | null>(null);
  let listEl = $state<HTMLDivElement | null>(null);

  /** 見本（チェック先頭の item を読めなかったとき。架空の書誌であることを明示して出す） */
  const SAMPLE = {
    type: "article-journal",
    title: "Sample article title for style preview",
    author: [
      { family: "Yamada", given: "Taro" },
      { family: "Suzuki", given: "Hanako" },
      { family: "Tanaka", given: "Ichiro" },
      { family: "Sato", given: "Jiro" },
      { family: "Ito", given: "Saburo" },
      { family: "Kato", given: "Shiro" },
      { family: "Watanabe", given: "Goro" },
    ],
    "container-title": "Radiology",
    "container-title-short": "Radiology",
    volume: "300",
    issue: "2",
    page: "123-130",
    issued: { "date-parts": [[2024, 5]] },
    DOI: "10.0000/sample.0000",
  };

  let wasOpen = false;
  $effect(() => {
    const o = open;
    if (o && !wasOpen) untrack(init);
    wasOpen = o;
  });

  async function init() {
    query = "";
    chosen = app.cite.style;
    locale = app.cite.locale;
    doiAlways = app.cite.doiAlways;
    cursor = cursorOfChosen();
    preview = null;
    previewErr = "";
    queueMicrotask(() => {
      inputEl?.focus();
      listEl?.querySelector(".sm-row.cur")?.scrollIntoView({ block: "nearest" });
    });
    // プレビュー対象: チェック先頭 → 見本（入口は 引用 ▾ = チェック 1 件以上。見本は読めなかったときの代わり）
    sample = { csl: SAMPLE, label: () => t("見本（架空の書誌）") };
    const k = checkedKeys()[0];
    if (k) {
      const d = await api.getItem(k).catch(() => null);
      if (d) sample = { csl: d.csl, label: () => t("チェック中の 1 件目") };
    }
    if (!catalog) {
      catalogErr = "";
      try {
        catalog = await api.citeCatalog(false);
      } catch (e) {
        catalogErr = String(e);
      }
    }
  }

  function close() {
    if (saving) return;
    app.modal = null;
  }

  // ---- 一覧（空欄 = 同梱・導入済み / 入力 = 索引検索） ----
  const installed = $derived(app.cite.installed);
  const byId = $derived.by(() => {
    const m = new Map<string, StyleEntry>();
    for (const s of catalog ?? []) m.set(s.id, s);
    for (const s of installed) m.set(s.id, { ...(m.get(s.id) ?? {}), ...s });
    return m;
  });
  const sections = $derived.by(() => {
    const q = query.trim();
    if (q) {
      // 自作（索引に無い）も検索対象に含める
      const pool = [...installed.filter((s) => s.custom), ...(catalog ?? [])];
      const hits = searchStyles(pool, q, 200);
      return [{ title: `${t("検索結果")} ${hits.length === 200 ? "200+ " : hits.length} ${t("件")}`, items: hits }];
    }
    return installed.length ? [{ title: t("同梱・導入済み"), items: installed }] : [];
  });
  const rows = $derived(sections.flatMap((s) => s.items));
  const installedIds = $derived(new Set(installed.map((s) => s.id)));
  const canSave = $derived(!!chosen && !saving && !previewErr && !(query.trim() && rows.length === 0));
  const chosenEntry = $derived<StyleEntry>(byId.get(chosen) ?? { id: chosen, title: chosen });
  /** いまの選択・言語・DOI 設定で作ったプレビューだけを出す */
  const shown = $derived(
    preview && preview.id === chosen && preview.lang === locale && preview.doi === doiAlways ? preview : null
  );
  /** プレビュー対象に DOI も URL も無い（トグルを入れても変わらない理由を出す） */
  const sampleNoIds = $derived(!!sample && !String(sample.csl?.DOI ?? "").trim() && !String(sample.csl?.URL ?? "").trim());

  function pick(s: StyleEntry, i: number) {
    chosen = s.id;
    cursor = i;
  }

  /** 一覧の中の選択中の行（開いた直後 = 既定スタイル）。↑↓ はここから動く。一覧に無ければ -1（↓ で先頭行から入る） */
  function cursorOfChosen() {
    return rows.findIndex((s) => s.id === chosen);
  }

  function onQueryKey(e: KeyboardEvent) {
    if (composing(e)) return;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (!rows.length) return;
      cursor = Math.max(0, Math.min(rows.length - 1, cursor + (e.key === "ArrowDown" ? 1 : -1)));
      chosen = rows[cursor].id;
      queueMicrotask(() => listEl?.querySelector(".sm-row.cur")?.scrollIntoView({ block: "nearest" }));
    } else if (e.key === "Enter") {
      e.preventDefault();
      // 該当なし・取得失敗・候補未選択（索引の読込前に打った等）では保存しない（保存ボタンの disabled と同じ条件）
      if (canSave) void save();
    }
  }

  // 入力のたびに先頭候補を選ぶ（入力 → Enter で保存できるように）。空欄に戻したら選択は保ち、カーソルをその行に置く。
  // 索引が後から届いた場合（初回は約 3 MB の取得）もそこで先頭を選ぶ
  $effect(() => {
    const q = query.trim();
    catalog;
    untrack(() => {
      if (!q) {
        cursor = cursorOfChosen();
        return;
      }
      cursor = 0;
      if (rows.length) chosen = rows[0].id;
    });
  });

  // ---- ライブプレビュー（選択・言語・DOI 設定の変更ごと。古い応答は捨てる） ----
  let seq = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    const id = chosen;
    const lang = locale;
    const doi = doiAlways;
    const s = sample;
    if (!open || !id || !s) return;
    previewErr = "";
    clearTimeout(timer);
    // 打鍵中は先頭候補が次々変わる（= 未取得スタイルの取得が走る）ので少し待つ。DOI トグルだけの変更は待たない
    const onlyDoi = untrack(() => !!preview && preview.id === id && preview.lang === lang);
    timer = setTimeout(() => void runPreview(id, lang, doi, s.csl), onlyDoi ? 0 : 300);
  });

  async function runPreview(id: string, lang: string, doi: boolean, csl: any) {
    const my = ++seq;
    previewBusy = true;
    previewErr = "";
    try {
      const xml = await styleXml(id);
      const [cite, bib] = await Promise.all([
        formatCitation(csl, id, lang),
        formatBibliography(csl, id, lang, doi),
      ]);
      if (my !== seq) return;
      preview = { id, lang, doi, cite, bib, base: cslTitle(xml), missing: missingLocales() };
    } catch (e) {
      if (my !== seq) return;
      preview = null;
      previewErr = String(e).slice(0, 300);
    } finally {
      if (my === seq) previewBusy = false;
    }
  }

  /** 解決済み CSL の <info><title>（dependent の場合は親スタイル名になる） */
  function cslTitle(xml: string): string {
    const m = /<info>[\s\S]*?<title>([\s\S]*?)<\/title>/.exec(xml);
    return m ? m[1].replace(/&amp;/g, "&").trim() : "";
  }

  async function save() {
    if (!canSave) return;
    saving = true;
    try {
      await saveCitePrefs(chosen, locale, doiAlways);
      app.modal = null;
    } catch (e) {
      toast(t("✗ スタイルを保存できません: ") + String(e).slice(0, 200));
    } finally {
      saving = false;
    }
  }

  async function loadFile() {
    const path = await openDialog({ multiple: false, filters: [{ name: t("CSL スタイル"), extensions: ["csl", "xml"] }] });
    if (!path || Array.isArray(path)) return;
    try {
      const e = await api.citeInstallFile(path);
      forgetStyle(e.id); // 同じファイルを直して読み込み直したときに古い XML を使わない
      app.cite.installed = await api.citeInstalled();
      query = "";
      chosen = e.id;
      // 検索欄が元から空だと query の $effect が走らないので、カーソルもここで読み込んだ行に合わせる
      cursor = cursorOfChosen();
      queueMicrotask(() => listEl?.querySelector(".sm-row.cur")?.scrollIntoView({ block: "nearest" }));
      toast(t("✓ 読み込みました: ") + e.title);
    } catch (e) {
      toast(t("✗ CSL を読み込めません: ") + String(e).slice(0, 200));
    }
  }

  function fmtLabel(f?: string | null) {
    return f === "numeric" ? t("番号") : f === "author-date" ? t("著者-年") : f === "note" ? t("脚注") : f ?? "";
  }
</script>

<svelte:window
  onkeydown={(e) => {
    if (open && e.key === "Escape" && !composing(e)) {
      e.stopPropagation();
      close();
    }
  }} />

{#if open}
  <div class="overlay" onclick={close} role="presentation">
    <div class="modal sm-modal" onclick={(e) => e.stopPropagation()} role="dialog" aria-label={t("引用スタイルを設定")}>
      <div class="modal-head">
        <h3>{t("引用スタイルを設定")}</h3>
        <button class="x" onclick={close}>×</button>
      </div>
      <div class="modal-body sm-body">
        <input
          class="sm-q"
          bind:this={inputEl}
          bind:value={query}
          onkeydown={onQueryKey}
          placeholder={t("誌名・スタイル名で検索（例: European Radiology / AJNR / Vancouver）")} />

        <div class="sm-list" bind:this={listEl}>
          {#if query.trim() && !catalog}
            <div class="sm-empty">{catalogErr ? "✗ " + catalogErr : t("スタイル索引を読み込み中…")}</div>
          {:else}
            {#each sections as sec (sec.title)}
              <div class="sm-sec">{sec.title}</div>
              {#each sec.items as s (sec.title + s.id)}
                {@const i = rows.indexOf(s)}
                <button class="sm-row" class:cur={chosen === s.id} onclick={() => pick(s, i)} ondblclick={save}>
                  <span class="sm-title">{s.title}</span>
                  {#if s.short && s.short !== s.title}<span class="sm-short">{s.short}</span>{/if}
                  <span class="sm-tags">
                    {#if s.format}<span class="sm-tag">{fmtLabel(s.format)}</span>{/if}
                    {#if s.builtin}<span class="sm-tag">{t("同梱")}</span>
                    {:else if s.custom}<span class="sm-tag">{t("自作")}</span>
                    {:else if installedIds.has(s.id)}<span class="sm-tag">{t("導入済")}</span>{/if}
                    {#if app.cite.style === s.id}<span class="sm-cur">{t("✓ 既定")}</span>{/if}
                  </span>
                </button>
              {/each}
            {/each}
            {#if query.trim() && rows.length === 0}
              <div class="sm-empty">
                {t("該当なし。リポジトリに無い誌は、出版社の汎用スタイル（例:")} <i>Springer Vancouver</i> / <i>Elsevier Vancouver</i> /
                <i>American Medical Association</i>{t("）を検索するか、投稿規定に合う .csl を読み込んでください。")}
              </div>
            {/if}
          {/if}
        </div>

        <div class="sm-opts">
          <label>
            {t("言語")}
            <select bind:value={locale}>
              {#each LOCALES as [code, label] (code)}<option value={code}>{code ? `${label} (${code})` : t("既定（スタイルに従う）")}</option>{/each}
            </select>
          </label>
          <button class="sm-file" onclick={loadFile} title={t("リポジトリに無い誌・自作スタイル（.csl）を読み込む")}>{t(".csl を読み込む…")}</button>
        </div>
        <label
          class="sm-doi"
          title={t("スタイルが DOI を省く場合も、文献リストの各文献の末尾に DOI（DOI が無ければ URL）を付けます。書き方はスタイルの DOI 表記（無ければ https://doi.org/…）。DOI も URL も無い文献はそのまま")}>
          <input type="checkbox" class="sm-switch" bind:checked={doiAlways} />
          {t("DOI / URL を常に含める")}
          <span class="sm-dim">{t("（スタイルが省く場合も末尾に付ける・DOI が無ければ URL）")}</span>
        </label>

        <div class="sm-preview">
          <div class="sm-pv-head">
            <b>{chosenEntry.title}</b>
            {#if shown?.base && (chosenEntry.dep || chosenEntry.parent)}
              <span class="sm-base" title={t("この誌名スタイルは書式の本体（親スタイル）を参照しています")}>{t("書式:")} {shown.base}</span>
            {/if}
            <span class="sm-pv-src">{sample?.label() ?? ""}</span>
          </div>
          {#if previewErr}
            <div class="sm-err">✗ {previewErr}</div>
          {:else if shown}
            <div class="sm-pv-row"><span class="sm-pv-k">{t("本文中")}</span><span class="sm-pv-v">{shown.cite}</span></div>
            <div class="sm-pv-row"><span class="sm-pv-k">{t("文献リスト")}</span><span class="sm-pv-v">{shown.bib}</span></div>
            {#if shown.missing.length}
              <div class="sm-dim">{t("※ 言語")} {shown.missing.join(", ")} {t("の語彙を取得できず、英語（en-US）で代用しています")}</div>
            {/if}
            {#if shown.doi && sampleNoIds}
              <div class="sm-dim">{t("※ この文献には DOI も URL も無いため、付け足すものがありません")}</div>
            {/if}
          {:else}
            <div class="sm-dim">{previewBusy || chosen ? t("スタイルを取得中…") : ""}</div>
          {/if}
        </div>
      </div>
      <div class="modal-foot">
        <span class="sm-foot-note">{t("保存したスタイルは引用コピーと Word アドインの一覧で使えます")}</span>
        <button onclick={close}>{t("キャンセル")}</button>
        <button class="primary" disabled={!canSave} onclick={save}>
          {saving ? t("保存中…") : t("保存")}
        </button>
      </div>
    </div>
  </div>
{/if}
