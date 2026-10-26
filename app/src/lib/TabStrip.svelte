<script lang="ts">
  import { t } from "./i18n.svelte";
  /* タブ列（ブラウザ型・ヘッダーの上に 1 段）。[アイコン 題名 ×]…[＋]。
     タブ本体は div role="tab"（× を中に置くと <button> の入れ子 = Svelte 5 のコンパイルエラー・tab の子は支援技術から見えない）、
     × は兄弟の button を重ねる。tablist が持つのはタブだけ（× は aria-hidden のマウス用・キーは ⌘W、＋ は tablist の外）。
     クリック = 切替 / ← → Home End = 隣・端へ / 中クリック = 閉じる / 空きのダブルクリック・＋ = 新しいタブ（すべて）。
     最後の 1 枚は × を出さない（PDF タブなら出す = ビューアだけ閉じる）。題名は id から引く（viewTitle）。検索語・チップは題名に
     足さず tooltip に出す。PDF タブは文書のアイコン + 論文の題名（tooltip は題名の全文）。
     並べ替え = タブを掴んで左右へドラッグ（pointer イベントで自作 = onTabDown。HTML5 の D&D は Tauri の dragDropEnabled と競合する）。
     class は tabs- 接頭辞（tb- は viewer のツールバーが使用中） */
  import { tick } from "svelte";
  import LabelBadge from "./LabelBadge.svelte";
  import { app, activateTab, closeTab, moveTab, openTab, tabView, viewTitle, type Tab, type View } from "./state.svelte";

  let listEl = $state<HTMLDivElement | null>(null);
  /** 並べ替えで掴んでいるタブの id と、見た目のずれ（配列上の枠の左端から px。transform で指に追わせる） */
  let dragId = $state<number | null>(null);
  let dragX = $state(0);

  function tip(tab: Tab, v: View): string {
    const front = tab.id === app.tab;
    const search = front ? app.search : tab.search;
    const f = front ? app.filters : tab.filters;
    const parts = [viewTitle(v)];
    if (f.author) parts.push(`${t("著者")} ${f.author.label}`);
    if (f.journal) parts.push(`${t("誌名")} ${f.journal}`);
    if (search) parts.push(`${t("「")}${search}${t("」")}`);
    return parts.join(" · ");
  }

  function onAux(e: MouseEvent, id: number) {
    if (e.button !== 1) return;
    e.preventDefault();
    void closeTab(id);
  }

  /** タブの上のキー: Enter / Space = 前面に・← → = 隣（端からは反対の端）・Home / End = 端。移った先のタブにフォーカス
      （切替を待つ間に user がフォーカスを列の外 = 検索欄などへ移していたら奪わない）。修飾キー付きは扱わない */
  async function onTabKey(e: KeyboardEvent, i: number) {
    if (e.metaKey || e.ctrlKey || e.altKey || e.shiftKey) return;
    const n = app.tabs.length;
    let j: number;
    if (e.key === "Enter" || e.key === " ") j = i;
    else if (e.key === "ArrowRight") j = (i + 1) % n;
    else if (e.key === "ArrowLeft") j = (i - 1 + n) % n;
    else if (e.key === "Home") j = 0;
    else if (e.key === "End") j = n - 1;
    else return;
    e.preventDefault();
    await activateTab(app.tabs[j].id);
    await tick();
    // 前面になった PDF ビューアはページにフォーカスを取る（PdfViewer）が、タブの上で ← → を続けられるよう列へ戻す
    const el = document.activeElement;
    if (listEl?.contains(el) || el?.closest(".viewer-overlay")) listEl?.querySelector<HTMLElement>(".tabs-item.front .tabs-tab")?.focus();
  }

  /** タブの pointerdown（左ボタン）: 6px 動いたら並べ替えのドラッグ（一覧の行ドラッグと同じ pointer 自作。1 枚なら何もしない）。
      掴んだタブは transform で指に追わせ（列の中に留める）、その中心に最も近い枠へ app.tabs の順を動かす（moveTab = 配列の順そのもの
      が変わる → ⌘1〜9・巡回・⌘W の行き先・ui_tabs の保存が従う）。全タブは同じ幅（flex: 1 1 0）なので枠 i の位置は中身に依らず、
      DOM の i 番目の rect で測れる（DOM は前回の move までの配列順 = 枠の位置は変わらない）。
      動かずに離した = 従来のクリック（onclick が切替）。ドラッグになったら、離した直後の click は 1 回だけ奪う（指の下が別のタブや
      × に変わっていても切替・閉じるに飛ばない）。離したら掴んだタブを前面に（ブラウザと同じ。処理中なら切替だけ見送られる）。
      Esc = 掴んだときの順に戻して終える（押したままの指を離したときの click も奪う）。pointerup が来ない経路 = その位置で終える
      （前面にしない）: pointercancel・window の blur（⌘Tab で他のアプリへ）・contextmenu（⌃クリック・掴んだまま右クリック = OS の
      メニューが mouseup を飲む）・左ボタンが離れている move（ev.buttons）。
      ドラッグの最中にタブが閉じられた・開かれた（⌘W ⌘T）ら枠を測り直して続け、掴んだタブが消えた・1 枚になったら終える。
      離した後にフォーカスが body へ落ちていたら（動いた DOM = 掴んだタブ自身が外れて戻ると外れる）掴んだタブに戻す（← → が続く） */
  function onTabDown(e: PointerEvent, id: number) {
    if (e.button !== 0 || e.ctrlKey || !e.isPrimary || app.tabs.length < 2 || !listEl) return;
    const list = listEl;
    const item = (e.target as HTMLElement | null)?.closest<HTMLElement>(".tabs-item");
    if (!item) return;
    const sx = e.clientX;
    const sy = e.clientY;
    const grab = e.clientX - item.getBoundingClientRect().left; // 掴んだ点のタブ内の位置
    let started = false;
    let order: number[] = []; // 掴んだときの順（Esc で戻す）
    const move = (ev: PointerEvent) => {
      if (!(ev.buttons & 1)) return cancel(); // 左ボタンが離れている = pointerup を取りこぼした
      if (!started) {
        if (Math.hypot(ev.clientX - sx, ev.clientY - sy) < 6) return;
        started = true;
        order = app.tabs.map((t) => t.id);
        document.body.classList.add("dragging");
        dragId = id;
        window.addEventListener("keydown", key, true); // Esc はドラッグが始まってから奪う（押しているだけなら App の Esc）
      }
      const from = app.tabs.findIndex((t) => t.id === id);
      if (from < 0 || app.tabs.length < 2) return stop(); // 掴んだタブが閉じられた（⌘W）・1 枚だけ残った
      // 枠 i の左端（レイアウト上の位置）。掴んだタブ（DOM では from 番目）の rect は transform の分ずれているので戻す
      // （dragX は前回の move で書いた値 = この時点で DOM に反映済み）
      const items = [...list.querySelectorAll<HTMLElement>(".tabs-item")];
      if (items.length !== app.tabs.length || items.length < 2) return; // 描画が追いつくまで待つ
      const slots = items.map((el, i) => el.getBoundingClientRect().left - (i === from ? dragX : 0));
      const w = items[from === 0 ? 1 : 0].getBoundingClientRect().width;
      const lr = list.getBoundingClientRect();
      const left = Math.max(lr.left, Math.min(lr.right - w, ev.clientX - grab)); // 見た目の左端（列の中に留める）
      const center = left + w / 2;
      let to = 0;
      for (let i = 1; i < slots.length; i++) {
        if (Math.abs(slots[i] + w / 2 - center) < Math.abs(slots[to] + w / 2 - center)) to = i;
      }
      if (to !== from) moveTab(id, to);
      dragX = left - slots[to];
    };
    /** 見た目と move を解く（pointerup のリスナーは残す = 離したときの click を奪う） */
    const stop = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("keydown", key, true);
      document.body.classList.remove("dragging");
      if (dragId === id) dragId = null;
      dragX = 0;
    };
    /** 終わり。activate = pointerup で離した（pointercancel / blur は前面にしない） */
    const finish = (activate: boolean) => {
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", cancel);
      window.removeEventListener("blur", cancel);
      window.removeEventListener("contextmenu", cancel);
      window.removeEventListener("pointermove", move);
      window.removeEventListener("keydown", key, true);
      if (!started) return; // クリック → onclick が切替
      const dropped = dragId === id && activate; // Esc で取り消していない・掴んだタブが消えていない
      stop();
      swallowClick();
      if (dropped && app.tab !== id) void activateTab(id);
      void tick().then(() => {
        if (document.activeElement !== document.body) return; // 入力欄などにあるフォーカスは奪わない
        const i = app.tabs.findIndex((t) => t.id === id);
        if (i >= 0) list.querySelectorAll<HTMLElement>(".tabs-tab")[i]?.focus();
      });
    };
    const up = () => finish(true);
    const cancel = () => finish(false);
    const key = (ev: KeyboardEvent) => {
      if (ev.key !== "Escape") return;
      ev.preventDefault();
      ev.stopImmediatePropagation(); // App の Esc（メニュー・ビューアを閉じる）に渡さない
      stop();
      // 掴んだときの順に戻す（その間に開いたタブは後ろに残る・閉じたタブは飛ばす）
      order.filter((x) => app.tabs.some((t) => t.id === x)).forEach((x, i) => moveTab(x, i));
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", cancel);
    window.addEventListener("blur", cancel);
    window.addEventListener("contextmenu", cancel);
  }

  /** ドラッグの直後の click と dblclick を 1 回ずつ奪う（タブの切替・× の閉じる・列の空きのダブルクリック = 新しいタブに飛ばない）。
      どちらも pointerup と同じタスクで続けて来る（Svelte の onclick は root への委譲 = window の capture で止めれば届かない）。
      来なければ setTimeout 0 で解く */
  function swallowClick() {
    const offs: (() => void)[] = [];
    for (const type of ["click", "dblclick"] as const) {
      const f = (e: MouseEvent) => {
        e.stopPropagation();
        e.preventDefault();
        off();
      };
      const off = () => window.removeEventListener(type, f, true);
      window.addEventListener(type, f, true);
      offs.push(off);
    }
    setTimeout(() => offs.forEach((off) => off()), 0);
  }

  /** タブ列の空き（タブ・＋ の外）のダブルクリック = 新しいタブ。マウスだけの近道なので、列はフォーカスを取らない
      （tabindex を付けると空きのクリックで検索欄からフォーカスが外れる）= リスナーは action で付ける */
  function dblclickNew(node: HTMLElement) {
    const f = (e: MouseEvent) => {
      if (e.target === node || e.target === listEl) void openTab();
    };
    node.addEventListener("dblclick", f);
    return { destroy: () => node.removeEventListener("dblclick", f) };
  }

  // 前面のタブが列の外（タブが溢れて横に送られている）なら見える位置へ送る（⌘9 で末尾へ移ったとき等）
  $effect(() => {
    app.tab;
    app.tabs.length;
    void tick().then(() =>
      listEl?.querySelector<HTMLElement>(".tabs-item.front")?.scrollIntoView({ block: "nearest", inline: "nearest" })
    );
  });
