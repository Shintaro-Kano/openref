<script lang="ts">
  /* OpenNote の中央 = 前面のタブのページ。ツールバー + kind で描画部品（md / html = render/ShadowPage・その他 = MediaView）を出し分け、
     部品の遷移の要求（WikiNav）は wiki.svelte.ts の navigate が実行する。ホーム（path = null）は vault の概要だけ。
     読み直し = 前面の path が変わった・索引の変更（wiki-changed）にこのページが入った。スクロール位置はタブ（TabWiki.scrollTop）に持つ。
     編集: [表示 / 編集 / 分割]（⌘E = 表示 ⇄ 編集・⌘S = 保存）。編集はエディタ（WikiEditor = CodeMirror の動的 import）、
     分割は左にエディタ・右にプレビュー（入力中の本文を debounce で描き直す・スクロール同期なし）。保存の規則は edit.svelte.ts。
     編集中（編集・分割）の右ペインの目次 = 入力中の本文から作る（render/outline.ts・id は表示と同じ規則・保存を待たない）。
     目次の押下（wk.scrollTo）はエディタがその見出しの行へ（分割のプレビューも同じ id で動く）。表示の目次と押下は描画部品（ShadowPage）のまま
     ワークスペース: ページはそのタブの root で読む（root と path の組が変われば読み直す）・設定を前面の root に揃え終えてから読む
     （rootReady = wiki_front_root が asset scope に足し終えた = 初めて前面にした root の画像が 403 のまま残らない）・編集はその root に固定 */
  import { onDestroy, tick, untrack } from "svelte";
  import { t } from "../i18n.svelte";
  import { composing } from "../input";
  import { app, covered, frontWiki, tabOpsAllowed } from "../state.svelte";
  import ShadowPage from "./render/ShadowPage.svelte";
  import MediaView from "./MediaView.svelte";
  import WikiResults from "./WikiResults.svelte";
  import WikiEditor from "./WikiEditor.svelte";
  import WikiConflict from "./WikiConflict.svelte";
  import WikiDialogs from "./WikiDialogs.svelte";
  import WikiCite from "./WikiCite.svelte";
  import { cite, openCite } from "./cite.svelte";
  import type { EditorHandle } from "./editor/cm";
  import type { WikiLink, WikiNav, WikiPageData } from "./types";
  import { absOf, baseName, refViewable, relInRoot } from "./render/asset";
  import { sourceHeadings } from "./render/outline";
  import { findHeading } from "./render/sanitize";
  import {
    goHistory,
    hasQuery,
    navigate,
    noteTitle,
    openPath,
    openWikiPage,
    readPage,
    requestFind,
    rootReady,
    setEditing,
    setHeadings,
    touched,
    vaultName,
    wk,
  } from "./wiki.svelte";
  import { clearSearch } from "./search.svelte";
  import { askNewPage } from "./files.svelte";
  import { loadRecent, recent, recentList } from "./recent.svelte";
  import { ed, editable, flush, refreshFromDisk, startEdit, stopEdit } from "./edit.svelte";
  import { inPage, selectionText, setChatEditor } from "./selection";
  import { countBody, countChars } from "./count";
  import { focusMode, toggleFocus } from "./focus.svelte";

  /** links の無いページの代わり（毎回新しい [] を作ると previewPage が shown の置換のたびに組み直される） */
  const NO_LINKS: WikiLink[] = [];
  const tw = $derived(frontWiki());
  const path = $derived(tw?.path ?? null);
  /** 前面のタブのワークスペース（分からなければ null = 画面の root で読む） */
  const root = $derived(tw?.root ?? null);
  let body = $state<HTMLDivElement | null>(null);
  let seq = 0;

  /** スクロール位置を戻す。後から伸びる中身（html の読込・`![[埋め込み]]`・画像）で届かなければ、伸びるたびに追う
      （利用者の操作・別のページ・見出しへの移動（still が false）・3 秒で止める）。追う間の onScroll は途中の値を書くが、届けば最後の値になる */
  let chase: (() => void) | null = null;
  function restoreScroll(el: HTMLDivElement, target: number, still: () => boolean) {
    chase?.();
    el.scrollTop = target;
    if (target <= 0 || el.scrollTop >= target - 1 || typeof ResizeObserver === "undefined") return;
    const stop = () => {
      ro.disconnect();
      clearTimeout(timer);
      el.removeEventListener("wheel", stop);
      el.removeEventListener("pointerdown", stop);
      window.removeEventListener("keydown", stop, true);
      if (chase === stop) chase = null;
    };
    const ro = new ResizeObserver(() => {
      if (!still()) return stop();
      el.scrollTop = target;
      if (el.scrollTop >= target - 1) stop();
    });
    const timer = setTimeout(stop, 3000);
    for (const c of Array.from(el.children)) ro.observe(c);
    el.addEventListener("wheel", stop, { passive: true });
    el.addEventListener("pointerdown", stop);
    window.addEventListener("keydown", stop, true);
    chase = stop;
  }

  async function load(p: string | null, r: string | null, keepScroll: boolean) {
    const my = ++seq;
    chase?.();
    const jump = wk.scrollTo?.nonce;
    wk.headings = [];
    if (!p) {
      wk.page = null;
      wk.pageError = null;
      return;
    }
    await rootReady(); // 前面の root を揃え終えてから（起動の門・タブの行き来の wiki_front_root）
    if (my !== seq) return;
    try {
      const page = await readPage(p, r);
      if (my !== seq) return;
      wk.page = page;
      wk.pageError = null;
    } catch (e) {
      if (my !== seq) return;
      wk.page = null;
      wk.pageError = String(e);
    }
    // 描画の後にタブの位置へ（遷移で開いた = 0・タブの切替で戻った = 前の位置）。ただし描画の間に見出しへ飛んだ
    // （`[[page#見出し]]` の着地 = onHeadings → jumpToHeading → scrollTo）ならその位置を残す（0 で上書きしない）
    await tick();
    if (wk.scrollTo?.nonce !== jump) return;
    const want = frontWiki();
    if (body && want && want.path === p) {
      restoreScroll(body, keepScroll ? want.scrollTop : 0, () => my === seq && frontWiki() === want && want.path === p && wk.scrollTo?.nonce === jump);
    }
  }

  // 前面の path か root が変わった（タブの切替・遷移・明示の切替）
  // load の同期部分が読む状態（wk.scrollTo 等）を依存にしない = path / root が変わったときだけ（untrack）
  // path が変わった = 前のページの見出しへの scrollTo を消す（タブの切替で戻った html ページが前のページの id へ着地しない。
  // `[[page#見出し]]` の着地は読込の後の onHeadings → jumpToHeading が新しく立てる）
  $effect(() => {
    const p = path;
    const r = root;
    untrack(() => {
      wk.scrollTo = null;
      void load(p, r, true);
    });
  });

  // 索引が変わった = このページが入っていれば読み直す（位置は保つ）。編集中は本文を edit.svelte の規則で
  // （未保存が無ければ黙って読み直す・あれば触らない = 次の保存が conflict）・題名やリンクだけ wk.page に反映
  let seenStamp = wk.stamp;
  $effect(() => {
    const s = wk.stamp;
    if (s === seenStamp) return;
    seenStamp = s;
    const p = frontWiki()?.path ?? null;
    if (!p || !touched(p)) return;
    untrack(() => {
      const r = frontWiki()?.root ?? null;
      if (ed.path === p && ed.root === r) void refreshFromDisk().then((pg) => pg && ed.path === p && adopt(pg));
      else void load(p, r, true);
    });
  });

  function onScroll() {
    const w = frontWiki();
    if (w && body && wk.page && w.path === wk.page.path) w.scrollTop = body.scrollTop;
  }

  const searching = $derived(wk.showResults && hasQuery());
  // 検索の結果一覧から本文へ戻った（本文の箱を作り直した）= このタブの位置へ戻す（今までは先頭に戻っていた）。
  // 効かせるのは結果一覧から本文へ戻ったときだけ（結果一覧から編集へ戻った・編集 → 表示は今どおり）。
  // 見出しへの移動（wk.scrollTo の nonce）が来たら追うのをやめる（load と同じ照合）
  let fromResults = false;
  $effect(() => {
    if (searching) fromResults = true;
    else if (editing) fromResults = false;
  });
  $effect(() => {
    const el = body;
    if (!el) return;
    untrack(() => {
      if (!fromResults) return;
      fromResults = false;
      const w = frontWiki();
      if (w && w.scrollTop > 0 && wk.page && w.path === wk.page.path && el.scrollTop === 0) {
        const jump = wk.scrollTo?.nonce;
        restoreScroll(el, w.scrollTop, () => frontWiki() === w && w.path === wk.page?.path && body === el && wk.scrollTo?.nonce === jump);
      }
    });
  });

  /** 入力欄・テキストエリア・contenteditable（CodeMirror）の中か */
  function typing(t: EventTarget | null): boolean {
    const el = t as HTMLElement | null;
    return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable);
  }

  /* キーが来た時点で覆いがあったか（window の capture の段で控える = ShelfViewer / PdfViewer の coveredAtKey と同じ型）。
     App の Esc の連鎖（取込の窓を閉じる closeImport）・窓自身の Esc の受け手は、このページの bubble の受け手より先に走って覆いを閉じる
     → bubble の時点の covered() だけを見ると、窓を閉じた同じ Esc で裏の検索まで消していた */
  let coveredAtKey = false;
  function onKeyCapture() {
    coveredAtKey = covered();
  }

  function onKey(e: KeyboardEvent) {
    if (!frontWiki()) return;
    const cmd = e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey;
    // ⌘S = 保存（編集していなければ何もしない。既定の動作も止める）
    if (cmd && e.key.toLowerCase() === "s") {
      e.preventDefault();
      if (!e.repeat && ed.path) void flush();
      return;
    }
    // 覆い（covered = 本体の窓・OpenNote / チャットの窓 = 引用の窓・新規 / 改名・競合・同意・救出 + 参照エディタ・取込）の下では、
    // 以下のページのキー（⌘[ ⌘]・検索中の Esc・⌘⌥K・⌘E・⌘F）を裏のページに効かせない（前は OpenNote の窓と app.modal だけを見ていて、
    // 参照エディタ・取込・同意・救出の下で ⌘E が裏を編集に切り替え・⌘⌥K が引用の窓を重ね・⌘F が裏で検索を開き・Esc が裏の検索を消した）。
    // キーは奪わない（素通し）: この 3 つ + Esc はメニューの項目に無い（main.rs の menu_spec = ⌘Q と Edit の標準だけ）= 流しても何も起きず、
    // 窓の中の入力欄の既定・窓自身の Esc に任せる（「字として入る / 窓の物は素通し」と同じ側）。⌘S は上 = 覆いの下でも保存する
    // 門はキーが来た時点の覆い（coveredAtKey）と今の覆いの両方を見る = 同じキーの処理の途中で別の受け手が覆いを閉じても、裏は動かない
    if (coveredAtKey || covered()) return;
    // ⌘[ / ⌘] = 戻る / 進む（このタブの中の履歴・ホームと検索の結果一覧からも）。入力欄・エディタの中では奪わない
    // （CodeMirror の字下げ = 同じキー・チャットの入力欄・html ページの中の入力欄 = shadow の中なので composedPath の頭で見る）。
    // 覆い（参照エディタ・チャットの説明・救出・取込）の下でも動かさない = タブの操作と同じ門。⌘⇧[ ] はタブの巡回（App）
    if (cmd && (e.key === "[" || e.key === "]") && !typing(e.composedPath()[0] ?? e.target) && tabOpsAllowed()) {
      e.preventDefault();
      if (!e.repeat && !composing(e)) void goHistory(e.key === "[" ? -1 : 1);
      return;
    }
    // 結果一覧の Esc = 検索を消して木（ページ）に戻る（検索欄の中の Esc は検索欄が先に扱う）。
    // IME の取消 Esc は入力欄が stopPropagation しない = ここで弾く（App.svelte の Esc と同じ形・欄ごとの手当てに頼らない）
    if (searching && e.key === "Escape" && !composing(e)) {
      clearSearch();
      return;
    }
    if (!wk.page || searching) return;
    // ⌘⌥K = 文献を引用（md の編集中だけ）。⌥K は配列により別の字（˚ 等）= e.code で見る
    if (e.metaKey && e.altKey && !e.ctrlKey && !e.shiftKey && e.code === "KeyK") {
      if (!citeOn || cite.open) return;
      e.preventDefault();
      if (!e.repeat && !composing(e)) openCite();
      return;
    }
    // ⌘E = 表示 ⇄ 編集（編集へは前回の配置 = エディタだけ / 分割）
    if (cmd && e.key.toLowerCase() === "e" && canEdit) {
      e.preventDefault();
      if (!e.repeat && !composing(e)) void setEditing(!editing);
      return;
    }
    if (cmd && e.key.toLowerCase() === "f") {
      if (e.defaultPrevented) return; // CodeMirror が自分の検索を開いた
      e.preventDefault();
      if (editing) edHandle?.openSearch(); // 編集中は CodeMirror の検索（置換つき）
      else requestFind();
    }
  }

  const shown = $derived(wk.page && wk.page.path === path ? wk.page : null);
  /** 参照フォルダのファイル: 描画部品には root = そのフォルダ・path = その相対で渡す（相対リンク・CSS・画像の解決がワークスペースと同じ式で効く）。
      部品が返す「root 相対のページ」は絶対パスに戻し、内蔵で開ける種類なら ref・それ以外（md 等）はホストの open へ */
  const external = $derived(shown?.external ?? null);
  const viewPage = $derived(shown && external ? { ...shown, path: relInRoot(external, shown.abs_path) ?? shown.abs_path } : shown);
  function navigateRef(nav: WikiNav, newTab?: boolean) {
    if (nav.kind === "page" && external) {
      const abs = absOf(external, nav.path);
      // 参照フォルダが vault を含むとき（vault の親を参照フォルダにできる）: vault の中のファイルは vault のページとして開く（ref の別タブに二重に出さない）
      const ws = root ?? wk.status?.root;
      const inVault = ws ? relInRoot(ws, abs) : null;
      if (inVault) return navigate({ kind: "page", path: inVault, heading: nav.heading }, newTab);
      return navigate(refViewable(abs) ? { kind: "ref", abs } : { kind: "open", abs }, newTab);
    }
    return navigate(nav, newTab);
  }
  /** 描画部品の props のうち page 以外（page は表示 = viewPage・分割のプレビュー = previewPage を別に渡す）。
      shown の同一性には依存させない（保存のたびに wk.page が置き換わる → spread の props が新しくなると部品の描画 effect が走る）
      = 依存は有無（viewOn）・external・root・参照フォルダ・scrollTo・findRequest・見出しの色だけ */
  const viewOn = $derived(!!shown && !!viewPage);
  const viewBase = $derived(
    viewOn
      ? {
          root: external ?? root ?? wk.status?.root ?? "",
          refs: wk.refDirs,
          onNavigate: external ? navigateRef : navigate,
          readPage,
          onHeadings: setHeadings,
          scrollTo: wk.scrollTo,
          findRequest: wk.findRequest,
          headingColors: wk.headColors,
        }
      : null
  );

  /** 編集中に読み直したページを wk.page に（題名・tags・リンク = 右ペインとプレビューのリンクの解決） */
  function adopt(pg: WikiPageData) {
    noteTitle(pg);
    if (frontWiki()?.path === pg.path) wk.page = pg;
  }

  // ---- 編集 ----
  // 編集中のページは編集を続ける（外部の書換えで読みが途中まで・U+FFFD 入りになっても、エディタの本文を捨てない）
  const canEdit = $derived(!!shown && (ed.path === shown.path || editable(shown)));
  const editing = $derived(!!tw?.editing && canEdit);
  // 編集のセッション = 前面のタブの編集中のページ（タブの切替・ページの移動は先に保存してから = wiki.svelte / state.svelte の leave）
  $effect(() => {
    const p = shown;
    const on = editing;
    const r = root;
    untrack(() => {
      if (on && p) startEdit(p, r); // 編集はこのタブの root に固定（ed.root）
      else if (ed.path && (!p || ed.path !== p.path || ed.root !== r || !on)) stopEdit();
    });
  });
  onDestroy(() => {
    chase?.();
    stopEdit();
    setChatEditor(null);
  });
  // 編集を閉じた = チャットの選択の読み取りにエディタを使わない
  $effect(() => {
    if (!editing) untrack(() => setChatEditor(null));
  });

  let edHandle: EditorHandle | null = null;

  // ---- 編集中の目次: 入力中の本文（debounce = ed.preview）から見出しを作る。分割のプレビューの onHeadings は使わない（受け手を 1 つに）----
  // 上の編集のセッションの effect（startEdit = ed.preview を新しいページの本文に）より後に置く = 同じ flush でその後に走る
  // （前に置くと、編集に入った直後に前のページの本文 = 終えた編集の ed.preview の目次が 1 拍出る。edit.test ㉗ が見張る）
  const editKind = $derived<"md" | "html">(shown?.kind === "html" ? "html" : "md");
  $effect(() => {
    if (!editing || !shown) return;
    const text = ed.preview;
    const kind = editKind;
    untrack(() => setHeadings(sourceHeadings(text, kind, document)));
  });
  const ignoreHeadings = () => {};
  /** 目次の押下（nonce が変わるたび）= 編集中ならエディタをその見出しの行へ。行は押した時点のエディタの本文で引き直す
      （目次は debounce の分だけ古い）。id で引けなければ見出しの文字で。エディタがまだ無い（動的 import の間）なら開いた後に */
  let seenJump = untrack(() => wk.scrollTo?.nonce ?? null);
  let pendingJump: string | null = null;
  function jumpEditor(id: string) {
    const h = edHandle;
    if (!h) {
      pendingJump = id;
      return;
    }
    const heads = sourceHeadings(h.getText(), editKind, document);
    const want = wk.headings.find((x) => x.id === id);
    const hit = heads.find((x) => x.id === id) ?? (want ? findHeading(heads, want.text) : undefined) ?? findHeading(heads, id);
    if (hit) h.gotoLine(hit.line);
  }
  $effect(() => {
    const s = wk.scrollTo;
    if (!s || s.nonce === seenJump) return;
    seenJump = s.nonce;
    untrack(() => {
      if (editing && shown) jumpEditor(s.id);
    });
  });
  // 編集を閉じた・別のページのエディタに作り直す（{#key shown.path}）= エディタの handle と待ちの移動を捨てる（閉じたエディタへ dispatch しない）
  const shownPath = $derived(shown?.path ?? null);
  $effect(() => {
    void shownPath;
    const on = editing;
    untrack(() => {
      edHandle = null;
      if (!on) pendingJump = null;
    });
  });
  // 分割のプレビュー: 保存のたびに wk.page が新しくなっても同じ本文を描き直さない = shown の同一性ではなく path と ed.preview だけから組む
  // （描画部品が finish に渡す root 相対の path と links は viewPage から・本文は ed.preview。保存で変わる sha256 / size / mtime は描画に無関係）
  const previewOn = $derived(!!shown && editing);
  const previewPath = $derived(viewPage?.path ?? null);
  const previewLinks = $derived(shown?.links ?? NO_LINKS);
  const previewPage = $derived.by((): WikiPageData | null => {
    if (!previewOn || previewPath == null) return null;
    const path = previewPath;
    const links = previewLinks;
    const content = ed.preview;
    const base = untrack(() => viewPage);
    return base ? { ...base, path, links, content, truncated: false } : null;
  });
  // ---- 字数（md だけ）: 本文 = 描画したときに見える文字（編集中は入力の debounce の本文）・選択 = 選んだ文字（count.ts）----
  const bodyCount = $derived(shown?.kind === "md" ? countBody(editing ? ed.preview : (shown.content ?? "")) : null);
  let selCount = $state(0);
  /** selectionchange の次のタスクで読む（CodeMirror が自分の選択を反映した後 = viewing.svelte と同じ）。ページの外の選択は 0。
      isCollapsed では弾かない（表示の shadow DOM の選択は文書の選択に畳まれて見えることがある = selectionText が中を読む） */
  function readSelCount() {
    const s = document.getSelection();
    if (!s) {
      selCount = 0;
      return;
    }
    const { text, node } = selectionText(s);
    selCount = inPage(node) ? countChars(text) : 0;
  }
  $effect(() => {
    const on = () => setTimeout(readSelCount, 0);
    document.addEventListener("selectionchange", on);
    return () => document.removeEventListener("selectionchange", on);
  });

  /** 文献の引用が使える = md の編集中（html のソースには入れない） */
  const citeOn = $derived(editing && shown?.kind === "md");
  const saveLabel = $derived(
    ed.state === "saving" ? t("保存中…")
    : ed.state === "saved" ? t("保存済み::wiki")
    : ed.state === "dirty" ? t("未保存")
    : ed.state === "error" ? t("⚠ 保存に失敗")
    : ed.state === "conflict" ? t("⚠ 競合")
    : ""
  );

  // ---- ホーム ----
  /** 木がこのワークスペースの物で読めている（起動直後・切替の直後は別の root の木か空 = まだ） */
  const treeReady = $derived(!!wk.status?.root && wk.treeRoot === wk.status.root);
  /** ホームの最近開いたページ（検索欄の一覧と同じ recentList = 画面のワークスペース・今の木に在る物・10 件）。木と記録を読めるまでは出さない
      （別の root の木で絞ると、同じパスの別のページを出しうる） */
  const homeRecents = $derived(path || !treeReady || !recent.loaded ? [] : recentList());
  /** 「0 件」と言ってよい = 記録と木の両方を読めた後（読み込み中・作り直し中・木の失敗でない）で本当に 0 件 */
  const homeEmpty = $derived(!homeRecents.length && recent.loaded && treeReady && !wk.treeLoading && !wk.treeError && !wk.status?.rebuilding);
  /** 読み込み中（記録 / 木の読み込み・作り直し・切替の直後）。木を読めなかったとき・状態（wiki_status）を読めないときは言わない
      （木の失敗は左の木が・状態の失敗はヘッダーの警告が出す = 中央で二重に出さない） */
  const homeLoading = $derived(!homeRecents.length && !homeEmpty && !recent.readError && !wk.treeError && !wk.status?.statusError);
  // ホームを出したら記録を読む（起動直後・検索欄に入る前でも出す。読めていれば何もしない）
  $effect(() => {
    if (!path) void loadRecent();
  });
  /** 行: 押す = 開く・⌘クリック / 中クリック = 新しいタブ（ほかの一覧と同じ）。覆いの下では何もしない */
  function pickHome(e: MouseEvent, p: string) {
    if (covered()) return;
    if (e.metaKey || e.button === 1) {
      e.preventDefault();
      void openWikiPage(p, true);
    } else if (e.button === 0) void openWikiPage(p);
  }
  /** ⌘Enter = 新しいタブ（素の Enter / Space はボタンの click = 開く） */
  function homeKey(e: KeyboardEvent, p: string) {
    if (e.key !== "Enter" || !e.metaKey || composing(e) || covered()) return;
    e.preventDefault();
    void openWikiPage(p, true);
  }
  /** [＋ 新規ページ…] = [＋ 新規] の「新規ページ…」と同じ窓（作る場所 = ワークスペースの直下） */
  function newPage() {
    if (covered()) return;
    askNewPage("");
  }

  /** [⋯] = ページの操作（WikiDialogs の page のメニュー）: 外部で開く・分割（編集できるページ）・見出しの色（md の表示中）・名前の変更・複製・ゴミ箱 */
  function openMenu(e: MouseEvent | KeyboardEvent, keyboard: boolean) {
    // 覆い（窓・参照エディタ・取込）の間は開かない（ページのキーの門と同じ述語 = 窓の上にメニュー・別の窓を重ねない。ボタンの keydown / click は門を通らない）
    if (!shown || covered()) return;
    const opener = e.currentTarget as HTMLElement;
    const r = opener.getBoundingClientRect();
    wk.menu = { kind: "page", path: shown.path, x: Math.max(8, r.right - 200), y: r.bottom + 4, abs: shown.abs_path, canEdit, md: shown.kind === "md", keyboard, opener };
  }
  /** [⋯] のキー: Enter / Space / ↓ = 開いて最初の項目へ（キーの click = detail 0 も同じ扱い） */
  function moreKey(e: KeyboardEvent) {
    if (composing(e) || e.metaKey || e.ctrlKey || e.altKey || covered()) return; // 覆いの間は素通し（奪わない = ページのキーの門と同じ）
    if (e.key === "Enter" || e.key === " " || e.key === "ArrowDown") {
      e.preventDefault();
      openMenu(e, true);
    }
  }
