# OpenRef

**AI エージェント（Claude）統合型・macOS ローカル文献管理システム**  

- **License**: [MIT License](LICENSE)（第三者ソフトウェア表示: [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md)）
- **Trust & Privacy**: **Free forever** / **Open source (MIT)** / **No telemetry** / **アカウント登録不要**（詳細: [SECURITY.md](SECURITY.md)）
- **対応環境**: macOS（Apple Silicon `arm64`・macOS 14 以降推奨）

---

## 特徴

1. **すべて端末ローカルで完結**: 書誌（CSL-JSON 正準）・PDF・ハイライト注釈・ノート・全文検索索引（和文 CJK 対応 FTS5）を `~/Library/Application Support/OpenRef/` に保持。クラウドアカウント不要・テレメトリ送信なし。
2. **PubMed / Crossref 機械確定**: PMID・DOI・URL や PDF ドロップ、Chrome 拡張からの 1 クリック保存で権威レジストリから正確な書誌を確定。撤回論文（Retraction）も自動検出。
3. **タブ型 PDF ビューア & 注釈**: アプリ内で複数 PDF をタブ表示し、ハイライト・下線・コメント注釈を付与。
4. **Word 引用プラグイン同梱**: Microsoft Word リボンの **OpenRef** タブから引用挿入・文献リスト自動整形（Vancouver / AMA / Radiology 等、CSL 約 1 万誌対応）。
5. **Claude から直接操作（MCP 統合）**: `openref-mcp` により Claude Code / Claude Desktop から蔵書検索・PDF 本文読取・注釈付与・ラベル整理を一気通貫で実行（書込前の自動バックアップ付き）。

---

## インストール（macOS Apple Silicon）

ターミナルで以下の 1 行を実行すると、`OpenRef.app`・常駐取込サービス (`openref-connectd`)・CLI (`openref`)・MCP サーバ (`openref-mcp`)・Chrome 拡張・Word アドインが一括で設置されます（更新も同じコマンドを再実行するだけです）:

```bash
curl -fsSL https://openref.unitedradiology.jp/install.sh | bash
```

アンインストール（登録した論文・PDF データは消さずに残します）:

```bash
curl -fsSL https://openref.unitedradiology.jp/uninstall.sh | bash
```

### 初回セットアップ（拡張機能・Word・AI 連携）

1. **Chrome 拡張（OpenRef Capture）の有効化**
   - Chrome で `chrome://extensions` を開き、右上の「デベロッパーモード」を ON
   - 「パッケージ化されていない拡張機能を読み込む」から `~/Applications/OpenRef Capture` を選択
   - （Zotero Connector 互換 API も `:23119` で待機しているため、既存の Zotero Connector からの保存も可能です）
2. **Microsoft Word で引用を使う**
   - インストーラが Word の Startup フォルダにテンプレート `OpenRef.dotm` を置きます（ローカル証明書の信頼登録で macOS のパスワード確認が 1 回出ることがあります）
   - Word を終了して開き直すと、リボンに **OpenRef** タブが表示されます
3. **Claude Code / Claude Desktop との MCP 連携**
   - **Claude Code**:
     ```bash
     claude mcp add -s user openref "$HOME/Library/Application Support/OpenRef/bin/openref-mcp"
     ```
   - **Claude Desktop**: Claude Desktop を完全終了（⌘Q）した状態で `~/Library/Application Support/Claude/claude_desktop_config.json` の `mcpServers` に以下を追加して起動:
     ```json
     {
       "mcpServers": {
         "openref": {
           "command": "/Users/<あなたのユーザー名>/Library/Application Support/OpenRef/bin/openref-mcp",
           "args": []
         }
       }
     }
     ```

---

## MCP サーバ (`openref-mcp`)

`openref-mcp` は 3 つのツール（`schema` / `sql` / `call`）ですべての蔵書操作を提供します（使い方の注記とコマンド一覧は `schema` ツールの応答に含まれます）。

| Tool | 役割 |
|---|---|
| `schema` | DB スキーマ（DDL）・利用規約・全コマンド一覧・DB 統計を返す |
| `sql` | 読み取り専用 SQL（`PRAGMA query_only` + SQLite Authorizer により `SELECT` 系のみ許可・10 秒上限） |
| `call` | GUI と同一の書込・読取コマンド群を実行（初回書込前に `backup/pre-agent-*.sqlite` へ自動スナップショットを作成） |

---

## CLI (`openref`) の使い方

インストーラ導入時は `~/Library/Application Support/OpenRef/bin/openref` に配置されます（必要に応じて PATH に追加してください）。

```bash
openref add https://pubmed.ncbi.nlm.nih.gov/36804602/   # PubMed URL
openref add 36804602                                     # PMID
openref add 10.1148/rg.230216                            # DOI
openref list
openref search pancreatic cancer
openref search 胆道                                      # 和文 OK（CJK per-char phrase）
openref show <KEY>                                       # --json で CSL-JSON
openref collections                                      # フォルダツリー
openref fetch-pdf --missing                              # OA 論文の PDF を自動取得（PMC AWS Open Data 等）
openref info

# Paperpile JSON エクスポートからの一括インポート（同じ入力から再実行可能）
openref import paperpile "/path/to/Paperpile-Export.json" [--dry-run]
```

