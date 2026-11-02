# Security Policy & Architecture (`SECURITY.md`)

OpenRef は「文献データをユーザーの Mac ローカルに保持し、AI エージェント（Claude 等）と安全に協調する」ことを前提に設計されています。本書では OpenRef のセキュリティ境界、通信先、インストーラの挙動、および MCP 統合時の保護機構を明記します。

---

## 1. ローカル完結と認証情報の非保持

- **データベースの所在**: すべての書誌・注釈・ノート・全文索引はユーザーの Mac ローカル（`~/Library/Application Support/OpenRef/openref.sqlite`）に保存されます。クラウド同期や外部 DB への送信はありません。
- **API キー・トークンの非保持**: OpenRef 自身は LLM の API キーやクラウドアカウントの認証情報を一切保持しません。AI 統合は Claude Code や Claude Desktop 等のホスト側がローカル MCP（stdio）経由で `openref-mcp` を呼び出す構成です。
- **テレメトリゼロ**: 利用統計の収集、クラッシュレポートの自動送信は一切行いません。
- **新版の確認（opt-out）**: アプリの起動時、前回の確認から 24 時間以上経っていれば 1 回だけ、配布サイトの版番号ファイル（`https://openref.unitedradiology.jp/dl/VERSION`・内容は版番号 1 行）を取得し、動いている版と比べます（取れなかったときも次は 24 時間後 = 起動のたびには取りに行きません。初回起動の 1 回は、設定を変える前に走ります）。送るのは通常の HTTP GET 1 回で、識別子・利用情報は含みません（User-Agent に動いている版 `openref/X.Y.Z` が載ります）。取得した版番号・確認日時・失敗の理由は端末内の設定（`settings` テーブルの `update_*`）に保存するだけです。新しい版があっても**自動では入れ替えず**、画面上部にお知らせを出すだけです（更新はインストールコマンドの再実行）。**Settings › 「起動時に新版を確認」のチェックを外すと、以後は一切取りに行きません**（出ているお知らせも消えます）。

---

## 2. ローカル待受デーモン (`openref-connectd`) の境界

ブラウザ拡張（OpenRef Capture / Zotero Connector）および OpenRef for Word（Word の Startup テンプレート `OpenRef.dotm` と `OpenRef Dialog.app`）との連携用デーモン `openref-connectd` は、**ループバックアドレス（`127.0.0.1`）のみ**にバインドします。

| ポート | プロトコル | 用途 | バインド先 |
|---|---|---|---|
| `23119` | HTTP | Chrome 拡張（OpenRef Capture / Zotero Connector 互換）からの書誌・PDF 受領 | `127.0.0.1:23119` |
| `23120` | HTTPS | OpenRef for Word（`OpenRef.dotm` / `OpenRef Dialog.app`）の引用整形・参考文献一覧・検索ダイアログの API（`/word/v2/*`） | `127.0.0.1:23120` |

