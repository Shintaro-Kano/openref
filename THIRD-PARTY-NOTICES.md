# Third-Party Notices (`THIRD-PARTY-NOTICES.md`)

OpenRef 本体のソースコード（`crates/*`, `app/src/*`, `app/src-tauri/*`, `extension/*`, `word/*.{js,html,xml}`, `site/*`, `scripts/*`）は **MIT License**（[LICENSE](LICENSE)）の下で提供されます。

本プロジェクトが同梱またはビルド時にリンクする第三者ソフトウェア・アセットのライセンスおよび著作権表示は以下の通りです。

---

## 1. 同梱 JavaScript / XML アセット

### 1.1 `citeproc-js`
- **所在**: `word/vendor/citeproc.js`（および `app/public/cite/citeproc.js`）
- **著作権**: Copyright (c) 2009–2019 Frank Bennett
- **ライセンス**: Common Public Attribution License Version 1.0 (**CPAL-1.0**) OR GNU Affero General Public License v3.0 or later
- **本プロジェクトでの利用**: CPAL-1.0 の条件に従い、無改変の独立ファイルとして同梱しています。ソースコードおよびヘッダは `word/vendor/citeproc.js` に保持されています（<https://github.com/Juris-M/citeproc-js>）。

### 1.2 Citation Style Language (CSL) スタイル・ロケールファイル
- **所在**: `word/styles/*.csl`, `word/vendor/locales-en-US.xml`（および `app/public/cite/` からの参照）
- **著作権**: Citation Style Language Contributors (Michael Berkowitz, Andrew Dunning, Julian Onions, Adam Tunis, Francis Deng, Sebastian Karcher, Rintze M. Zelle, et al.)
- **ライセンス**: Creative Commons Attribution-ShareAlike 3.0 Unported (**CC BY-SA 3.0**, <https://creativecommons.org/licenses/by-sa/3.0/>)

### 1.3 PDF.js (`pdfjs-dist`)
- **所在**: `app/` フロントエンドバンドル（`pdfjs-dist` v6.1.200）
- **著作権**: Copyright Mozilla Foundation and contributors
- **ライセンス**: **Apache-2.0** (<https://www.apache.org/licenses/LICENSE-2.0>)

### 1.4 Svelte / Tauri フロントエンドパッケージ
- **所在**: `app/` フロントエンドバンドル
- **パッケージ**:
  - `svelte` (5.x): MIT License — Copyright (c) 2016–2026 Rich Harris and contributors
  - `@tauri-apps/api`, `@tauri-apps/plugin-dialog` (2.x): Apache-2.0 OR MIT — Copyright (c) 2019–2026 Tauri Programme within The Commons Conservancy

---

## 2. Rust クレート依存関係（ビルド時リンク）

OpenRef の各バイナリ（`OpenRef.app`, `openref`, `openref-mcp`, `openref-connectd`）がリンクする Rust クレート群（`aarch64-apple-darwin` ターゲット、計 355 パッケージ）はすべて MIT / Apache-2.0 / BSD / ISC / Zlib / MPL-2.0 / Unicode-3.0 / CDLA-Permissive-2.0 のいずれかで提供されており、GPL / LGPL / AGPL クレートは含まれません（`scripts/check-licenses.py` で検証可能）。

主要な依存クレートおよび特記すべき NOTICE 表示は以下の通りです。

| クレート群 | ライセンス | 著作権・備考 |
|---|---|---|
| `tauri`, `wry`, `muda`, `tray-icon` | Apache-2.0 OR MIT | Copyright (c) Tauri Programme within The Commons Conservancy |
| `tao` | Apache-2.0 | Copyright (c) 2014-2020 The winit contributors / Copyright (c) 2021-2026 Tauri Programme |
| `rusqlite`, `libsqlite3-sys` | MIT | Copyright (c) 2014-2026 The rusqlite developers（同梱される SQLite C ライブラリ本体は Public Domain） |
| `tokio`, `axum`, `hyper`, `tower` | MIT | Copyright (c) Tokio Contributors / Tower Contributors |
| `serde`, `serde_json`, `thiserror`, `anyhow`, `clap`, `regex`, `time`, `url`, `ureq`, `roxmltree`, `rcgen`, `walkdir` | MIT OR Apache-2.0 | 各クレート著作者 |
| `pdf-extract`, `lopdf`, `adobe-cmap-parser` | MIT | 各クレート著作者 |
| `objc2`, `objc2-*`, `block2`, `dispatch2` | MIT / Zlib OR Apache-2.0 OR MIT | Copyright (c) Mads Marquart and contributors |
| `ring` | Apache-2.0 AND ISC | Copyright 2015-2024 Brian Smith / Google LLC / OpenSSL Project (BoringSSL 由来コードの ISC ライセンス表示を含む) |
| `aws-lc-rs`, `aws-lc-sys` | ISC AND Apache-2.0 AND MIT AND BSD-3-Clause | Copyright Amazon.com, Inc. or its affiliates / Google Inc. / OpenSSL Project |
| `rustls`, `rustls-webpki`, `webpki-roots` | Apache-2.0 OR ISC OR MIT / CDLA-Permissive-2.0 | Copyright (c) Joseph Birr-Pixton and contributors / Mozilla CA Root Program (CDLA-Permissive-2.0) |
| `icu_*`, `zerovec`, `tinystr`, `litemap`, `yoke` | Unicode-3.0 | Copyright (c) 2020-2026 Unicode, Inc. (Unicode License v3) |
| `cssparser`, `selectors`, `dtoa-short`, `option-ext` | MPL-2.0 | Mozilla Public License 2.0（無改変クレートとしてリンク） |

---

## 3. 外部ツールおよび開発時検証用ファイル（非同梱）

### 3.1 Poppler (`pdftotext`) — **非同梱**
- OpenRef は Poppler (`pdftotext`, GPL) を**一切同梱・配布していません**。
- 内蔵の PDF テキスト抽出器（`pdf-extract` / MIT）で文字化けが検出された場合に限り、ユーザーの Mac に `/opt/homebrew/bin/pdftotext` 等が導入されていれば外部プロセスとして呼び出す任意フォールバック構成をとっています。

### 3.2 Office Add-in XML Schema (`word/xsd/*.xsd`) — **ビルド・配布物には非同梱**
- リポジトリ内の `word/xsd/` は、`word/manifest.xml` の構文検証（`xmllint --schema`）にのみ使用する Microsoft Open Specifications（`MS-OWEMXML`）のスキーマ定義であり、配布バイナリやインストーラには含まれません。
