// OpenNote の画面側の共有型。骨組み（wiki.svelte.ts）と描画部品（MdView / HtmlView / MediaView）の受け渡しの契約。
// GUI の wiki_read_gui（core の read）/ wiki_list / wiki_backlinks / wiki_status / wiki_search / wiki_tags / wiki_inbox の戻り値（crates/openref-core/src/wiki/commands.rs）と同じ形。

export type WikiKind = "md" | "html" | "media" | "other";
export type WikiMediaKind = "image" | "video" | "audio" | "pdf";

/** 索引が解決したリンク 1 本（wiki_read_gui の links[]）。path は解決先の vault 相対パス（未解決は null） */
export interface WikiLink {
  kind: "wiki" | "embed" | "md" | "html" | "openref" | "file" | "external";
  target: string;
  path: string | null;
  ambiguous: boolean;
}

export interface WikiFrontmatterEntry {
  key: string;
  value: unknown;
  /** 部分集合パーサで解けなかったときの生の文字列 */
  raw?: string;
}

/** wiki_read_gui（core の read）の戻り値。md / html は本文とメタ、media / other は path と大きさだけ */
export interface WikiPageData {
  path: string;
  abs_path: string;
  kind: WikiKind;
  title: string;
  size: number;
  /** unix 秒 */
  mtime: number;
  media?: WikiMediaKind | null;
  tags?: string[];
  items?: string[];
  frontmatter?: WikiFrontmatterEntry[];
  links?: WikiLink[];
  sha256?: string;
  chars?: number;
  truncated?: boolean;
  content?: string;
  /** 参照フォルダのファイル = そのフォルダの絶対パス（wiki_read_ref）。path は絶対・読み取り専用・索引に無い（バックリンクも編集も無い） */
  external?: string;
}

export interface WikiListEntry {
  path: string;
  kind: WikiKind;
  title: string;
  tags: string[];
  items: string[];
  mtime: number;
  size: number;
}

/** wiki_search の 1 件。snippet は一致箇所を U+E000 … U+E001 で囲んだ抜粋（media・tag だけの検索は null） */
export interface WikiSearchHit {
  path: string;
  kind: WikiKind;
  title: string;
  tags: string[];
  mtime: number;
  size: number;
  match: "title" | "body" | "title+body";
  snippet: string | null;
}

/** wiki_backlinks の 1 件（ページ / item を参照するページ） */
export interface WikiBacklink {
  path: string;
  title: string;
  kind: string;
  link_kinds: string[];
  excerpt: string | null;
}

/** wiki_status の戻り値（必要な鍵だけ） */
export interface WikiStatus {
  enabled: boolean;
  root: string | null;
  error?: string;
  rebuilding?: { percent: number } | null;
  pages?: number;
  inbox?: number;
  last_scan?: number | null;
  /** このプロセスで壊れた索引を退避して作り直した（core の wiki_status） */
  recovered?: { at: number; moved_to: string } | null;
  /** 画面側: wiki_status の呼び出し自体が失敗した（有効 / 無効は wiki_settings で決め、これはヘッダーの警告に出すだけ） */
  statusError?: string;
}

/** wiki_tags の 1 件 */
export interface WikiTagCount {
  tag: string;
  count: number;
}

/** wiki_inbox の 1 件（ingest/ の中のファイル） */
export interface WikiInboxFile {
  path: string;
  name: string;
  kind: WikiKind;
  media: WikiMediaKind | null;
  size: number;
  mtime: number;
}

/** Tauri event "wiki-changed"（wiki_gui.rs の WikiChanged）の payload = 索引を更新したパス（vault 相対）を種別ごとに。
    種別は core の取り込み結果から（added = 索引に無かった / removed = 消えたので外した / modified = あった = 読み直した）。
    all = 全走査の後（[再索引]・起動時・監視がフォルダの増減で全走査に切り替えた）= 何が増減したか分からない = 全部が変わったものとして扱う（配列は空）。
    注意: core が同期で索引に入れた書込（GUI の保存・チャット / MCP の wiki_write・取込）は、監視が拾う時点で索引にある = 新しいページでも modified で届く。
    画面は「知らない（木に無い）modified」を増として扱う（reloadPlan の known）。
    画面: 木（wiki_list）は all / added / removed / 知らない modified のときだけ・Tags 箱は毎回（ログの `#tag` も索引に入る）・Ingest / Chat log 箱はそのフォルダに変更があるときだけ */