- LAN や外部ネットワークからは到達不能です。
- Word 連携用の HTTPS 通信には、初回起動時にローカル（`~/Library/Application Support/OpenRef/certs/`）で生成した `localhost` 専用の自己署名証明書を使用します（秘密鍵は端末外に出ません）。
- PDF 自動取得（`fetch_pdf`）が DOI 着地ページ等を辿る際は、ループバック・プライベート IP（`127.0.0.0/8`, `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `169.254.0.0/16`, `100.64.0.0/10`, `localhost`, 数値表記 IP 等）へのリクエストを各リダイレクトホップで拒否する SSRF ガードを備えています。

---

## 3. 外部通信先の一覧

OpenRef がインターネットへ HTTP(S) リクエストを送るのは、ユーザーが明示的に書誌確定・PDF 取得・引用スタイル追加を行ったときの以下の公開 API と、起動時の新版確認（表の最終行・§1 のとおり opt-out 可）のみです。

| 通信先 | タイミング | 送信される情報 |
|---|---|---|
| **NCBI PubMed E-utilities** (`eutils.ncbi.nlm.nih.gov`) | PMID / DOI からの書誌確定・撤回情報確認時 | PMID または DOI |
| **Crossref / doi.org** (`api.crossref.org`, `doi.org`) | DOI からの書誌確定・PDF 所在確認時 | DOI またはタイトル断片（PDF 取込時） |
| **医書.jp** (`webview.isho.jp`) | 医書.jp の DOI（`10.18885/CI.` で始まるもの）がまだ doi.org で解決できないときの書誌確定 | DOI |
| **PMC AWS Open Data** (`pmc-oa-opendata.s3.amazonaws.com`) | OA 論文の PDF 自動取得時 | PMCID |
| **出版社のサイト**（DOI の着地ページ・Crossref が示す PDF の URL・ページが宣言する `citation_pdf_url` のホスト） | PDF 自動取得時 | 通常の GET のみ（URL に含まれる DOI 等。Cookie・識別子は送りません。各ホップで §2 の SSRF ガード） |
| **Unpaywall API** (`api.unpaywall.org`) | 環境変数 `OPENREF_EMAIL` をユーザーが設定している場合のみ（opt-in） | DOI + `OPENREF_EMAIL` |
| **CSL Styles Repository** (GitHub `citation-style-language`) | 同梱 4 種以外の引用スタイル・言語ファイルを新規導入したとき | スタイル ID / ロケール名（取得後はローカルにキャッシュ） |
| **配布サイト** (`openref.unitedradiology.jp`) | アプリ起動時・前回から 24 時間以上経っていたときだけ（Settings › 起動時に新版を確認 で無効化可） | 動いている版番号のみ（`dl/VERSION` の GET 1 回。User-Agent `openref/X.Y.Z`） |

OpenRef / OpenNote のチャット（既定 OFF・§6 の 4）を有効にしたときの Anthropic への通信は、OpenRef ではなく利用者の Claude Code が行います。

---

## 4. インストーラ (`install.sh`) が行うこと・行わないこと

配布スクリプト（`https://openref.unitedradiology.jp/install.sh`）は平文のシェルスクリプトであり、実行前に内容を確認できます:

```bash
curl -fsSL https://openref.unitedradiology.jp/install.sh | less
```

### `install.sh` が行うこと（全リスト）
1. アーキテクチャ（Apple Silicon `arm64`）と macOS バージョンの確認
2. 配布アーカイブ（`openref-macos-arm64.tar.gz`）のダウンロードと一時展開
3. （旧名 `Refract` が存在する場合のみ）旧常駐サービスの停止と `~/Library/Application Support/Refract` から `…/OpenRef` への引き継ぎ（変更前に `backup/pre-rename-*.sqlite` へ自動バックアップ）
4. `OpenRef.app` を `/Applications`（書込権限がなければ `~/Applications`）へコピー
5. `openref`（CLI）・`openref-mcp`（MCP サーバ）・`openref-connectd`（取込サービス）を `~/Library/Application Support/OpenRef/bin/` へ設置
6. `~/Library/LaunchAgents/dev.openref.connectd.plist` を作成し、`launchctl bootstrap` で `openref-connectd` を常駐開始
7. Chrome 拡張フォルダを `~/Applications/OpenRef Capture/` へ展開
8. Microsoft Word が導入されている場合、Word の Startup フォルダ（`~/Library/Group Containers/UBF8T346G9.Office/User Content/Startup/Word/`。環境により `User Content.localized/Startup.localized/Word/`）に `OpenRef.dotm` を、`~/Library/Application Support/OpenRef/word/` に `OpenRef Dialog.app` を設置し、旧方式の Office アドイン manifest（`~/Library/Containers/com.microsoft.Word/Data/Documents/wef/openref-manifest.xml`）があれば削除したうえで、`security add-trusted-cert` でローカル自己署名証明書（`openref-localhost.pem`）をログインキーチェーンに信頼登録（このとき macOS のパスワード確認ダイアログが 1 回表示されます）
9. Claude Desktop が導入されている場合（`~/Library/Application Support/Claude/` が存在）、その設定ファイル `claude_desktop_config.json` の `mcpServers` に `openref`（`~/Library/Application Support/OpenRef/bin/openref-mcp` を起動する 1 エントリ）を追加（既存の `openref` エントリがあれば `env` 等の他のキーは保って `command` だけを向け直す。`args` が配列でなければ `[]` にする）。他の登録・他の設定・キーの順序は保持する（ファイルは JSON として読み直して整形し直す）。書き換える前に同じフォルダへ `claude_desktop_config.json.bak-openref` として退避し（退避できなければ書かない）、所有者のみ読み書きできる権限（0600）で書く。設定ファイルが壊れた JSON・UTF-8 でない・NUL を含む・シンボリックリンク・読み書きできないときは触らずに理由を表示。Claude Desktop が起動中のときは書かずに案内だけ表示（Desktop は終了時に設定ファイルをメモリの内容で書き戻すため、起動中に足した項目は消える）。`uninstall.sh` は同じ条件で `openref` のエントリだけを外す。Claude Code への登録（`claude mcp add`）は行わない（README の 1 行を手動で実行）

