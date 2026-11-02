use openref_core::Db;
use serde_json::Value;
use std::{path::PathBuf, process::Command};

/// 一時フォルダに DB と vault を置く（DB の設定で Wiki を有効にし、root を vault に向ける）。vault はホームの中しか
/// 使えないので、子プロセスの HOME だけを一時フォルダにする（このテストのプロセスの HOME は変えない）
struct Scratch {
    root: PathBuf,
    db: PathBuf,
}
impl Scratch {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "openref-cli-wiki-{}-{}",
            std::process::id(),
            openref_core::keys::new_key()
        ));
        let vault = root.join("vault");
        for (rel, body) in [
            ("a.md", "---\ntags: [ct, mri]\n---\n# A\n"),
            ("b.md", "---\ntags: [ct]\n---\n# B\n"),
            ("ingest/src.md", "# 素材\n"),
            ("chat-log/2026-11-01 0900 x.md", "# log 1\n"),
            ("chat-log/2026-11-02 0900 y.md", "# log 2\n"),
        ] {
            let p = vault.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        let root = std::fs::canonicalize(&root).unwrap();
        let vault = root.join("vault");
        let db = root.join("app/openref.sqlite");
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        let d = Db::open(&db).unwrap();
        d.set_setting("wiki_enabled", "1").unwrap();
        d.set_setting("wiki_root", &vault.to_string_lossy()).unwrap();
        Self { root, db }
    }
    fn cmd(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_openref"));
        c.env("HOME", &self.root).arg("--db").arg(&self.db).arg("wiki");
        c
    }
    fn wiki(&self, args: &[&str]) -> Value {
        let out = self.cmd().args(args).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap()
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn wiki_tags_and_inbox_dir_reach_the_registry() {
    let s = Scratch::new();
    let t = s.wiki(&["tags", "--limit", "1"]);
    assert_eq!((t["total"].as_u64(), t["count"].as_u64()), (Some(2), Some(1)), "{t}");
    assert_eq!((t["tags"][0]["tag"].as_str(), t["tags"][0]["count"].as_u64()), (Some("ct"), Some(2)), "多い順");

    let i = s.wiki(&["inbox"]);
    assert_eq!((i["dir"].as_str(), i["count"].as_u64()), (Some("ingest/"), Some(1)), "{i}");
    let c = s.wiki(&["inbox", "--dir", "chat-log", "-n", "1"]);
    assert_eq!((c["dir"].as_str(), c["total"].as_u64(), c["count"].as_u64()), (Some("chat-log/"), Some(2), Some(1)), "{c}");
    assert_eq!(c["files"][0]["path"], "chat-log/2026-11-02 0900 y.md", "新しい順");

    let bad = s.cmd().args(["inbox", "--dir", "notes"]).output().unwrap();
    assert!(!bad.status.success() && String::from_utf8_lossy(&bad.stderr).contains("dir は"));
}