export interface WikiChanged {
  all: boolean;
  added: string[];
  removed: string[];
  modified: string[];
}

/** 見出し（目次・`[[name#heading]]` の着地）。id は描画側が shadow root 内で付けたもの */
export interface WikiHeading {
  level: number;
  text: string;
  id: string;
}

/** 描画部品が骨組みへ返す遷移の要求。解決と実行は骨組みが行う */
export type WikiNav =
  | { kind: "page"; path: string; heading?: string }   // vault 内のページ・media（新規タブは newTab）
  | { kind: "item"; key: string; page?: number; att?: string } // openref://open?key=…
  | { kind: "external"; url: string }                    // http(s) → wiki_open_url（設定で別窓 / 外部ブラウザ）・mailto → open_external
  | { kind: "open"; abs: string }                        // ホストの open に渡すファイル（vault 外の file:// を含む・安全の判定は wiki_open_path）
  | { kind: "ref"; abs: string };                        // 参照フォルダの中の pdf / html / 画像 / 動画 / 音声 = 内蔵の表示で開く（wiki_read_ref）

/** MdView / HtmlView / MediaView の共通 props */
export interface WikiViewProps {
  page: WikiPageData;
  /** vault の root（絶対パス）。asset URL の組立てに使う。参照フォルダのページでは、そのフォルダ（path はその相対） */
  root: string;
  /** 参照フォルダ（絶対パス）。その下への file:// は内蔵の表示で開く（無ければ全部ホストの open） */
  refs?: string[];
  /** 遷移の要求。newTab = ⌘クリック */
  onNavigate: (nav: WikiNav, newTab?: boolean) => void;
  /** `![[ページ]]` の 1 段展開用（wiki_read_gui を呼ぶ） */
  readPage: (path: string) => Promise<WikiPageData>;
  /** 描画後の見出し一覧（目次用） */
  onHeadings?: (h: WikiHeading[]) => void;
  /** 目次クリック等でこの見出しへスクロール（nonce が変わるたびに実行） */
  scrollTo?: { id: string; nonce: number } | null;
  /** ⌘F（表示モードのページ内検索バー）を開く要求（nonce が変わるたびに開く） */
  findRequest?: number;
  /** md の見出しを h1〜h6 で色分けする（html ページには効かせない = 作者の CSS を尊重） */
  headingColors?: boolean;
}

// ---- 編集の契約: core と Tauri の包み と エディタ画面 の受け渡し ----
// すべて GUI から invoke(name, { args: {...} })。path は vault 相対。エラーは文字列(画面に出す)。

/** wiki_save {path, content, if_sha256?}(GUI のエディタの保存。MCP の wiki_write とは別)。if_sha256 が今のファイルと違えば書かずに conflict を返す(黙って負けない・勝たない) */
export type WikiWriteResult =
  | { ok: true; path: string; sha256: string; size: number; mtime: number; created: boolean; backup?: string; index_error?: string } // backup = 退避先(照合なし・force の上書きのとき)
  | { ok: false; conflict: true; path: string; sha256: string | null }; // sha256 = 今のファイルの hash(消えていれば null)

/** wiki_save に force: true を足すと照合を飛ばし、上書き前の内容を wiki-history に退避してから書く(競合ダイアログの「上書き」) */

/** wiki_create {dir, name?} → 新規ページ(既定 "Untitled.md"・同名があれば "Untitled 2.md" …)。md は空・html は最小の骨格 */
export interface WikiCreateResult { path: string }

/** wiki_mkdir {dir, name} → 空のフォルダ(同じ名前のフォルダ / ファイルがあれば拒否)。索引には入らない = 木は GUI が覚えて出す */
export interface WikiMkdirResult { path: string }

/** wiki_rename_preview {from, to} → 書き換え対象([[旧名]] / ![[旧名]] / 相対リンクを持つページ)と件数 */
export interface WikiRenamePreview { from: string; to: string; links: number; files: string[]; other?: WikiRenameOther[] }