### `install.sh` が行わないこと
- システム領域（`/usr`, `/Library` 等）への書き込みや `sudo` によるルート権限昇格（キーチェーンへの証明書登録時のみ macOS 標準の確認が出ます）
- シェル設定ファイル（`~/.zshrc` 等）の無断書き換え
- アンインストール時（`uninstall.sh`）のライブラリデータ（`~/Library/Application Support/OpenRef/` 内の DB・PDF）の削除

---

## 5. コード署名（Code Signing）

OpenRef は現在 Apple Developer Program の公証（Notarization）ではなく、**ad-hoc コード署名**（`codesign -s -`）でビルド・配布されています。

- ターミナルから `curl -fsSL https://openref.unitedradiology.jp/install.sh | bash` で導入した場合、ブラウザダウンロード由来の `com.apple.quarantine` 拡張属性が付与されないため、Gatekeeper の警告なしで起動できます。
- 署名の整合性は以下のコマンドで検証できます:
  ```bash
  codesign -v --deep /Applications/OpenRef.app
  codesign -v "$HOME/Library/Application Support/OpenRef/bin/openref-connectd"
  ```

---

## 6. MCP サーバ (`openref-mcp`) の安全機構

AI エージェントに蔵書操作を委譲する `openref-mcp` は、誤操作や SQL インジェクションによるデータ破損を防ぐため以下の多層防御を実装しています。

1. **`sql` ツールの厳格な読み取り専用化**:
   - SQLite 接続に `PRAGMA query_only = ON` を設定。
   - さらに **SQLite Authorizer (`rusqlite::hooks::Authorization`)** を登録し、`SELECT`・読み取り専用 `PRAGMA`・関数呼び出し以外のすべての操作（`INSERT`, `UPDATE`, `DELETE`, `DROP`, `ATTACH` 等）をコンパイル段階で拒否します。
   - 単文のみ許可（複数文は拒否）し、行数上限（既定 200・最大 2,000 行）、文字数上限（100,000 字）、実行時間上限（10 秒で `Interrupt`）を設けています。
2. **書込コマンド実行前の自動スナップショット**:
   - エージェントが `call` ツールで書込コマンドを初めて実行する直前に、DB 全体のオンラインバックアップを `~/Library/Application Support/OpenRef/backup/pre-agent-<日時>.sqlite` に自動作成します（最新 5 個を保持。バックアップ作成に失敗した場合は書込自体を中止します）。OpenRef / OpenNote のチャット（下の 4・環境変数 `OPENREF_MCP_CHAT=1` で起動）と許可一覧（`OPENREF_MCP_ALLOW`）で起動したセッションでは `pre-chat-<日時>.sqlite` に作り、こちらも最新 5 個を別に保持します（`pre-agent-` を押し出しません）。
3. **安全な復元手順（`.restore`）**:
   - 万一エージェントの書込を巻き戻したい場合は、WAL（`-wal`）との不整合を防ぐため `cp` / `mv` ではなく SQLite の `.restore` コマンドを使用します:
     ```bash
     sqlite3 "$HOME/Library/Application Support/OpenRef/openref.sqlite" \
       ".restore '$HOME/Library/Application Support/OpenRef/backup/pre-agent-<日時>.sqlite'"
     ```
     チャットや許可一覧で起動したセッションの書込を巻き戻すときは `pre-chat-<日時>.sqlite` を同じ手順で指定します。