</script>

<svelte:window onkeydowncapture={onKeyCapture} onkeydown={onKey} />

<section class="wk-page" class:wk-focus={focusMode.on}>
  {#if focusMode.on && (searching || !path)}
    <!-- ツールバーの無い画面（ホーム・検索の結果）でも札を出す（戻る手段を見せる） -->
    <button class="wk-focus-tag wk-focus-float" aria-pressed="true" title={t("集中モードを終える（⌘⌥S）")} onclick={toggleFocus}>{t("集中")} ×</button>
  {/if}
  {#if searching}
    <WikiResults />
  {:else if !path}
    <!-- ホーム: 最近開いたページ（検索欄の「最近」の一覧と同じ記録 = ui_wiki_recent・ワークスペースごと・今の木に在る物だけ・10 件）と
         新規ページ（[＋ 新規] の「新規ページ…」と同じ窓・ワークスペースの直下）。root・ページ数・ingest/ の素材は右ペインのホームと重なる = 中央からは外した -->
    <div class="wk-home">
      <h2>{vaultName() || "OpenNote"}</h2>
      <div class="wk-home-head">
        <h3>{t("最近開いたページ")}</h3>
        <button class="wk-btn" onclick={newPage}>{t("＋")} {t("新規ページ…")}</button>
      </div>
      {#if homeRecents.length}
        <ul class="wk-home-list" aria-label={t("最近開いたページ")}>
          {#each homeRecents as r (r.path)}
            <li>
              <button
                class="wk-home-recent"
                title={r.path}
                onclick={(e) => pickHome(e, r.path)}
                onauxclick={(e) => pickHome(e, r.path)}
                onkeydown={(e) => homeKey(e, r.path)}>
                <span class="wk-home-title">{r.title}</span>
                <span class="wk-home-path">{r.path}</span>
              </button>
            </li>
          {/each}
        </ul>
      {:else if recent.readError}
        <p class="wk-dim">{t("最近開いたページを読めませんでした")}</p>
      {:else if homeEmpty}
        <p class="wk-dim">{t("最近開いたページはありません")}</p>
        <p class="wk-dim">{t("左の木からページを選ぶか、検索欄で探してください")}</p>
      {:else if homeLoading}
        <p class="wk-dim">{t("読込中…")}</p>
      {/if}
    </div>
  {:else}
    <div class="wk-bar">
      <div class="wk-bar-title" title={path}>
        <span class="wk-bar-name">{shown?.title ?? baseName(path)}</span>
        <span class="wk-bar-path">{path}</span>
      </div>
      {#if bodyCount !== null}
        <span class="wk-count" title={t("本文の字数（記法の記号・リンクの URL・frontmatter・空白と改行を除く）")}>
          {#if selCount}<span class="wk-count-sel">{t("選択::count")} {selCount.toLocaleString()} {t("字")}</span>{/if}
          {bodyCount.toLocaleString()} {t("字")}
        </span>
      {/if}
      {#if editing && saveLabel}
        <button
          class="wk-save wk-save-{ed.state}"
          title={ed.error ?? (ed.state === "conflict" ? t("外部で書き換えられています（⌘S で選ぶ）") : t("⌘S で今すぐ保存"))}
          onclick={() => void flush()}>{saveLabel}</button>
      {/if}
      {#if citeOn}
        <button class="wk-btn" title={t("蔵書から文献を引用（⌘⌥K）")} onclick={() => openCite()}>{t("文献::cite")}</button>
      {/if}
      {#if canEdit}
        <!-- [編集] = 表示 ⇄ 編集のトグル（⌘E と同じ = 編集へは前回の配置 = エディタだけ / 分割）。分割の切替は [⋯] の中 -->
        <button
          class="wk-btn wk-edit-btn"
          class:wk-edit-on={editing}
          aria-pressed={editing}
          title={editing ? t("表示（⌘E で切替）") : t("編集（⌘E で切替）")}
          onclick={() => void setEditing(!editing)}>{shown?.kind === "html" ? t("ソース") : t("編集")}</button>
      {:else if shown && !external && (shown.kind === "md" || shown.kind === "html")}
        <span class="wk-ro" title={shown.truncated ? t("長いページのため途中までしか読めていません（全体を書き戻せないので編集できません）") : t("UTF-8 でない文字を含むため編集できません（書き戻すと文字が壊れます）")}>{t("編集不可")}</span>
      {/if}
      {#if external}
        <span class="wk-ro" title={external}>{t("参照フォルダ（読み取り専用）")}</span>
      {/if}
      {#if shown}
        {#if external}
          <!-- 参照フォルダのページは [⋯] が無い（名前の変更・ゴミ箱をしない）= [外部で開く] はボタンのまま -->
          <button class="wk-btn" title={t("既定のアプリで開く")} onclick={() => void openPath(shown.abs_path)}>{t("外部で開く")}</button>
        {:else}
          <button class="wk-btn wk-more" title={t("ページの操作（外部で開く・分割・見出しの色・名前の変更 ほか）")} aria-label={t("ページの操作（外部で開く・分割・見出しの色・名前の変更 ほか）")} aria-haspopup="menu" aria-expanded={wk.menu?.kind === "page"} onclick={(e) => openMenu(e, e.detail === 0)} onkeydown={moreKey}>⋯</button>
        {/if}
      {/if}
      <!-- 集中モード: 入ると右上（ツールバーの右端）が「集中 ×」の札になる（押すか ⌘⌥S で戻る） -->
      {#if focusMode.on}
        <button class="wk-focus-tag" aria-pressed="true" title={t("集中モードを終える（⌘⌥S）")} onclick={toggleFocus}>{t("集中")} ×</button>
      {:else}
        <button class="wk-btn" aria-pressed="false" title={t("集中モード: 木・右ペイン・ヘッダーを隠して本文だけにする（⌘⌥S）")} onclick={toggleFocus}>{t("集中")}</button>
      {/if}
    </div>
    {#if editing && shown && previewPage && viewBase}
      <div class="wk-edit" class:wk-split={wk.split}>
        <div class="wk-edit-pane">
          {#key shown.path}
            <WikiEditor mode={shown.kind === "html" ? "html" : "md"} onReady={(h) => {
              edHandle = h;
              setChatEditor(h);
              const id = pendingJump;
              pendingJump = null;
              if (id) jumpEditor(id);
            }} />
          {/key}
        </div>
        {#if wk.split}
          <div class="wk-preview wk-body">
            <div class="wk-sheet">
              {#if shown.kind === "md"}
                <ShadowPage {...viewBase} onHeadings={ignoreHeadings} page={previewPage} mode="md" />
              {:else}
                <ShadowPage {...viewBase} onHeadings={ignoreHeadings} page={previewPage} mode="html" />
              {/if}
            </div>
          </div>
        {/if}
      </div>
    {:else}
    <div class="wk-body" bind:this={body} onscroll={onScroll}>
      {#if wk.pageError && !shown}
        <p class="wk-err">{wk.pageError}</p>
      {:else if !shown}
        <p class="wk-dim wk-pad">{t("読込中…")}</p>
      {:else}
        {#if shown.truncated}
          <p class="wk-warn">{t("長いページのため途中までを表示しています")} · {shown.chars} {t("字")}</p>
        {/if}
        {#if viewBase && viewPage && shown.kind === "md"}
          <div class="wk-sheet"><ShadowPage {...viewBase} page={viewPage} mode="md" /></div>
        {:else if viewBase && viewPage && shown.kind === "html"}
          <div class="wk-sheet"><ShadowPage {...viewBase} page={viewPage} mode="html" /></div>
        {:else if viewBase && viewPage && shown.kind === "media"}
          <MediaView {...viewBase} page={viewPage} />
        {:else}
          <div class="wk-other">
            <p>{t("このファイルは外部のアプリで開きます")}</p>
            <button class="wk-btn" onclick={() => void openPath(shown.abs_path)}>{t("外部で開く")}</button>
          </div>
        {/if}
      {/if}
    </div>
    {/if}
  {/if}
</section>
{#if ed.dialog && ed.path}<WikiConflict onReloaded={adopt} />{/if}
{#if cite.open}<WikiCite />{/if}
<WikiDialogs />

<style>
  /* position は持たない（通常の配置）= 左の木（.sidepane = position: relative）の影を、後ろの .wk-page が覆わない（MY LIBRARY の .list と同じ・45c）。
     集中モードの右上の札（.wk-focus-float = absolute）の基準は、集中モードのときだけ（そのとき木は隠れている） */
  .wk-page { display: flex; flex-direction: column; min-width: 0; min-height: 0; background: var(--panel); }
  :global(main.wk-main-focus) > .wk-page { position: relative; }
  .wk-bar {
    display: flex; align-items: center; gap: 8px; padding: 6px 14px; border-bottom: 1px solid var(--border); flex-shrink: 0;
  }
  .wk-bar-title { flex: 1; min-width: 0; display: flex; align-items: baseline; gap: 8px; overflow: hidden; }
  .wk-bar-name { font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .wk-bar-path { color: var(--dim); font-size: 11px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .wk-btn {
    border: 1px solid var(--border); border-radius: 6px; padding: 3px 10px; background: var(--bg); font-size: 12px; cursor: pointer;
    color: var(--text); white-space: nowrap;
  }
  .wk-btn:hover { border-color: var(--accent); }
  /* [編集] の押し込まれた状態（編集中）= [集中] の ON（集中 × の札）と同じ見せ方 = アクセントの枠 + --accent-soft の面（新しい色は作らない） */
  .wk-btn.wk-edit-on { background: var(--accent-soft); border-color: var(--accent); }
  .wk-body { flex: 1; min-height: 0; overflow: auto; container-type: inline-size; }
  /* md / html の外周の余白（中央の幅に応じて縮む = cqi は .wk-body の幅）。MediaView は自前の余白のまま */
  .wk-sheet { padding: clamp(14px, 3cqi, 26px) clamp(16px, 5cqi, 40px) 8px; }
  .wk-home { padding: 32px; max-width: 640px; }
  .wk-home h2 { margin: 0 0 4px; }
  /* 最近開いたページ = 検索欄の「最近」の一覧の行と同じ見た目（題名 + パス・押せる行・hover / 焦点で --accent-soft） */
  .wk-home-head { display: flex; align-items: center; gap: 12px; margin: 20px 0 6px; }
  .wk-home-head h3 { margin: 0; font-size: 12px; font-weight: 600; color: var(--dim); }
  .wk-home-head .wk-btn { margin-left: auto; }
  .wk-home-list { display: flex; flex-direction: column; gap: 1px; margin: 0; padding: 0; list-style: none; }
  .wk-home-recent {
    display: flex; flex-direction: column; align-items: flex-start; min-width: 0; width: 100%; padding: 4px 8px; border-radius: 5px;
    text-align: left; cursor: pointer;
  }
  .wk-home-recent:hover, .wk-home-recent:focus-visible { background: var(--accent-soft); outline: none; }
  .wk-home-title { max-width: 100%; font-size: 12.5px; color: var(--text); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .wk-home-path { max-width: 100%; font-size: 10.5px; color: var(--dim); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .wk-dim { color: var(--dim); font-size: 12px; overflow-wrap: anywhere; }
  .wk-pad { padding: 16px 20px; }
  .wk-err { margin: 16px 20px; color: var(--danger); white-space: pre-wrap; overflow-wrap: anywhere; }
  .wk-warn { margin: 8px clamp(16px, 5cqi, 40px) 0; font-size: 12px; color: var(--dim); }
  .wk-other { padding: 32px 20px; display: flex; flex-direction: column; align-items: flex-start; gap: 8px; }
  .wk-btn.wk-more { padding: 3px 8px; letter-spacing: 1px; }
  .wk-save { font-size: 11.5px; color: var(--dim); white-space: nowrap; padding: 2px 4px; }
  .wk-save-error, .wk-save-conflict { color: var(--danger); }
  .wk-save-dirty { color: var(--text); }
  .wk-ro { font-size: 11.5px; color: var(--dim); white-space: nowrap; }
  /* 集中モード（App の main.wk-main-focus = 1 列）。木と右ペインは作り直さずに表示だけ隠す: 列から外して同じ幅のまま重ね、
     visibility で消す（幅が変わらない = 中身が折り返し直さず、スクロール位置も保つ・focus も入らない）。本文は読みやすい幅で中央に。
     230 / 380 = sidebarw.svelte.ts の SIDEBAR_DEFAULT / RIGHT_W と対（app.css の main の格子と同じ数字） */
  :global(main.wk-main-focus) { grid-template-columns: minmax(0, 1fr); position: relative; }
  :global(main.wk-main-focus > .wk-sidebar),
  :global(main.wk-main-focus > .wk-info) { position: absolute; top: 0; bottom: 0; visibility: hidden; pointer-events: none; }
  :global(main.wk-main-focus > .wk-sidebar) { left: 0; width: var(--sb-w, 230px); }
  :global(main.wk-main-focus > .wk-info) { right: 0; width: 380px; }
  .wk-focus .wk-sheet { max-width: 46em; margin: 0 auto; }
  .wk-focus .wk-edit-pane :global(.cm-content) { max-width: 46em; margin: 0 auto; }
  .wk-focus-tag {
    font-size: 11.5px; padding: 2px 10px; border-radius: 999px; border: 1px solid var(--accent); background: var(--accent-soft);
    color: var(--text); white-space: nowrap; cursor: pointer;
  }
  .wk-focus-float { position: absolute; top: 8px; right: 14px; z-index: 2; }
  .wk-count { font-size: 11.5px; color: var(--dim); white-space: nowrap; font-variant-numeric: tabular-nums; }
  .wk-count-sel { color: var(--text); margin-right: 8px; }
  .wk-edit { flex: 1; min-height: 0; display: grid; grid-template-columns: minmax(0, 1fr); }
  .wk-edit.wk-split { grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); }
  .wk-edit-pane { min-width: 0; min-height: 0; overflow: hidden; }
  .wk-split .wk-edit-pane { border-right: 1px solid var(--border); }
  .wk-preview { min-width: 0; }
</style>
