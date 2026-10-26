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

# Paperpile JSON エクスポートからの一括インポート（冪等・再実行で既存とマージ）
openref import paperpile "/path/to/Paperpile-Export.json" [--dry-run]
```

DB の既定パス: `~/Library/Application Support/OpenRef/openref.sqlite`（`--db` または環境変数 `OPENREF_DB` で上書き可）。

---

## プライバシーと外部通信先

OpenRef はテレメトリ・アナリティクスを一切行いません。また、LLM の API キーをアプリ内に保持しません。バックグラウンドの通信は、アプリ起動時の新版確認（配布サイトの版番号ファイルを 24 時間に 1 回まで取得・識別子なし・Settings で無効化可）だけで、それ以外の外部への HTTP(S) 通信は以下の機能を実行した際に限定されます（詳細は [SECURITY.md](SECURITY.md)）:

- **DOI / PubMed 書誌解決**: `eutils.ncbi.nlm.nih.gov`, `api.crossref.org`, `doi.org`
- **OA PDF 自動取得**: `pmc-oa-opendata.s3.amazonaws.com`（PMC AWS Open Data）、および環境変数 `OPENREF_EMAIL` をユーザーが明示設定した場合のみ `api.unpaywall.org`（Unpaywall・opt-in）
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
# テスト一式
cargo test --workspace
node word/test-cite.cjs && node word/test-types.cjs && node word/test-styles.cjs && node word/test-commands.cjs
node app/src/lib/viewer/util.test.mjs && node app/src/lib/i18n.test.mjs
xmllint --noout --schema word/xsd/OfficeAppManifestV1_1.xsd word/manifest.xml

# リリースビルド
cargo build --release -p openref-cli -p openref-mcp -p openref-connectd
cd app && npm install && npm run tauri build -- --bundles app
```
