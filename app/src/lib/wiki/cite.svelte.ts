// OpenNote の文献の引用の窓（WikiCite）の状態と蔵書の検索。開き方 = ツールバーの [文献]・⌘⌥K・「/」の「文献を引用…」。
// 入れるのは edit.svelte の citeInsert（今のエディタの選択 → `[選択](openref://open?key=KEY)`・無ければ `[](…)`）。
// 検索の条件は Word の引用検索と同じ（list_items の search + metadata_only・上位 20）。全文索引は DOI と年を持たないので:
// - DOI・PMID・doi.org / PubMed の URL = wiki_cite_lookup（ネットワークなし）でその 1 件
// - 語の中の 4 桁の年 = 検索語から外して ItemSummary.year で絞る（語は AND で結ばれる = 年を語に残すと 0 件になる）
// CodeMirror はここで読まない（WikiPage と一緒に初期の bundle に入る = エディタは編集を開いたときの動的 import のまま）

import { api, type ItemSummary } from "../api";
import { invoke } from "./ipc";
import { citeCheck, citeInsert, ed, editNotice, editSelection } from "./edit.svelte";
import { errText } from "../err";

export const cite = $state({
  open: false,
  query: "",
  results: [] as ItemSummary[],
  /** 候補の選択（↑↓） */
  sel: 0,
  busy: false,
  /** 候補の代わりに出す一言（蔵書に無い・失敗） */
  note: "",
  /** 窓を開いたときのエディタの選択（リンクの文字になる・空 = 番号だけ） */
  selText: "",
  /** 窓を開いたときの編集（別のページ・別のワークスペースへ移ったら入れない） */
  path: null as string | null,
  root: null as string | null,
});

/** 間合い（テストで縮める） */
export const citeCfg = { delay: 150 };

/** wiki_cite_lookup の答え。null = 識別子ではない・item が null = 識別子だが蔵書に無い */
export interface CiteLookup {
  item: ItemSummary | null;
}
export const lookupCite = (input: string) => invoke<CiteLookup | null>("wiki_cite_lookup", { input });

const ID = /^(?:https?:\/\/\S+|doi:\s*\S+|10\.\d{4,9}\/\S+|pmid:\s*\d+|\d{5,9})$/i;
const YEAR = /^(?:1[89]|20)\d\d$/;

/** 打った語の仕分け: 識別子らしい 1 語なら id・それ以外は語（4 桁の年は外して year に。最初の 1 つだけ） */
export function citeQuery(q: string): { id: string | null; terms: string; year: number | null } {
  const s = q.trim();
  if (ID.test(s)) return { id: s, terms: s, year: null };
  let year: number | null = null;
  const rest: string[] = [];
  for (const w of s.split(/\s+/).filter(Boolean)) {
    if (year === null && YEAR.test(w)) year = Number(w);
    else rest.push(w);
  }
  return { id: null, terms: rest.join(" "), year };
}

/** 蔵書を探す。上位 20。note = 候補の代わりの一言 */
export async function searchCite(q: string): Promise<{ items: ItemSummary[]; note: string }> {
  const { id, terms, year } = citeQuery(q);
  if (id) {
    const r = await lookupCite(id);
    if (r) return r.item ? { items: [r.item], note: "" } : { items: [], note: "missing" };
    // 識別子として読めなかった（DOI でも PubMed でもない URL 等）= 普通の語として探す
  }
  if (!terms && year === null) return { items: [], note: "" };
  // 年で絞るときは多めに取ってから絞る（上位 20 だけを絞ると、その年の論文が 21 位以下にあっても出ない）
  const rows = await api.listItems({ search: terms || null, metadataOnly: true, limit: year === null ? 20 : 200 });
  const items = (year === null ? rows : rows.filter((r) => r.year === year)).slice(0, 20);
  return { items, note: "" };
}

let qseq = 0;
let timer: ReturnType<typeof setTimeout> | undefined;

/** 窓を開く（編集中だけ）。選択はここで読む（窓に案内を出す）。今の選択に置けない（コード・frontmatter・リンクの中・
    行をまたぐ選択）なら開かずに理由を知らせる（探してから入らない、を避ける） */
export function openCite(): boolean {
  if (!ed.path) return false;
  const why = citeCheck();
  if (why) {
    editNotice(`✗ ${why}`);
    return false;
  }
  clearTimeout(timer);
  qseq++;
  cite.path = ed.path;
  cite.root = ed.root;
  cite.selText = editSelection();
  cite.query = "";
  cite.results = [];
  cite.sel = 0;
  cite.busy = false;
  cite.note = "";
  cite.open = true;
  return true;
}

/** 窓を閉じる（入れない） */
export function closeCite() {
  clearTimeout(timer);
  qseq++;
  cite.open = false;
  cite.busy = false;
}

/** 打った語で探し直す（debounce・古い応答は捨てる） */
export function setCiteQuery(q: string) {
  cite.query = q;
  clearTimeout(timer);
  const my = ++qseq;
  if (!q.trim()) {
    cite.results = [];
    cite.note = "";
    cite.busy = false;
    return;
  }
  cite.busy = true;
  timer = setTimeout(async () => {
    try {
      const r = await searchCite(q);
      if (my !== qseq) return;
      cite.results = r.items;
      cite.note = r.note;
      cite.sel = 0;
    } catch (e) {
      if (my !== qseq) return;
      cite.results = [];
      cite.note = `✗ ${errText(e, 200)}`;
    } finally {
      if (my === qseq) cite.busy = false;
    }
  }, citeCfg.delay);
}

/** 候補を入れる（窓を開いたときと同じ編集のときだけ）。閉じてエディタへ戻る */
export function chooseCite(it: ItemSummary): boolean {
  const { path, root } = cite;
  closeCite();
  return citeInsert(it.key, it.retracted, path, root);
}