4. **OpenRef / OpenNote のチャット（既定 OFF）**:
   - 利用者が自分でインストールした Claude Code CLI を子プロセスとして起動します（shell を通さない）。ログインは利用者の Claude Code のもので、チャットの [ログイン] は Terminal.app に `claude auth login` を開くだけ・[ログアウト] は `claude auth logout` を実行します。OpenRef は認証情報・API キーを保持しません。発言と Claude が読んだ内容（ワークスペースのページ・添付・蔵書の情報・Claude が読みに行ったその他のファイル）は Claude Code から Anthropic へ送られ、会話記録は Claude Code が `~/.claude/projects/` に保存します（チャットを無効にしても残ります）。
   - **ふるまいは Claude Code をターミナルで普通に使うときと同じです**: 組み込みの道具（Read / Edit / Write / Bash / Web…）を確認なしで使い（`--permission-mode bypassPermissions`）、利用者の Claude Code の設定（`~/.claude/` の settings・hooks・plugins・memory）と `CLAUDE.md`（ワークスペースの root と、その親・利用者のもの）を読みます。ワークスペースの中に限る制限はありません。MCP だけは `--strict-mcp-config` で `openref-mcp`（アプリと同じ DB・蔵書と OpenNote の全コマンド）に限ります。ページや PDF の中の指示の文に従わないことは ワークスペースの `CLAUDE.md` の規則に委ねます（無ければ OpenRef が既定の文を置き、利用者が編集します）。
   - `openref-mcp` の `call` で書くときは、初回の書込の前に DB の写し（`backup/pre-chat-<日時>.sqlite`・上の 2）を作ります。`wiki_write` / `wiki_move` で書いたページは上書き前の内容が `wiki-history/` に退避され、チャットの [元に戻す] で戻せます（その後に利用者が編集したファイル・Edit / Write / Bash で直接変えたファイルは戻しません）。
   - **会話のログ（既定 ON・Settings › OpenNote › チャットのログを chat-log/ に残す で OFF）**: 発言・Claude の応答・使った道具の要点・変えたファイルの一覧を ワークスペースの `chat-log/<日時> <最初の発言>.md` に追記します（発言に添えた選択範囲の本文も含みます）。ワークスペースの中なので OpenNote の索引に入り、以後のチャットで Claude が読めます。原本は Claude Code の `~/.claude/projects/` で、OFF にしても既に書いたファイルは残ります（不要なら削除してください）。
   - 内蔵の PDF ビューアの [チャット] タブは同じ会話で、開いている文献の key と題名・選択した本文を発言に添えます。
5. **OpenRef / OpenNote の参照フォルダと URL の窓（任意）**:
   - 参照フォルダ（右ペインの home で追加・読み取り専用）の下の pdf / html / 画像 / 動画 / 音声は、ワークスペースと同じく asset protocol の配信範囲に加えて内蔵の表示で開きます（md と他の種類は OS の既定アプリ）。配信範囲の追加は OpenRef の再起動まで取り消せません（外した直後は内蔵で開くのだけ止まります）。索引・検索・MCP・編集の範囲は広がりません。
   - 「リンク（URL）を OpenRef の窓で開く」（既定 OFF）を ON にすると、OpenNote のページの http(s) リンクを OpenRef の別窓（WKWebView）で開きます。この窓には OpenRef の API（Tauri の capabilities）を与えず、http(s) 以外への遷移とダウンロードを拒否します。ワークスペースと参照フォルダを配信する `asset://` は OpenRef 自身が配信し、要求元がメインの窓でなければ 403 を返します（別窓に読み込んだ外部ページからは読めません）。Cookie はブラウザと別で、ページの内容は通常のブラウザと同じくそのサイトへ送られます。
   - この窓のページから**カメラ / マイクは使えません**: 利用者の Mac で実測したところ、ページの `navigator.mediaDevices` は未定義で `getUserMedia` は呼べず、OS の許可ダイアログも出ません（OpenRef はカメラ / マイクの利用目的を Info.plist に宣言していません）。将来この窓でカメラ / マイクを使えるようにする予定はなく、WebView 側で明示的に拒否する実装（Tauri 2.12 以降の許可ハンドラ）は公開後の課題として記録しています。

---

## 7. 脆弱性の報告

セキュリティ上の問題を発見された場合は、公開 Issue に詳細な攻撃手順を記載する前に、GitHub リポジトリの Security Advisory（またはメンテナ連絡先）を通じてご報告ください。確認次第、修正版を準備して対応いたします。
