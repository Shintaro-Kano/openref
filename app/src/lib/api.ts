import { invoke } from "@tauri-apps/api/core";

export interface MigrationConfig {
  format: "paperpile" | "zotero";
  path: string;
  roots: string[];
  mode: "copy" | "link";
  source_id: string | null;
  adopt_legacy_ids: boolean;
  decisions?: MigrationDecisions;
}
export interface MigrationDecisions {
  source_id:string; input_sha256:string;
  items:{object_id:string;item_key:string;csl_sha256?:string|null}[];
  files:{object_id:string;attachment_id:string;path:string}[];
}
export interface MigrationPlan {
  input_sha256:string;
  source_id: string;
  managed_storage: string | null;
  counts: Record<"new_items" | "existing_items" | "review" | "blocked" | "folders" | "notes" | "tags" | "available_pdfs" | "existing_pdfs" | "missing_pdfs" | "ambiguous_pdfs" | "unsupported_attachments" | "failed_pdfs" | "copy_bytes_upper_bound", number>;
  warnings: string[];
  items: {object_id:string; title:string; allow_new:boolean; candidate_details:{key:string;csl_sha256:string;title:string;doi:string|null;pmid:string|null}[]; file_options:{object_id:string;attachment_id:string;path:string}[]; selection:{item_key:string}|null; selected_files:string[]; action:{action:string; item_key?:string; candidates?:string[]; reason?:string}; warnings:string[]; attachments:{id:string; status:string; path?:string; candidates?:string[]; reason?:string}[]}[];
}
export interface MigrationReport {
  run_id:string; source_id:string; state:string; total:number; complete:boolean; fulltext_pending:number; backup_path:string|null; warnings:string[];
  results:{object_id:string; status:string; item_key:string|null; warnings:string[]; attachments:{id:string;status:string;detail:string|null}[]}[];
}
export interface MigrationStatus {
  id:string; phase:string; progress:{phase:string;completed:number;total:number;bytes:number}|null;
  config:MigrationConfig|null; plan:MigrationPlan|null; report:MigrationReport|null; error:string|null;
}
export interface MigrationHistory {id:string;format:string;source_id:string;state:string;attachment_mode:string;date_added:string;total:number}
export const migrationApi = {
  preview:(config:MigrationConfig)=>invoke<string>("migration_preview",{config}),
  status:()=>invoke<MigrationStatus|null>("migration_status"),
  start:(id:string)=>invoke<void>("migration_start",{id}),
  cancel:(id:string)=>invoke<void>("migration_cancel",{id}),
  index:(id:string)=>invoke<void>("migration_index",{id}),
  selectReport:(run:string)=>invoke<void>("migration_select_report",{run}),
  history:()=>invoke<MigrationHistory[]>("migration_history"),
  save:(id:string,path:string)=>invoke<void>("migration_save_report",{id,path}),
};

// ---- 型: core（crates/openref-core）の行型と 1:1（Rust 側の `#[derive(Serialize)]`・camelCase を手で写す。自動生成はしない）。
//      鍵の並びは Rust の定義順。core に足した列はここにも足す（使わなくても）

/** = core search::ItemSummary（一覧の行。ListedItem::summary が整形した表示用の項目だけ・CSL は持たない） */
export interface ItemSummary {
  key: string;
  title: string;
  firstAuthor: string;
  /** 筆頭著者の given（著者チップのラベルと頭文字絞込用） */
  firstAuthorGiven: string | null;
  year: number | null;
  /** 誌名（short があれば short） */
  container: string;
  starred: boolean;
  hasPdf: boolean;
  retracted: boolean;
  /** メモ（notes）あり — 行のインジケータ用 */
  hasNote: boolean;
  /** 検索語が添付 PDF の本文に一致したときの抜粋（一致箇所は U+E000 … U+E001 で囲まれる） */
  fulltextSnippet: string | null;
  dateAdded: string;
  /** 付いているラベル（行のバッジ・名前順） */
  tags: TagRef[];
}

/** = core search::TagRef（行・詳細に付くラベルの参照） */
export interface TagRef {
  id: number;
  name: string;
  color: string | null;
}

/** = core search::CollectionRef（詳細に付くフォルダの参照） */
export interface CollectionRef {
  id: number;
  name: string;
}

/** = core model::ItemRow（items の 1 行。ItemDetail に平坦に含まれる） */
export interface ItemRow {
  id: number;
  key: string;
  csl: any;
  starred: boolean;
  dateAdded: string;
}

