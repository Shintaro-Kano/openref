use anyhow::{bail, Context};
use clap::{Parser, Subcommand};
use openref_core::{datadir, fulltext, ingest, model, pdffetch, resolver, search, sync, CoreError, Db};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "openref", version, about = "OpenRef — LLM 統合型ローカル文献管理")]
struct Cli {
    /// DB パス（既定: 環境変数 OPENREF_DB、無ければ ~/Library/Application Support/OpenRef/openref.sqlite）
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// PMID / DOI / PubMed URL / doi.org URL から書誌確定して登録
    Add { input: String },
    /// 最近の item を一覧（--journal / --author で GUI の検索チップと同じ絞り込み）
    List {
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
        /// 誌名で絞る（full / short どちらでも。表記ゆれ・ISSN 経由で同一誌を束ねる）
        #[arg(long)]
        journal: Option<String>,
        /// 著者の姓で絞る（位置は問わない）。"Kano S" のように頭文字を付けると同姓の別人を外す
        #[arg(long)]
        author: Option<String>,
        /// ラベル名で絞る（大小無視）。各行の末尾に付いているラベルを [..] で出す
        #[arg(long)]
        label: Option<String>,
    },
    /// 検索（title / abstract / 著者 / 誌名 / keywords+MeSH + 添付 PDF の本文。GUI の検索ボックスと同じ経路）
    Search {
        #[arg(required = true)]
        query: Vec<String>,
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
    /// item 詳細表示
    Show {
        key: String,
        /// CSL-JSON をそのまま出力
        #[arg(long)]
        json: bool,
    },
    /// 外部ソースからの一括 import
    Import {
        #[command(subcommand)]
        source: ImportSrc,
    },
    /// フォルダ（collection）ツリー表示
    Collections,
    /// Drive mount と DB の差分スキャン（報告のみ）
    Sync {
        #[arg(long)]
        mount: Option<PathBuf>,
    },
    /// PDF 自動取得（PMC AWS Open Data → Unpaywall（OPENREF_EMAIL 設定時）→ Crossref link → doi.org 着地ページ。同一性照合つき）
    FetchPdf {
        key: Option<String>,
        /// PDF 未添付の全 item を対象にする
        #[arg(long)]
        missing: bool,
    },
    /// 書誌を PubMed / doi.org で引き直して是正（connector 由来の壊れた著者などの修復）
    Refresh {
        key: Option<String>,
        /// 取込元で絞る（例: connector）
        #[arg(long)]
        source: Option<String>,
        /// PMID/DOI を持つ全 item を対象にする
        #[arg(long)]
        all: bool,
    },
    /// DB パスと件数
    Info,
    /// PDF 本文の全文索引（検索ボックスで本文もヒットさせる。Paperpile の full-text search 相当）
    Fulltext {
        #[command(subcommand)]
        action: FulltextAction,
    },
    /// PDF 書込先を Drive 互換レイアウトへ切替
    Cutover {
        #[command(subcommand)]
        action: CutoverAction,
    },
    /// 引用スタイル（CSL・約 1 万誌）の検索・取得（GUI の「引用スタイルを変更…」と同じ経路）
    Style {
        #[command(subcommand)]
        action: StyleAction,
    },
}

#[derive(Subcommand)]
enum StyleAction {
    /// 索引から誌名・id を検索（語は AND・大小無視）
    Search { query: String },
    /// citeproc に渡す解決済み CSL を標準出力へ（dependent は親で解決。未取得なら取得してキャッシュ）
    Get { id: String },
    /// locale XML を標準出力へ
    Locale { lang: String },
    /// 同梱 + 導入済みの一覧と既定スタイル
    List,
}

#[derive(Subcommand)]
enum FulltextAction {
    /// 既存の全 PDF 添付を一括索引（索引済みは md5 で判定してスキップ。1 本ごとに commit するので Ctrl-C 後の再実行で続きから）
    Index {
        /// 索引済みも抽出し直す
        #[arg(long)]
        force: bool,
    },
    /// 索引の現況（添付数 / 索引済み / テキスト層なし / 失敗 / 未索引）と FTS 索引の整合検査
    Status,
    /// FTS 索引を fulltext テーブルから作り直す（status で不整合が出たときの修復。本文の再抽出はしない）
    Rebuild,
}

#[derive(Subcommand)]
enum CutoverAction {
    /// 現在のモード・添付内訳・mount 検出状態を表示
    Status,
    /// preflight（mount 検出・All Papers 存在・書込プローブ）後に drive モードへ切替
    Enable,
    /// managed モードへ戻す（新規添付のみ影響。既存 linked はそのまま）
    Disable,
    /// managed 添付を Drive 互換レイアウトへ移行（冪等・読み戻し照合・storage 原本は残置）
    MigrateManaged {
        /// 書き込まずに移行先だけ報告
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum ImportSrc {
    /// Paperpile JSON export（初回 seed。folders/添付リンク込み）
    Paperpile {
        json: PathBuf,
        /// Paperpile の Drive mount（省略時は自動検出）
        #[arg(long)]
        mount: Option<PathBuf>,
        /// 書き込まずに結果だけ報告
        #[arg(long)]
        dry_run: bool,
    },
    /// 手元の PDF を一括登録（冒頭 2 頁の DOI/PMID → 書誌確定 → 登録＋添付。識別子なしは登録しない）
    Pdf {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let path = match cli.db.clone() {
        Some(p) => p,
        None => datadir::db_path()?,
    };
    let mut db =
        Db::open(&path).with_context(|| format!("DB open 失敗: {}", path.display()))?;
    match cli.cmd {
        Cmd::Add { input } => cmd_add(&mut db, &input)?,
        Cmd::List { limit, journal, author, label } => {
            if journal.is_none() && author.is_none() && label.is_none() {
                print_rows(&search::list(&db, limit)?);
            } else {
                let (family, initial) = match author.as_deref().map(str::trim) {
                    Some(a) if !a.is_empty() => {
                        let mut it = a.split_whitespace();
                        let fam = it.next().unwrap_or("").to_string();
                        let ini = it.next().and_then(|g| g.chars().next()).map(|c| c.to_string());
                        (Some(fam), ini)
                    }
                    _ => (None, None),
                };
                let rows = search::list_items(
                    &db,
                    &search::ListFilter {
                        limit,
                        journal,
                        author_family: family,
                        author_initial: initial,
                        tag: label,
                        ..Default::default()
                    },
                )?;
                if rows.is_empty() {
                    println!("(なし)");
                }
                for r in &rows {
                    print_row(&r.item);
                    if !r.tags.is_empty() {
                        let names: Vec<String> =
                            r.tags.iter().map(|t| format!("{}({})", t.name, t.color.as_deref().unwrap_or("-"))).collect();
                        println!("      [{}]", names.join(", "));
                    }
                }
            }
        }
        Cmd::Search { query, limit } => {
            let q = query.join(" ");
            let rows = search::list_items(
                &db,
                &search::ListFilter { search: Some(q.clone()), limit, ..Default::default() },
            )?;
            if rows.is_empty() {
                println!("(hit なし: {q})");
            }
            for r in &rows {
                print_row(&r.item);
                if let Some(snip) = &r.fulltext_snippet {
                    // 一致箇所のマーカー（U+E000/U+E001）は端末では [ ] に
                    let snip = snip
                        .replace(fulltext::HL_START, "[")
                        .replace(fulltext::HL_END, "]");
                    println!("              本文: {snip}");
                }
            }
        }
        Cmd::Show { key, json } => cmd_show(&db, &key, json)?,
        Cmd::Import { source } => match source {
            ImportSrc::Pdf { paths } => cmd_import_pdf(&mut db, &paths)?,
            ImportSrc::Paperpile {
                json,
                mount,
                dry_run,
            } => {
                let m = mount.or_else(openref_core::mount::detect);
                match &m {
                    Some(p) => eprintln!("mount: {}", p.display()),
                    None => eprintln!("mount: (未検出 — PDF リンクはスキップされます)"),
                }
                let rep = openref_core::import::paperpile::import(&mut db, &json, m, dry_run)?;
                println!(
                    "=== Paperpile import{} ===",
                    if rep.dry_run { "（dry-run・未書込）" } else { "" }
                );
                println!(
                    "records {} / 新規 {} / merge {} / 不採用 {}",
                    rep.total, rep.inserted, rep.merged, rep.skipped_bad
                );
                println!(
                    "collections 作成 {} / PDF リンク {} / PDF 不明 {} / DOI・PMID なし {}",
                    rep.collections_created,
                    rep.pdf_linked,
                    rep.pdf_missing.len(),
                    rep.without_ids
                );
                for f in &rep.pdf_missing {
                    println!("  PDF 不明: {f}");
                }
                if !rep.dry_run {
                    println!(
                        "本文索引 {} / 未索引 {}（未索引は openref fulltext index で再試行）",
                        rep.fulltext_indexed, rep.fulltext_unindexed
                    );
                }
            }
        },
        Cmd::Fulltext { action } => cmd_fulltext(&db, action)?,
        Cmd::Collections => cmd_collections(&db)?,
        Cmd::Sync { mount } => {
            let m = mount
                .or_else(openref_core::mount::detect)
                .context("Paperpile mount が見つかりません（--mount で指定してください）")?;
            let rep = sync::scan(&db, &m)?;
            println!("mount: {}", m.display());
            if rep.enumeration_blocked {
                println!(
                    "⚠ mount のフォルダ列挙が OS に拒否されています（macOS のプライバシー許可）。\n  システム設定 > プライバシーとセキュリティ > ファイルとフォルダ でターミナルに許可を与えて再実行してください。\n  この状態では新規 PDF の検出は機能しません（既存リンクの実体確認は有効）。"
                );
            }
            println!(
                "mount PDFs {} / linked 添付 {} / 新規 {} / orphan {}",
                rep.mount_pdfs,
                rep.linked,
                rep.new_files.len(),
                rep.orphans.len()
            );
            for f in rep.new_files.iter().take(30) {
                println!("  new: {f}");
            }
            if rep.new_files.len() > 30 {
                println!("  … 他 {} 件", rep.new_files.len() - 30);
            }
            for f in &rep.orphans {
                println!("  orphan: {f}");
            }
        }
        Cmd::FetchPdf { key, missing } => cmd_fetch_pdf(&db, key.as_deref(), missing)?,
        Cmd::Refresh { key, source, all } => {
            cmd_refresh(&mut db, key.as_deref(), source.as_deref(), all)?
        }
        Cmd::Info => {
            let c = search::counts(&db)?;
            println!("DB: {}", path.display());
            println!(
                "items {}  attachments {}  annotations {}  notes {}  collections {}  tags {}",
                c.items, c.attachments, c.annotations, c.notes, c.collections, c.tags
            );
        }
        Cmd::Cutover { action } => cmd_cutover(&db, action)?,
        Cmd::Style { action } => cmd_style(&db, action)?,
    }
    Ok(())
}

fn cmd_style(db: &Db, action: StyleAction) -> anyhow::Result<()> {
    use openref_core::citestyle;
    let dir = citestyle::styles_dir(db);
    match action {
        StyleAction::Search { query } => {
            let words: Vec<String> = query.to_lowercase().split_whitespace().map(String::from).collect();
            let cat = citestyle::catalog(&dir, false)?;
            let hits: Vec<_> = cat
                .iter()
                .filter(|v| {
                    let hay = format!(
                        "{} {} {}",
                        v["title"].as_str().unwrap_or(""),
                        v["short"].as_str().unwrap_or(""),
                        v["id"].as_str().unwrap_or("")
                    )
                    .to_lowercase();
                    words.iter().all(|w| hay.contains(w.as_str()))
                })
                .collect();
            println!("{} 件 / 索引 {} 件", hits.len(), cat.len());
            for v in hits.iter().take(50) {
                println!(
                    "{:<55} {:<12} {}{}",
                    v["id"].as_str().unwrap_or(""),
                    v["format"].as_str().unwrap_or(""),
                    v["title"].as_str().unwrap_or(""),
                    if v["dep"].as_bool() == Some(true) { "  (dependent)" } else { "" }
                );
            }
        }
        StyleAction::Get { id } => print!("{}", citestyle::style_xml(&dir, &id)?),
        StyleAction::Locale { lang } => print!("{}", citestyle::locale_xml(&dir, &lang)?),
        StyleAction::List => {
            println!("既定: {}", citestyle::prefs(db));
            for v in citestyle::installed(&dir) {
                println!("{v}");
            }
        }
    }
    Ok(())
}

fn cmd_cutover(db: &Db, action: CutoverAction) -> anyhow::Result<()> {
    use openref_core::{drive_layout, mount};
    let count_mode = |m: &str| -> i64 {
        db.conn
            .query_row("SELECT COUNT(*) FROM attachments WHERE mode = ?1", [m], |r| r.get(0))
            .unwrap_or(0)
    };
    match action {
        CutoverAction::Status => {
            println!("pdf_write_mode: {}", db.pdf_write_mode());
            println!("attachments: linked {} / managed {}", count_mode("linked"), count_mode("managed"));
            match mount::detect() {
                Some(m) => println!("mount: {}", m.display()),
                None => println!("mount: (未検出)"),
            }
        }
        CutoverAction::Enable => {
            // preflight 1: mount
            let m = mount::detect().ok_or_else(|| {
                anyhow::anyhow!("Drive mount が見つかりません（Drive for Desktop 稼働と Paperpile フォルダを確認。OPENREF_MOUNT で明示も可）")
            })?;
            // preflight 2: All Papers 実在（Paperpile レイアウトの存在確認）
            let all_papers = m.join("All Papers");
            if !all_papers.is_dir() {
                bail!("{} がありません — Paperpile レイアウトが確認できず中止", all_papers.display());
            }
            // preflight 3: 書込プローブ（File Provider は readdir 拒否でも write は通る想定を実測で確認）
            let probe = all_papers.join(".openref-write-probe");
            std::fs::write(&probe, b"probe")
                .with_context(|| format!("書込プローブ失敗: {}", probe.display()))?;
            std::fs::remove_file(&probe).ok();
            db.set_setting("pdf_write_mode", "drive")?;
            println!("✓ cutover 有効化: 以後の新規 PDF 添付は Drive 互換レイアウトへ書きます");
            println!("  mount: {}", m.display());
            let managed = count_mode("managed");
            if managed > 0 {
                println!("  ※ managed 添付が {managed} 件あります。`openref cutover migrate-managed` で Drive へ移行できます");
            }
        }
        CutoverAction::Disable => {
            db.set_setting("pdf_write_mode", "managed")?;
            println!("✓ managed モードへ戻しました（既存の linked 添付には影響しません）");
        }
        CutoverAction::MigrateManaged { dry_run } => {
            let m = mount::detect()
                .ok_or_else(|| anyhow::anyhow!("Drive mount が見つかりません"))?;
            let rows: Vec<(i64, String, String, String)> = {
                let mut stmt = db.conn.prepare(
                    "SELECT a.id, a.path, i.key, i.csl FROM attachments a
                     JOIN items i ON i.id = a.item_id WHERE a.mode = 'managed' ORDER BY a.id",
                )?;
                let r = stmt
                    .query_map([], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                r
            };
            if rows.is_empty() {
                println!("managed 添付はありません（移行済み）");
                return Ok(());
            }
            for (att_id, src_path, item_key, csl_s) in rows {
                let csl: serde_json::Value = serde_json::from_str(&csl_s)?;
                let (initial, filename) = drive_layout::relative_target(&csl);
                if dry_run {
                    println!("[dry-run] {item_key}: {src_path}\n  → All Papers/{initial}/{filename}");
                    continue;
                }
                let bytes = std::fs::read(&src_path)
                    .with_context(|| format!("storage 原本を読めません: {src_path}"))?;
                let dest = drive_layout::place_bytes(&m, &csl, &bytes)?;
                // 読み戻し照合（サイズ + 内容。Drive 書込の完全性を確認してから DB を書き換える）
                let back = std::fs::read(&dest)?;
                if back != bytes {
                    bail!("照合不一致: {} — DB は変更していません", dest.display());
                }
                db.conn.execute(
                    "UPDATE attachments SET mode = 'linked', path = ?1 WHERE id = ?2",
                    (dest.to_string_lossy().to_string(), att_id),
                )?;
                println!("✓ {item_key}: → {}", dest.display());
                println!("  (storage 原本は保険で残置: {src_path})");
            }
        }
    }
    Ok(())
}

fn cmd_add(db: &mut Db, input: &str) -> anyhow::Result<()> {
    let parsed = resolver::parse_input(input)?;
    match &parsed {
        resolver::AddInput::Pmid(p) => eprintln!("resolving PMID {p} …"),
        resolver::AddInput::Doi(d) => eprintln!("resolving DOI {d} …"),
    }
    let resolved = resolver::resolve(&parsed)?;
    match ingest::add_item(db, &resolved) {
        Ok(added) => {
            let c = &resolved.csl;
            println!("✓ 登録 {}", added.key);
            println!(
                "  {} ({}) {}",
                model::first_author_family(c),
                model::year(c).map(|y| y.to_string()).unwrap_or_else(|| "n.d.".into()),
                model::s(c, "title").unwrap_or("(no title)")
            );
            let mut jl = String::new();
            if let Some(j) = model::s(c, "container-title") {
                jl.push_str(j);
            }
            if let Some(v) = model::s(c, "volume") {
                jl.push_str(&format!(" {v}"));
            }
            if let Some(i) = model::s(c, "issue") {
                jl.push_str(&format!("({i})"));
            }
            if let Some(p) = model::s(c, "page") {
                jl.push_str(&format!(":{p}"));
            }
            if !jl.is_empty() {
                println!("  {jl}");
            }
            let ids: Vec<String> = ["DOI", "PMID", "PMCID"]
                .iter()
                .filter_map(|k| model::s(c, k).map(|v| format!("{k} {v}")))
                .collect();
            if !ids.is_empty() {
                println!("  {}", ids.join(" · "));
            }
            if model::s(c, "PMCID").is_some() {
                println!("  (PMC 収載あり → fetch-pdf で取得できます)");
            }
            if let Some(r) = &resolved.retracted {
                println!("  ⚠ 撤回論文の可能性: {r}");
            }
        }
        Err(CoreError::Duplicate {
            existing_key,
            field,
        }) => {
            bail!("既に登録済みです（{field} 一致・item {existing_key}）");
        }
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

fn cmd_show(db: &Db, key: &str, as_json: bool) -> anyhow::Result<()> {
    let Some(item) = search::get_by_key(db, key)? else {
        bail!("item {key} が見つかりません");
    };
    if as_json {
        println!("{}", serde_json::to_string_pretty(&item.csl)?);
        return Ok(());
    }
    let c = &item.csl;
    println!("{}  {}", item.key, model::s(c, "title").unwrap_or("(no title)"));
    let auth = model::authors(c);
    if !auth.is_empty() {
        println!("著者    : {}", auth.join("; "));
    }
    let mut line = String::new();
    if let Some(j) = model::s(c, "container-title") {
        line.push_str(j);
    }
    if let Some(y) = model::year(c) {
        line.push_str(&format!("  ({y})"));
    }
    if let Some(v) = model::s(c, "volume") {
        line.push_str(&format!("  {v}"));
    }
    if let Some(i) = model::s(c, "issue") {
        line.push_str(&format!("({i})"));
    }
    if let Some(p) = model::s(c, "page") {
        line.push_str(&format!(":{p}"));
    }
    if !line.is_empty() {
        println!("掲載    : {line}");
    }
    for k in ["DOI", "PMID", "PMCID"] {
        if let Some(v) = model::s(c, k) {
            println!("{k:<7}: {v}");
        }
    }
    if let Some(kw) = model::s(c, "keyword") {
        println!("keyword : {kw}");
    }
    if let Some(mesh) = c.pointer("/custom/mesh").and_then(|m| m.as_array()) {
        let ms: Vec<&str> = mesh.iter().filter_map(|x| x.as_str()).collect();
        if !ms.is_empty() {
            println!("MeSH    : {}", ms.join("; "));
        }
    }
    if let Some(r) = search::retraction_of(db, item.id)? {
        println!("⚠ 撤回  : {r}");
    }
    if let Some(a) = model::s(c, "abstract") {
        println!("---\n{a}");
    }
    println!(
        "---\nadded: {}  source: {}",
        item.date_added,
        c.pointer("/custom/source").and_then(|s| s.as_str()).unwrap_or("?")
    );
    Ok(())
}

fn cmd_fetch_pdf(db: &Db, key: Option<&str>, missing: bool) -> anyhow::Result<()> {
    let keys: Vec<String> = if missing {
        let mut stmt = db.conn.prepare(
            "SELECT key FROM items i WHERE i.deleted_at IS NULL
             AND NOT EXISTS (SELECT 1 FROM attachments a WHERE a.item_id = i.id)
             ORDER BY i.id",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.filter_map(|r| r.ok()).collect()
    } else {
        vec![key.context("key か --missing を指定してください")?.to_string()]
    };
    if keys.is_empty() {
        println!("PDF 未添付の item はありません");
        return Ok(());
    }
    println!("対象 {} 件", keys.len());
    for k in keys {
        match pdffetch::fetch_pdf(db, &k) {
            Ok(f) => println!("✓ {k}  {} ({} bytes, {})", f.filename, f.bytes_len, f.source),
            Err(e) => println!("✗ {k}  {e}"),
        }
    }
    Ok(())
}

fn cmd_refresh(
    db: &mut Db,
    key: Option<&str>,
    source: Option<&str>,
    all: bool,
) -> anyhow::Result<()> {
    let keys: Vec<String> = if let Some(k) = key {
        vec![k.to_string()]
    } else if source.is_some() || all {
        let mut stmt = db.conn.prepare(
            "SELECT key FROM items
             WHERE deleted_at IS NULL
               AND (pmid IS NOT NULL OR doi IS NOT NULL)
               AND (?1 IS NULL OR json_extract(csl, '$.custom.source') = ?1)
             ORDER BY id",
        )?;
        let rows = stmt.query_map([source], |r| r.get::<_, String>(0))?;
        rows.filter_map(|r| r.ok()).collect()
    } else {
        bail!("KEY か --source <src> か --all を指定してください");
    };
    if keys.is_empty() {
        println!("対象なし");
        return Ok(());
    }
    println!("対象 {} 件を引き直します", keys.len());
    let (mut upd, mut same, mut skip) = (0, 0, 0);
    for (i, k) in keys.iter().enumerate() {
        if i > 0 {
            // NCBI への礼儀（3 req/sec 制限）
            std::thread::sleep(NCBI_INTERVAL);
        }
        match ingest::refresh_item(db, k) {
            Ok(ingest::RefreshOutcome::Updated) => {
                upd += 1;
                let t = search::get_by_key(db, k)?
                    .and_then(|it| model::s(&it.csl, "title").map(|s| s.to_string()))
                    .unwrap_or_default();
                println!("✓ {k}  更新  {}", model::truncate_chars(&t, 56));
            }
            Ok(ingest::RefreshOutcome::Unchanged) => same += 1,
            Ok(ingest::RefreshOutcome::NoIdentifier) => skip += 1,
            Ok(ingest::RefreshOutcome::Rejected(msg)) => {
                skip += 1;
                println!("⚠ {k}  {msg}");
            }
            Err(e) => {
                skip += 1;
                println!("✗ {k}  {e}");
            }
        }
    }
    println!("更新 {upd} / 変化なし {same} / 対象外・失敗 {skip}");
    Ok(())
}

fn cmd_collections(db: &Db) -> anyhow::Result<()> {
    let rows = search::collections(db)?;
    if rows.is_empty() {
        println!("(collection なし)");
        return Ok(());
    }
    fn print_children(
        rows: &[search::CollectionRow],
        parent: Option<i64>,
        depth: usize,
    ) {
        for r in rows.iter().filter(|r| r.parent_id == parent) {
            println!("{}{} ({})", "  ".repeat(depth), r.name, r.n_items);
            print_children(rows, Some(r.id), depth + 1);
        }
    }
    print_children(&rows, None, 0);
    Ok(())
}

/// PDF 本文の一括索引 / 現況。1 本ごとに結果を出し、最後に内訳と所要時間を集計
fn cmd_fulltext(db: &Db, action: FulltextAction) -> anyhow::Result<()> {
    use openref_core::fulltext::{self, BackfillStep, IndexOutcome};
    match action {
        FulltextAction::Status => {
            let st = fulltext::status(db)?;
            println!(
                "PDF 添付 {} / 索引済み {} / 文字化け(partial) {} / テキスト層なし {} / 失敗 {} / 未索引 {}",
                st.total, st.ok, st.partial, st.empty, st.error, st.missing
            );
            if st.partial > 0 {
                println!(
                    "  partial = 内蔵抽出が文字化けし pdftotext が無かった本。poppler（brew install poppler）導入後に openref fulltext index で取り直し"
                );
            }
            let (n_ft, bytes): (i64, i64) = db.conn.query_row(
                "SELECT COUNT(*), COALESCE(SUM(length(text) + length(cjk)), 0) FROM fulltext",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            println!("fulltext 行 {n_ft} / 本文 + cjk 合計 {} KB", bytes / 1024);
            match fulltext::integrity_check(db)? {
                None => println!("FTS 索引の整合: OK"),
                Some(e) => println!("⚠ FTS 索引の整合: 不整合（{e}）→ openref fulltext rebuild で修復"),
            }
        }
        FulltextAction::Rebuild => {
            fulltext::rebuild(db)?;
            match fulltext::integrity_check(db)? {
                None => println!("✓ FTS 索引を作り直しました（整合 OK）"),
                Some(e) => println!("✗ 作り直し後も不整合: {e}"),
            }
        }
        FulltextAction::Index { force } => {
            let t0 = std::time::Instant::now();
            let rep = fulltext::backfill(db, force, |i, n, path, step| {
                let name = std::path::Path::new(path)
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.to_string());
                let line = match step {
                    BackfillStep::Already => "＝ 索引済み（スキップ）".to_string(),
                    BackfillStep::Done(IndexOutcome::Indexed { pages, chars, backend }) => {
                        if *backend == "builtin-garbled" {
                            format!("△ {pages} 頁 / {chars} 字（内蔵抽出が文字化け・pdftotext 無しのため partial で記録）")
                        } else {
                            format!("✓ {pages} 頁 / {chars} 字（{backend}）")
                        }
                    }
                    BackfillStep::Done(IndexOutcome::Empty { pages }) => {
                        format!("－ テキスト層なし（{pages} 頁・スキャン PDF の可能性。空で記録）")
                    }
                    BackfillStep::Done(IndexOutcome::Failed(r)) => format!("✗ 抽出失敗（空で記録）: {r}"),
                    BackfillStep::Done(IndexOutcome::Skipped(r)) => format!("… スキップ（未記録）: {r}"),
                };
                println!("[{i}/{n}] {name}\n    {line}");
            })?;
            println!(
                "=== 添付 {} / 索引 {} / テキスト層なし {} / 失敗 {} / 索引済みスキップ {} / 未ダウンロード等スキップ {} / {:.1} 秒 ===",
                rep.total,
                rep.indexed,
                rep.empty,
                rep.failed,
                rep.already,
                rep.skipped,
                t0.elapsed().as_secs_f64()
            );
            if rep.skipped > 0 {
                println!("未ダウンロード等でスキップした {} 本は、Finder で「オフラインで使用可能にする」後に再実行すると索引されます", rep.skipped);
            }
        }
    }
    Ok(())
}

fn print_rows(rows: &[model::ItemRow]) {
    if rows.is_empty() {
        println!("(なし)");
        return;
    }
    for r in rows {
        print_row(r);
    }
}

/// 一覧の 1 行（★・key・年・筆頭著者・題名）
fn print_row(r: &model::ItemRow) {
    let c = &r.csl;
    let star = if r.starred { "★" } else { " " };
    println!(
        "{} {}  {}  {:<18}  {}",
        star,
        r.key,
        model::year(c).map(|y| y.to_string()).unwrap_or_else(|| "----".into()),
        model::truncate_chars(&model::first_author_family(c), 18),
        model::truncate_chars(model::s(c, "title").unwrap_or("(no title)"), 70)
    );
}

/// 連続して NCBI（E-utilities・3 req/s）を引くときの間隔
const NCBI_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);

/// PDF 一括登録。1 件ごとに結果を出し、最後に内訳を集計（GUI の取込モーダルと同じ分類）。
fn cmd_import_pdf(db: &mut Db, paths: &[PathBuf]) -> anyhow::Result<()> {
    use openref_core::pdfingest::{ingest_pdf_report, is_review_stub, IngestOpts, PdfIngestOutcome};
    let (mut added, mut attached, mut review, mut dup, mut failed) = (0, 0, 0, 0, 0);
    for (i, p) in paths.iter().enumerate() {
        if i > 0 {
            // NCBI 3 req/s（1 件 = esearch + efetch）を超えないよう間隔を空ける
            std::thread::sleep(NCBI_INTERVAL);
        }
        let name = p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        // 理由（要確認の根拠・Drive → managed 降格）も出す
        let (res, warnings, reason) = match ingest_pdf_report(db, p, &IngestOpts::default()) {
            Ok(r) => (Ok(r.outcome), r.warnings, r.reason),
            Err(e) => (Err(e), Vec::new(), None),
        };
        let mut line = match res {
            Ok(PdfIngestOutcome::Added { key }) => {
                added += 1;
                format!("✓ 登録＋添付  {key}")
            }
            Ok(PdfIngestOutcome::AttachedToExisting { key }) => {
                attached += 1;
                format!("✓ 既存に添付  {key}")
            }
            Ok(PdfIngestOutcome::AttachedAdditional { key }) => {
                attached += 1;
                format!("✓ 既存に 2 本目を添付（別ファイル）  {key}")
            }
            Ok(PdfIngestOutcome::DuplicateFile { key }) => {
                dup += 1;
                let hint = if is_review_stub(db, &key) { "・相手は要確認 stub = 編集から DOI/PMID を引き直せます" } else { "" };
                format!("＝ 同一ファイルが添付済み（md5 一致・スキップ{hint}）  {key}")
            }
            Ok(PdfIngestOutcome::NeedsReview { key }) => {
                review += 1;
                format!(
                    "？ 要確認で登録（{}。ラベル「要確認」）  {key}",
                    reason.clone().unwrap_or_else(|| "識別子なし・タイトル検索未一致".into())
                )
            }
            Ok(PdfIngestOutcome::NoIdentifier) => {
                review += 1;
                format!("－ 識別子なし（stub 無効設定。{}）", reason.clone().unwrap_or_default())
            }
            Err(CoreError::Duplicate { existing_key, field }) => {
                dup += 1;
                format!("＝ 重複（{field}）既存 {existing_key}")
            }
            Err(e) => {
                failed += 1;
                format!("✗ 失敗: {e}")
            }
        };
        for w in warnings {
            line.push_str("\n    ⚠ ");
            line.push_str(&w);
        }
        println!("[{}/{}] {name}\n    {line}", i + 1, paths.len());
    }
    println!("=== 登録 {added} / 既存に添付 {attached} / 要確認 {review} / 重複 {dup} / 失敗 {failed} ===");
    Ok(())
}