/** 書き換えない参照(html のページ・file:// の絶対リンク・読めないページ・frontmatter の引用を壊す名前・チャットのログ = kind "chat-log") */
export interface WikiRenameOther { path: string; kind: string }

/** wiki_rename {from, to, rewrite: boolean} → 改名(移動先が既にあれば拒否)。rewrite なら各ファイルを退避してからリンクを書き換える */
export interface WikiRenameResult {
  path: string;
  rewritten: string[];
  from?: string;
  /** 書き換えたリンクの本数 */
  links?: number;
  /** 計画の後に書き換わっていた等で触らなかったファイル */
  skipped?: { path: string; reason: string }[];
  other?: WikiRenameOther[];
  index_error?: string;
}

/** wiki_trash {path, dir?} → macOS のゴミ箱へ(Finder の「戻す」が効く方式)。dir = 期待する形(true = フォルダを中身ごと・false = ファイル・違えば移さない)。
    結果の dir / files(. 始まりと symlink を除く中のファイル数)はフォルダのとき */
export interface WikiTrashResult { path: string; method?: string; dir?: boolean; files?: number; index_error?: string }

/** wiki_save_blob {name, data_base64, page?} → <vault>/attachments/<YYYY-MM>/<name>(同名は連番)。挿入する文字列 embed = "![[…]]" */
export interface WikiSaveBlobResult { path: string; embed: string }

/** wiki_import_file {abs, page?} → OS ドロップの PDF / 動画 / 画像を vault へコピー(attachments/<YYYY-MM>/)。vault 内のファイルならコピーせずそのまま */
export interface WikiImportResult { path: string; embed: string; copied: boolean }

// ---- チャットの契約: Rust（子プロセスと GUI 専用 command）と右ペインのチャット画面の受け渡し ----
// すべて GUI から invoke(name, { args: {...} })。エラーは文字列(画面に出す)。
// 子プロセス = Claude Code CLI(stream-json)。OpenRef は LLM を呼ばず、会話本文も持たない(本文は ~/.claude/projects/)。

/** wiki_chat_status {force?} → 使えるか。enabled = 設定 wiki_chat_enabled(Wiki が有効なときだけ true になりうる)。
 *  **無効なら何も調べずに返す**(claude の検出・`claude auth status` を起動しない = 原則 1)。cli / version / logged_in / mcp は null。
 *  `claude auth status` は 30 秒覚える(右ペインの mount・exit ごとに起こさない)。force: true(画面の [再確認])・login / logout・有効 / 無効の切替で捨てる */
export interface WikiChatStatus {
  enabled: boolean;
  /** 見つかった claude の実行ファイル(無ければ null) */
  cli: string | null;
  version: string | null;
  /** `claude auth status` の判定(調べられなければ null) */
  logged_in: boolean | null;
  /** `claude auth status` の中身(読めなければ null) */
  auth: WikiChatAuth | null;
  /** 設定 wiki_chat_model(--model に渡す別名か id。null = Claude Code の既定) */
  model: string | null;
  /** 子プロセスに渡す openref-mcp(wiki_write を持たない古い版・無い = null) */
  mcp: string | null;
  /** 使えない理由(案内の文言の元・画面がそのまま出す) */
  reason?: string;
  /** 今の子プロセスの状態 */
  state: WikiChatState;
  /** 今の会話(vault の root と組で wiki-chat.json に保存されているもの) */
  session_id: string | null;
}

export type WikiChatState = "off" | "starting" | "idle" | "busy" | "stopping";

/** `claude auth status` の JSON から(method = authMethod・subscription = subscriptionType) */
export interface WikiChatAuth { logged_in: boolean; method: string | null; email: string | null; subscription: string | null }

/** wiki_chat_set_model {model}(空 = 既定)→ WikiChatStatus。設定を書き、子プロセスを止める(次の送信で --resume + 新しい --model) */
/** wiki_chat_auth {action: "login" | "logout"} → WikiChatStatus。logout = 子を止めて `claude auth logout`・login = Terminal.app に `claude auth login` を開く
 *  (対話 = ブラウザ + ターミナル。終わったら画面の [再確認]) */