</script>

<div class="tabs-strip" use:dblclickNew>
  <div class="tabs-list" role="tablist" aria-label={t("タブ")} style="--n:{app.tabs.length}" bind:this={listEl}>
    {#each app.tabs as tab, i (tab.id)}
      {@const v = tabView(tab)}
      {@const front = tab.id === app.tab}
      <div
        class="tabs-item"
        class:front
        class:tabs-drag={dragId === tab.id}
        style:transform={dragId === tab.id ? `translateX(${dragX}px)` : null}
        role="presentation">
        <div
          class="tabs-tab"
          role="tab"
          tabindex={front ? 0 : -1}
          aria-selected={front}
          title={tab.viewer ? tab.viewer.title : tip(tab, v)}
          onpointerdown={(e) => onTabDown(e, tab.id)}
          onclick={(e) => {
            if (e.button === 0) void activateTab(tab.id);
          }}
          onauxclick={(e) => onAux(e, tab.id)}
          onkeydown={(e) => onTabKey(e, i)}>
          {#if tab.viewer}
            <span class="tabs-ico" aria-hidden="true">
              <svg viewBox="0 0 16 16" width="13" height="13"
                ><path
                  d="M4 1.8h5.2l3.3 3.3v8.4c0 .39-.31.7-.7.7H4c-.39 0-.7-.31-.7-.7v-11c0-.39.31-.7.7-.7ZM9 1.9v3.4h3.4M5.6 8.2h4.8M5.6 10.6h4.8"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1.2"
                  stroke-linecap="round"
                  stroke-linejoin="round" /></svg>
            </span>
            <span class="tabs-title">{tab.viewer.title || "PDF"}</span>
          {:else if v.kind === "tag"}
            {@const tv = v}
            <LabelBadge name={viewTitle(tv)} color={app.tags.find((x) => x.id === tv.id)?.color ?? null} />
          {:else}
            <!-- ★ Starred は題名の ★ がアイコンを兼ねる -->
            {#if v.kind !== "starred"}<span class="tabs-ico" aria-hidden="true">
                {#if v.kind === "collection"}
                  <svg viewBox="0 0 16 16" width="13" height="13"
                    ><path
                      d="M1.8 4.2c0-.66.54-1.2 1.2-1.2h3.1l1.5 1.6h5.4c.66 0 1.2.54 1.2 1.2v6.2c0 .66-.54 1.2-1.2 1.2H3c-.66 0-1.2-.54-1.2-1.2V4.2Z"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="1.3"
                      stroke-linejoin="round" /></svg>
                {:else if v.kind === "trash"}
                  <svg viewBox="0 0 16 16" width="13" height="13"
                    ><path
                      d="M2.8 4.3h10.4M6.2 4.3V2.9c0-.39.31-.7.7-.7h2.2c.39 0 .7.31.7.7v1.4M4.1 4.3l.6 8.6c.04.55.5.98 1.05.98h4.5c.55 0 1.01-.43 1.05-.98l.6-8.6"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="1.3"
                      stroke-linecap="round"
                      stroke-linejoin="round" /></svg>
                {:else if v.kind === "all"}
                  <svg viewBox="0 0 16 16" width="13" height="13"
                    ><path
                      d="M2.5 3h2.6v10H2.5zM6.4 3H9v10H6.4zM10.3 3.6l2.5-.7 2.4 9.6-2.5.7z"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="1.2"
                      stroke-linejoin="round" /></svg>
                {/if}
              </span>{/if}
            <span class="tabs-title">{viewTitle(v)}</span>
          {/if}
        </div>
        {#if app.tabs.length > 1 || tab.viewer}
          <button
            class="tabs-x"
            tabindex="-1"
            aria-hidden="true"
            title={front ? t("タブを閉じる（⌘W）") : t("タブを閉じる")}
            onclick={(e) => {
              if (e.button === 0) void closeTab(tab.id);
            }}
            onauxclick={(e) => onAux(e, tab.id)}>×</button>
        {/if}
      </div>
    {/each}
  </div>
  <button class="tabs-new" title={t("新しいタブ（⌘T）")} aria-label={t("新しいタブ")} onclick={() => void openTab()}>{t("＋")}</button>
</div>