DB の既定パス: `~/Library/Application Support/OpenRef/openref.sqlite`（`--db` または環境変数 `OPENREF_DB` で上書き可）。

## ライブラリ移行（Paperpile / Zotero）

配布版 1.1.0 以降で利用できます。

「＋ 新規 ▾ → 他の文献管理ソフトから移行…」で Paperpile または Zotero を選び、書誌ファイルと添付フォルダを指定します。
事前確認で新規・既存への追加・保留・PDF 不足・対象外・コピー容量を確認してから開始します。
移行中も画面を閉じてライブラリを閲覧でき、中止後は同じ入力で再確認すると保存済みの文献を再利用します。
過去の結果と JSON 保存、全文索引の再試行も同じ画面から行えます。

- **Paperpile**: ライブラリ全体の JSON を別途書き出し、PDF は「Download files as ZIP」で注釈を含めて取得します。ZIP を Finder で展開し、JSON と展開フォルダを選択します。JSON だけでも書誌を取り込めます。[公式手順](https://paperpile.com/h/bulk-download-PDFs/)
- **Zotero**: 「Export Library…」で Zotero RDF を選び、ノート・ファイル・PDF 注釈を含めます。RDF を選ぶと、その隣の files フォルダも参照します。元の登録・更新日時は引き継げません。[公式手順](https://www.zotero.org/support/kb/exporting)
- PDF は **OpenRef の Mac ローカル保存領域へコピー**が GUI の既定です。Drive の保存先設定には追従しません。リンクを選ぶ場合は、ZIP の展開フォルダを含め元ファイルの場所を保持してください。
- 同形式の移行元が1つなら GUI・CLI とも既定で再利用し、複数なら選択が必要です。同じライブラリの補完では「以前の移行に追加」を選びます。別ライブラリを同じ移行元にしないでください。新しい移行元を明示すると別の由来として扱うため、既存対象との対応を警告で確認してください。旧 Paperpile 取込の ID 照合には、同じライブラリであることの明示が必要です。
- 同じ移行元の別エクスポートに過去の取込との対応候補があれば、まず文献を保留します。IDだけで対応を確定しません。候補から同じ文献を選び、再確認すると書誌の不足分とPDFを追加できます。この場合、過去のノート・タグ・所属との対応は自動確定せず保留します。
- 既存の空でない編集値と、過去に適用後に削除・空にした書誌フィールドは保持し、未適用の不足情報だけを追加します。識別子の矛盾、同じ移行元の由来で確認できる削除済み文献、題名だけの候補、曖昧な添付は保留として報告します。確認候補の既存文献と曖昧なPDFは選択後に「選択内容で再確認」で確定します。入力内だけの重複候補は「新規として登録」（CLI decisions の item_key は `@new`）を選べます。適用時に識別子を再照合し、同一文献なら合流、矛盾なら保留します。未選択、削除済み対象、識別子の矛盾は保留します。
- 複数ノートを個別編集でき、PDF 内のハイライトやコメントは読み取り専用で閲覧できます。元 PDF のバイト列は変更しません。独自注釈への変換、既存 Word 文書の引用変換、双方向同期は含みません。

検証済み: Paperpile の小標本で文献・PDF・名前付きタグの適用と再実行。Zotero は公式公開 RDF とテスト用 graph でコレクション・著者順・ノート・全 PDF を検証。
Paperpile の現行 `folders` / `notes` は形式未確定のため、原情報と警告を保存するだけで階層・ノートへ変換しません。旧 `foldersNamed` と名前付きタグは対応済みです。現行 Zotero アプリの実書出し、両ソフト由来の注釈付き PDF は引き続き検証対象です。
画像・PDF 以外の添付、未対応フィールドは元情報をローカルに保存し、未対応の理由を結果に表示します。注意事項や不足があれば完全移行とは表示しません。

CLI は従来の Paperpile 引数を含め、事前退避・照合・履歴を備える共通処理を使います。Paperpile の既定はリンクです。PDF の場所は `--files` または `--mount` で明示してください。Drive の自動探索は行わず、指定がなければ書誌だけを取り込みます。

```sh
openref import paperpile /path/library.json --files /path/extracted --attachments copy --dry-run
openref import paperpile /path/library.json --files /path/extracted --attachments copy
openref import zotero /path/export.rdf --dry-run
openref import zotero /path/export.rdf
openref import history
openref import report RUN_ID
# 同じ入力から再開。別の書出しを追加する場合は --source-id SOURCE_ID
openref import paperpile /path/library.json --files /path/extracted --resume RUN_ID --report-json
```


既存文献の選択には `candidate_details` の `csl_sha256` も保存します。候補の書誌が変わった場合は選び直してください。`@new` にはこの値は不要です。

CLI の候補確定は `--decisions /path/choices.json` を使います。まず `--dry-run --report-json` の `source_id` / `input_sha256`、文献の `object_id` / `candidate_details`、添付の `id` / `file_options` を確認して、次の形のJSONを用意します。値は実際のプレビューに置き換え、同じ `--source-id` と添付ルートで再確認してから適用してください。

```json
{
  "source_id": "SOURCE_ID",
  "input_sha256": "INPUT_SHA256",
  "items": [{"object_id": "INPUT_OBJECT_ID", "item_key": "EXISTING_KEY", "csl_sha256": "CANDIDATE_CSL_SHA256"}],
  "files": [{"object_id": "INPUT_OBJECT_ID", "attachment_id": "ATTACHMENT_ID", "path": "/exact/candidate.pdf"}]
}
```

指定しない文献・添付は通常の判定を維持します。候補外のパスや別入力の選択は拒否します。既存文献の編集・削除、識別子の競合、PDFの変更は適用前にも検査し、選択で安全確認を解除しません。元ファイルへのリンクも同じ選択処理を使います。

---

## プライバシーと外部通信先

OpenRef はテレメトリ・アナリティクスを一切行いません。また、LLM の API キーをアプリ内に保持しません。バックグラウンドの通信は、アプリ起動時の新版確認（配布サイトの版番号ファイルを 24 時間に 1 回まで取得・識別子なし・Settings で無効化可）だけで、それ以外の外部への HTTP(S) 通信は以下の機能を実行した際に限定されます（詳細は [SECURITY.md](SECURITY.md)）:

- **DOI / PubMed 書誌解決**: `eutils.ncbi.nlm.nih.gov`, `api.crossref.org`, `doi.org`（医書.jp の DOI がまだ doi.org で解決できないときだけ `webview.isho.jp`）
- **OA PDF 自動取得**: `pmc-oa-opendata.s3.amazonaws.com`（PMC AWS Open Data）、出版社のサイト（DOI の着地ページ・Crossref が示す PDF の URL・ページが宣言する `citation_pdf_url` へ通常の GET。Cookie なし・プライベート IP は拒否）、および環境変数 `OPENREF_EMAIL` をユーザーが明示設定した場合のみ `api.unpaywall.org`（Unpaywall・opt-in）
- **引用スタイル追加**: GitHub `citation-style-language` リポジトリ（同梱 4 スタイル以外を追加選択したときのみ取得しローカルキャッシュ）
- **新版の確認**: `openref.unitedradiology.jp/dl/VERSION`（起動時・前回から 24 時間以上経っていれば 1 回。版番号 1 行を読むだけで、User-Agent に動いている版が載る以外は何も送りません。新版があれば画面上部にお知らせ。自動では入れ替えません。**Settings › 起動時に新版を確認** のチェックを外すと取りに行きません）

---

## 既知の制約（Known Limitations）

- **Quick Add の URL 入力**: 出版社ページの URL 直貼りは非対応です。PubMed URL / `doi.org` URL / PMID / DOI を入力するか、ブラウザ上で **OpenRef Capture** 拡張を使って保存してください。
- **PDF 本文抽出のフォールバック**: 内蔵の pure-Rust 抽出器で特殊フォントが文字化けする場合、環境に Poppler (`pdftotext`) があれば自動でフォールバックします（任意で `brew install poppler` を導入すると `openref fulltext index` で再抽出されます）。
- **開いている PDF ビューアへの注釈反映**: 開いている PDF タブに対して外部（Claude / MCP）から新規注釈を付けた場合、タブを開き直すとハイライトが重畳表示されます（一覧やノートは 2 秒ポーリングで自動反映されます）。
- **外部からのフォルダ・ラベル削除時のタブ残存**: MCP や CLI からフォルダ・ラベルを削除した場合、そのフォルダ/ラベルを開いていた一覧タブは次回起動時まで残ります（中身は空として表示されます）。
- **終了直前（⌘Q の 300ms 以内）のタブ操作**: アプリ終了直前の 300ms 以内に行ったタブ開閉はデバウンスにより次回起動時のタブ復元に含まれない場合があります。
- **Chrome 拡張・Word アドインの導入方式**: Chrome 拡張は `chrome://extensions` からのパッケージ化されていない拡張機能の読み込み（load unpacked）、Word 連携は Word の Startup フォルダに置くテンプレート（`OpenRef.dotm`）方式です。
- **Unpaywall 経路**: デフォルトでは無効（opt-in）です。利用する場合はユーザー自身のメールアドレスを環境変数 `OPENREF_EMAIL` に設定してください。
- **`openref sync`（Google Drive 等との差分スキャン）**: macOS の「システム設定 > プライバシーとセキュリティ > ファイルとフォルダ」でターミナルへのアクセス許可が必要です。

---

## ソースからのビルド（開発者向け）

必要環境: Rust (`rustup`)・Node.js (22.18+)

```bash
# テスト一式（cargo・svelte-check・node / python のテスト。先に cd app && npm ci）
bash scripts/test-all.sh

# リリースビルド
cargo build --release -p openref-cli -p openref-mcp -p openref-connectd
cd app && npm install && npm run tauri build -- --bundles app
```