/** = core search::AttachmentRow */
export interface AttachmentRow {
  id: number;
  /** 添付の key（8 桁。MCP の get_item が返すのと同じ） */
  key: string;
  mode: string;
  path: string;
  exists: boolean;
  pageCount: number | null;
  /** 自動取得 PDF の同一性照合: 'match' | 'unknown' | null（未照合） */
  doiCheck: string | null;
}

/** = core search::ItemDetail（item の列は平坦 = ItemRow をそのまま含む。ゴミ箱の item も引ける） */
export interface ItemDetail extends ItemRow {
  /** ゴミ箱にある */
  deleted: boolean;
  attachments: AttachmentRow[];
  collections: CollectionRef[];
  tags: TagRef[];
  retraction: string | null;
  /** item 単位のメモ（user + MCP 経由の Claude） */
  notes: NoteRow[];
}

/** = core tags::TagRow（Manage labels・サイドバーの LABELS） */
export interface TagRow {
  id: number;
  name: string;
  color: string | null;
  sortOrder: number;
  nItems: number;
}

/** = core search::CollectionRow（サイドバーのフォルダ） */
export interface CollectionRow {
  id: number;
  /** 不変キー（id は削除後に再利用されうる。UI の永続 = 折畳はこちらで持つ） */
  key: string;
  name: string;
  parentId: number | null;
  /** 直下の論文の数（ゴミ箱は数えない） */
  nItems: number;
}

/** = core edit::Purged（完全削除 1 件の結果・purge_item） */
export interface Purged {
  key: string;
  /** OpenRef 管理（storage/）から削除したファイル数 */
  filesRemoved: number;
  /** linked（Drive 等）のため残したファイル数 */
  filesKept: number;
  /** 削除に失敗した managed ファイル数 */
  filesFailed: number;
}

/** = core search::ViewCounts */
export interface ViewCounts {
  all: number;
  starred: number;
  trash: number;
}

/** = core search::Membership（選択中 item 群の所属集計。各要素は [collection_id または tag_id, keys のうち所属する件数]。
    0 件の所属先は含まれない） */
export interface Membership {
  collections: [number, number][];
  tags: [number, number][];
}

/** = core pdfingest::ImportSummary（PDF 取込 1 件の結果。status の語は Rust 側のコメントと同じ） */
export type ImportStatus =
  | "added"
  | "attached"
  | "attached_additional"
  | "needs_review"
  | "no_id"
  | "duplicate"
  | "error";
export interface ImportSummary {
  status: ImportStatus;
  key: string | null;
  /** 理由（要確認の根拠・Drive → managed 降格）も入る */
  message: string;
}

/** = core search::ListFilter（無い項目は Rust 側の既定値。search と他条件は併用可） */
export interface ListFilter {
  search?: string | null;
  collectionId?: number | null;
  /** ラベル名で絞る（MCP / CLI 用・大小無視）。GUI は tagId を使う */
  tag?: string | null;
  /** ラベルビュー: id で絞る（名前だと改名した瞬間に空になる） */
  tagId?: number | null;
  starredOnly?: boolean;
  trashed?: boolean;
  /** 0 = 既定（200） */
  limit?: number;
  /** 著者チップ: 姓（完全一致・大小無視）+ given 頭文字。著者の位置は問わない。空白だけは未指定扱い */
  authorFamily?: string | null;
  authorInitial?: string | null;
  /** 誌名チップ: container-title / container-title-short の完全一致 */
  journal?: string | null;
  /** true = PDF 本文を検索しない（Word アドイン用。GUI は使わない = 本文も検索） */
  metadataOnly?: boolean;
}

/** = core types::FieldDef */
export interface FieldDef {
  key: string;
  label: string;
  kind: "text" | "multiline" | "names" | "date";
  core: boolean;
}

/** = core types::TypeDef */
export interface TypeDef {
  id: string;
  label: string;
  desc: string;
  fields: FieldDef[];
}

/** = core annotate::AnnotationRow */
export interface AnnotationRow {
  key: string;
  author: string;
  kind: string;
  color: string | null;
  comment: string | null;
  page: number | null;
  quote: string | null;
  /** TextQuoteSelector の前後文脈（viewer は使わない） */
  prefix: string | null;
  suffix: string | null;
  dateAdded: string;
}