/** wiki_chat_set_enabled {enabled} → WikiChatStatus(OFF にすると子プロセスを止める) */
/** Settings › OpenNote の [チャット](ネイティブのメニュー = Rust): ON を選ぶと Rust は設定を変えずに Tauri event "wiki-chat-consent"(payload なし)を出し、
 *  画面の確認ダイアログ(WikiChatConsent の説明)が [有効にする] で wiki_chat_set_enabled {enabled: true} を呼ぶ。OFF は Rust がそのまま無効にする。
 *  有効 / 無効が変わったら(メニュー・command・Wiki の ON/OFF・vault の切替のどれでも・子の有無によらず)Rust は wiki-chat {type: "enabled"} を出す
 *  (画面は wiki_chat_status を読み直す)。{type: "state"} はターンの busy / idle 等の子の状態だけ(画面は status を読み直さない = claude --version 等を毎ターン起こさない) */

/** wiki_chat_start {new?: boolean} → 子プロセスを起こす。new なら新しい会話(--session-id)、でなければ保存済みの会話を --resume。
 *  resume に失敗したら新しい会話にして notice で知らせる */
export interface WikiChatStartResult { session_id: string; resumed: boolean; notice?: string }

/** 発言に添える文脈(user が外せる)。path = 今開いているページ(vault 相対・参照フォルダなら絶対)・selection = 選択範囲の本文・
 *  item / title = PDF のタブ(内蔵の PDF viewer のチャット)で開いている文献の key と題名(path と排他 = 前面のタブで決まる) */
export interface WikiChatContext { path?: string; selection?: string; item?: string; title?: string }

/** wiki_chat_start は返る前に session_id を wiki-chat.json に保存し(以後の wiki_chat_status は必ずそれを返す)、state(idle か、失敗なら off)を出す。画面も返った後に wiki_chat_status を読み直す */

/** wiki_chat_send {text, context?}(文脈が無ければ context を付けない)→ { turn }。turn = この会話で 1 から数えるターン番号(changes・undo の鍵)。**アプリの起動中は同じ会話の中で増え続ける**(子プロセスを --resume で起こし直しても 1 に戻さない) */
export interface WikiChatSendResult { turn: number }

/** wiki_chat_interrupt {} = ターンだけ止める(会話は続く)/ wiki_chat_stop {} = 子プロセスを終える(stdin を閉じる → 猶予 → SIGTERM) */

/** Claude がこのターンで変えたもの。Rust が stream-json の tool_use / tool_result(mcp__openref__call の wiki_write / wiki_move / import_pdf / quick_add)から組み立てる。
    同じターンに同じファイルを複数回書いたら 1 件に畳む(write は最初の backup・sha256_after は最後) */
export type WikiChatChange =
  | { kind: "write"; path: string; sha256_after: string; backup: string }      // 既存の上書き(戻す = backup の内容へ)
  | { kind: "create"; path: string; sha256_after: string }                     // 新規作成(戻す = ゴミ箱へ)
  | { kind: "move"; from: string; to: string }                                 // 移動(戻す = 逆の move)
  | { kind: "item"; key: string; title?: string };                             // 蔵書への登録(戻す = trash_item)

/** wiki_chat_undo {turn, index} → 1 件を戻す。今のファイルの sha256 が sha256_after と違えば(user がその後に編集した)戻さず reason を返す */
export type WikiChatUndoResult =
  | { ok: true; turn: number; index: number; path?: string; key?: string }
  | { ok: false; turn: number; index: number; reason: string };

/** Tauri event "wiki-chat" の payload。画面は line を寛容に解釈する(JSON でない・未知の type は無視) */
export type WikiChatEvent =
  | { type: "line"; session_id: string; turn: number; line: string }     // stdout の 1 行(stream-json)。巨大な行は Rust が切り捨てて stderr で知らせる
  | { type: "stderr"; text: string }                                     // 起動失敗等を画面に出す
  | { type: "state"; state: WikiChatState; session_id: string | null }
  | { type: "enabled"; enabled: boolean }                                 // 有効 / 無効の変化(画面は wiki_chat_status を読み直す)
  | { type: "changes"; session_id: string; turn: number; changes: WikiChatChange[] } // ターン中に増えるたびに全体を送り直す
  | { type: "exit"; code: number | null; signal: string | null }; // signal = "SIGTERM" 等の名前
