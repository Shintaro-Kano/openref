//! OpenNote の編集の実弾。scratch DB（`wiki_enabled=1`・`wiki_root=<vault の写し>`）を OPENREF_WIKI_REAL_DB に渡して手で走らせる:
//! OPENREF_WIKI_REAL_DB=<scratch.sqlite> OPENREF_WIKI_PROD_ROOTS=<触らない vault を : 区切り> \
//!   cargo test -p openref-core --release --test wiki_edit_real -- --ignored --nocapture --test-threads=1 <名前>
//! 本番の vault・DB を渡さない（vault に実書込する）。OPENREF_WIKI_PROD_ROOTS は必須（守る vault が無ければ空）: 挙げた vault と
//! その上下が root なら止める（本番の DB の写しは wiki_root に本番の vault を持つ）

use openref_core::wiki::{commands, edit, Ctx};
use openref_core::Db;
use serde_json::{json, Value};
use std::path::PathBuf;

/// 本番を渡したら止める: DB が既定の置き場（~/Library/Application Support/OpenRef/openref.sqlite）・vault が OPENREF_WIKI_PROD_ROOTS の
/// どれか（かその上下）（どれも canonicalize して比べる = symlink・`..`・大小の違いで抜けない）
fn ctx() -> Ctx {
    // env の確認は DB を開く前に（未設定なら空の DB を作らずに止める）
    let prod_roots = std::env::var("OPENREF_WIKI_PROD_ROOTS")
        .expect("OPENREF_WIKI_PROD_ROOTS（触ってはいけない vault を : 区切りで。無ければ空）");
    let canon = |p: &std::path::Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    let db_path = canon(&PathBuf::from(std::env::var("OPENREF_WIKI_REAL_DB").expect("OPENREF_WIKI_REAL_DB")));
    let prod_db = canon(&home.join("Library/Application Support/OpenRef/openref.sqlite"));
    assert!(db_path != prod_db && !db_path.starts_with(canon(&home.join("Library/Application Support/OpenRef"))), "本番の DB: {}", db_path.display());
    let db = Db::open(&db_path).unwrap();
    let c = Ctx::from_db(&db).unwrap();
    let root = canon(&PathBuf::from(c.root.clone().expect("scratch DB に wiki_root が無い（wiki_enabled=1 で vault の写しを指す設定が要る）")));
    for prod in prod_roots.split(':').filter(|s| !s.is_empty()) {
        let prod = canon(std::path::Path::new(prod));
        assert!(!root.starts_with(&prod) && !prod.starts_with(&root), "本番の vault: {}", root.display());
    }
    c
}

fn root(c: &Ctx) -> PathBuf {
    c.ready().unwrap().1
}

/// 外部（別プロセス）の書換えと競合
#[test]
#[ignore]
fn real_conflict() {
    let c = ctx();
    let path = std::env::var("OPENREF_WIKI_REAL_PAGE").expect("OPENREF_WIKI_REAL_PAGE");
    let r = commands::read(&c, &json!({"path": path})).unwrap();
    let sha0 = r["sha256"].as_str().unwrap().to_string();
    let body = r["content"].as_str().unwrap().to_string();
    let w = edit::write(&c, &json!({"path": path, "content": format!("{body}\n<!-- GUI の保存 -->\n"), "if_sha256": sha0})).unwrap();
    println!("1 GUI の保存（読んだ sha256 で）: ok={} backup={}", w["ok"], w.get("backup").is_some());
    let sha1 = w["sha256"].as_str().unwrap().to_string();
    let abs = root(&c).join(&path);
    // 別プロセスが書き換える
    let st = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg("printf '%s\\n' '# 外部が書き換えた' 'Claude Code の内容' > \"$1\"")
        .arg("sh")
        .arg(&abs)
        .status()
        .unwrap();
    assert!(st.success());
    let external = std::fs::read_to_string(&abs).unwrap();
    let c2 = edit::write(&c, &json!({"path": path, "content": "GUI の古い内容", "if_sha256": sha1})).unwrap();
    println!("2 古い sha256 で保存: {c2}");
    assert_eq!(c2["conflict"], true);
    assert_eq!(std::fs::read_to_string(&abs).unwrap(), external, "外部の内容のまま");
    let f = edit::write(&c, &json!({"path": path, "content": "GUI で上書き", "if_sha256": sha1, "force": true})).unwrap();
    println!("3 force: ok={} backup={}", f["ok"], f["backup"]);
    let backed = std::fs::read_to_string(f["backup"].as_str().unwrap()).unwrap();
    assert_eq!(backed, external, "wiki-history に外部の内容");
    assert_eq!(std::fs::read_to_string(&abs).unwrap(), "GUI で上書き");
    let hd = edit::history_dir(c.app_dir.as_deref().unwrap(), &root(&c), &path);
    println!("4 wiki-history の世代: {}（{}）", edit::history_files(&hd).len(), hd.display());
}