/** = core annotate::NoteRow */
export interface NoteRow {
  key: string;
  author: string;
  contentMd: string;
  dateAdded: string;
  dateModified: string;
}

/// 既存注釈の部分更新パッチ。フィールド省略（undefined）= 不変更。
/// comment / color は `null` を渡すと NULL へ消去、文字列（空文字含む）を渡すとその値に設定。
export interface AnnotationPatch {
  kind?: string;
  comment?: string | null;
  color?: string | null;
}

export const api = {
  itemTypes: () => invoke<TypeDef[]>("item_types"),
  subtypes: () => invoke<string[]>("subtypes"),
  createManual: (csl: any) => invoke<string>("create_manual", { csl }),
  updateItem: (key: string, csl: any) => invoke("update_item", { key, csl }),
  lookup: (input: string) => invoke<any>("lookup", { input }),
  attachmentData: (id: number) => invoke<ArrayBuffer>("attachment_data", { id }), // 生バイト（Rust は tauri::ipc::Response）
  listAnnotations: (key: string) => invoke<[AnnotationRow[], NoteRow[]]>("list_annotations", { key }),
  addAnnotation: (a: {
    key: string;
    quote: string;
    kind?: string | null;
    page?: number | null;
    comment?: string | null;
    color?: string | null;
  }) => invoke<string>("add_annotation", a),
  updateAnnotation: (key: string, patch: AnnotationPatch) => {
    const args: Record<string, unknown> = { key };
    if (patch.kind !== undefined) args.kind = patch.kind;
    if (patch.comment !== undefined) {
      args.comment = patch.comment;
      args.clearComment = patch.comment === null;
    }
    if (patch.color !== undefined) {
      args.color = patch.color;
      args.clearColor = patch.color === null;
    }
    return invoke<void>("update_annotation", args);
  },
  deleteAnnotation: (key: string) => invoke("delete_annotation", { key }),
  addNote: (key: string, contentMd: string) => invoke<string>("add_note", { key, contentMd }),
  deleteNote: (key: string) => invoke("delete_note", { key }),
  listNotes: (key: string) => invoke<NoteRow[]>("list_notes", { key }),
  updateNote: (key: string, contentMd: string) => invoke("update_note", { key, contentMd }),
  listItems: (filter: ListFilter) => invoke<ItemSummary[]>("list_items", { filter }),
  getItem: (key: string) => invoke<ItemDetail | null>("get_item", { key }),
  collectionsTree: () => invoke<CollectionRow[]>("collections_tree"),
  viewCounts: () => invoke<ViewCounts>("view_counts"),
  quickAdd: (input: string) => invoke<ItemSummary>("quick_add", { input }),
  importPdf: (path: string) => invoke<ImportSummary>("import_pdf", { path }),
  attachPdf: (key: string, path: string) => invoke("attach_pdf", { key, path }),
  fetchPdf: (key: string) => invoke<string>("fetch_pdf", { key }),
  setStar: (key: string, starred: boolean) => invoke("set_star", { key, starred }),
  trashItem: (key: string) => invoke("trash_item", { key }),
  restoreItem: (key: string) => invoke("restore_item", { key }),
  purgeItem: (key: string) => invoke<Purged>("purge_item", { key }),
  listTags: (byCount: boolean) => invoke<TagRow[]>("list_tags", { byCount }),
  createTag: (name: string, color: string | null) => invoke<number>("create_tag", { name, color }),
  updateTag: (id: number, name: string | null, color: string | null) =>
    invoke("update_tag", { id, name, color }),
  deleteTag: (id: number) => invoke("delete_tag", { id }),
  attachTag: (key: string, tagId: number) => invoke("attach_tag", { key, tagId }),
  detachTag: (key: string, tagId: number) => invoke("detach_tag", { key, tagId }),
  addToCollection: (key: string, collectionId: number) =>
    invoke("add_to_collection", { key, collectionId }),
  removeFromCollection: (key: string, collectionId: number) =>
    invoke("remove_from_collection", { key, collectionId }),
  membership: (keys: string[]) => invoke<Membership>("membership", { keys }),
  createCollection: (name: string) => invoke<number>("create_collection", { name }),
  openAttachment: (id: number) => invoke("open_attachment", { id }),
  openExternal: (url: string) => invoke("open_external", { url }),
  // ---- 引用スタイル（CSL） ----
  citeCatalog: (refresh = false) => invoke<StyleEntry[]>("cite_catalog", { refresh }),
  citeInstalled: () => invoke<StyleEntry[]>("cite_installed"),
  citeStyleXml: (id: string) => invoke<string>("cite_style_xml", { id }),
  citeLocaleXml: (lang: string) => invoke<string>("cite_locale_xml", { lang }),
  citePrefs: () => invoke<CitePrefs>("cite_prefs"),
  /** backend は渡さない項目を今のままにする（MCP の cite_set_prefs と同じ）が、フロントは常に全項目を渡す */
  citeSetPrefs: (style: string, locale: string, doiAlways: boolean) =>
    invoke<CitePrefs>("cite_set_prefs", { style, locale, doiAlways }),
  citeInstallFile: (path: string) => invoke<StyleEntry>("cite_install_file", { path }),
  // ---- ラベル（Manage labels） ----
  mergeTags: (sources: number[], target: number) => invoke<number>("merge_tags", { sources, target }),
  findReplaceTags: (find: string, replace: string, dryRun: boolean) =>
    invoke<RenameOutcome[]>("find_replace_tags", { find, replace, dryRun }),
  bibtexEntries: (keys: string[]) => invoke<BibtexEntry[]>("bibtex_entries", { keys }),
  // ---- フォルダ（New folder / Rename / Delete / Manage folders） ----
  /** parentId = null なら root。同じ親の下に大小無視の同名・`/` を含む名前は拒否（冪等ではない） */
  createCollectionIn: (name: string, parentId: number | null) =>
    invoke<number>("create_collection_in", { name, parentId }),
  renameCollection: (id: number, name: string) => invoke("rename_collection", { id, name }),
  /** サブフォルダごと即時に削除（論文は残る）。戻り値 = 消したフォルダ数 */
  deleteCollection: (id: number) => invoke<number>("delete_collection", { id }),
  findReplaceCollections: (find: string, replace: string, dryRun: boolean) =>
    invoke<RenameOutcome[]>("find_replace_collections", { find, replace, dryRun }),
  // ---- UI の表示設定（settings の ui_* だけ） ----
  getUiPref: (key: string) => invoke<string | null>("get_ui_pref", { key }),
  setUiPref: (key: string, value: string) => invoke("set_ui_pref", { key, value }),
  /** PRAGMA data_version（他のプロセス = MCP・CLI・connectd の書込で増える。app 自身の書込では変わらない） */
  dataVersion: () => invoke<number>("data_version"),
  /** 新版の確認（起動時に 1 回。取りに行くのは 1 日 1 度・Settings › 起動時に新版を確認 で止める）。動いている版より新しい版番号 or null */
  checkUpdate: () => invoke<string | null>("check_update"),
};