/// 改名（OPENREF_WIKI_REAL_FROM → OPENREF_WIKI_REAL_TO）。preview と実際の件数・退避・未解決の前後
#[test]
#[ignore]
fn real_rename() {
    let c = ctx();
    let from = std::env::var("OPENREF_WIKI_REAL_FROM").expect("OPENREF_WIKI_REAL_FROM");
    let to = std::env::var("OPENREF_WIKI_REAL_TO").expect("OPENREF_WIKI_REAL_TO");
    let before = commands::status(&c, &json!({})).unwrap();
    let bl = commands::backlinks(&c, &json!({"path": from, "limit": 10000})).unwrap();
    let t = std::time::Instant::now();
    let p = edit::rename_preview(&c, &json!({"from": from, "to": to})).unwrap();
    let tp = t.elapsed();
    println!("backlinks（索引）: {} ページ", bl["total"]);
    println!("preview: links={} files={} other={} ({:?})", p["links"], p["files"].as_array().unwrap().len(), p["other"], tp);
    let t = std::time::Instant::now();
    let r = edit::rename(&c, &json!({"from": from, "to": to, "rewrite": true})).unwrap();
    let tr = t.elapsed();
    println!("rename: links={} rewritten={} skipped={} index_error={:?} ({:?})", r["links"], r["rewritten"].as_array().unwrap().len(), r["skipped"], r.get("index_error"), tr);
    assert_eq!(r["links"], p["links"]);
    let app = c.app_dir.clone().unwrap();
    let mut with_hist = 0;
    for f in r["rewritten"].as_array().unwrap() {
        if !edit::history_files(&edit::history_dir(&app, &root(&c), f.as_str().unwrap())).is_empty() {
            with_hist += 1;
        }
    }
    println!("wiki-history に残ったファイル: {with_hist} / {}", r["rewritten"].as_array().unwrap().len());
    let after = commands::status(&c, &json!({})).unwrap();
    println!("未解決: {} → {}・ambiguous: {} → {}", before["unresolved"], after["unresolved"], before["ambiguous"], after["ambiguous"]);
    let bl2 = commands::backlinks(&c, &json!({"path": to, "limit": 10000})).unwrap();
    println!("改名後の backlinks: {} ページ", bl2["total"]);
    let v: Value = json!({"preview": p, "result": r});
    std::fs::write(std::env::temp_dir().join("wiki-rename.json"), serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

/// 貼付・取込（OPENREF_WIKI_REAL_OUTSIDE = vault の外のファイル）
#[test]
#[ignore]
fn real_blob_import() {
    use base64::Engine;
    let c = ctx();
    let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
    for _ in 0..2 {
        println!("save_blob: {}", edit::save_blob(&c, &json!({"name": "スクリーンショット 2026-10-31 23.00.png", "data_base64": b64})).unwrap());
    }
    let outside = std::env::var("OPENREF_WIKI_REAL_OUTSIDE").expect("OPENREF_WIKI_REAL_OUTSIDE");
    for _ in 0..2 {
        println!("import(外): {}", edit::import_file(&c, &json!({"abs": outside})).unwrap());
    }
    let inside = root(&c).join(std::env::var("OPENREF_WIKI_REAL_INSIDE").expect("OPENREF_WIKI_REAL_INSIDE"));
    println!("import(中): {}", edit::import_file(&c, &json!({"abs": inside})).unwrap());
}

/// 索引を作る・件数
#[test]
#[ignore]
fn real_status() {
    let c = ctx();
    let t = std::time::Instant::now();
    let s = commands::status(&c, &json!({})).unwrap();
    println!("{} ({:?})", serde_json::to_string(&s).unwrap(), t.elapsed());
}