// ---- 型: core が serde_json::Value で返すもの（citestyle の JSON。MCP の cite_* と同じ形 = snake_case のまま） ----

/** スタイル索引 / 導入済みの 1 行（id = CSL リポジトリのファイル名） */
export interface StyleEntry {
  id: string;
  title: string;
  short?: string | null;
  /** numeric / author-date / note / author / label */
  format?: string | null;
  /** 索引: dependent（誌名の別名。整形は親スタイル） */
  dep?: boolean;
  fields?: string[];
  /** 導入済み: 親スタイルの id */
  parent?: string | null;
  builtin?: boolean;
  /** 手元の .csl から読み込んだもの */
  custom?: boolean;
}

/** = core citestyle::prefs の JSON（MCP の cite_prefs / cite_set_prefs がそのまま返す契約）。
    GUI 用に camelCase へ変換する層を置かないので、ここだけ snake_case */
export interface CitePrefs {
  style: string;
  /** "" = スタイルの既定言語 */
  locale: string;
  /** 文献リストに DOI（無ければ URL）を常に含める（Paperpile の Always include DOI and URL） */
  doi_always: boolean;
}

/** = core の bibtex::Entry（bibtex::citekey / item_to_bibtex と DOI を束ねたもの。MCP の bibtex_entries と同じ中身） */
export interface BibtexEntry {
  key: string;
  citekey: string;
  bibtex: string;
  doi: string | null;
}

/** = core tags::RenameOutcome（Find and replace の 1 件。status: renamed = 改名（dry run では予定）/ conflict = 同名が既にあり飛ばす /
    empty = 置換後が空 / invalid = 置換後が `/` を含む（フォルダだけ）） */
export interface RenameOutcome {
  id: number;
  old: string;
  new: string;
  status: "renamed" | "conflict" | "empty" | "invalid";
}
