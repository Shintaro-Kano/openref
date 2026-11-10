use openref_core::{shelf, Db};
use serde_json::{json, Value};
use std::{path::PathBuf, process::Command};

/// 一時フォルダに DB と本棚を置く（履歴は DB に直接書く = 開くのは GUI だけ）。子プロセスの HOME だけを一時フォルダにする
struct Scratch {
    base: PathBuf,
    db: PathBuf,
    root: String,
}
impl Scratch {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!("openref-cli-shelf-{}-{}", std::process::id(), openref_core::keys::new_key()));
        let books = base.join("教科書");
        std::fs::create_dir_all(books.join("解剖学/1冊だけ")).unwrap();
        std::fs::create_dir_all(base.join("app")).unwrap();
        std::fs::write(books.join("解剖学/1冊だけ/a.pdf"), "%PDF-1.4").unwrap();
        std::fs::write(books.join("b.pdf"), "%PDF-1.4").unwrap();
        let base = std::fs::canonicalize(&base).unwrap();
        let root = base.join("教科書").to_string_lossy().into_owned();
        let db = base.join("app/openref.sqlite");
        let d = Db::open(&db).unwrap();
        shelf::with_workspaces(&d, |m| {
            shelf::touch(m, &root, 5);
            Ok(())
        })
        .unwrap();
        Self { base, db, root }
    }
    fn call(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_openref")).env("HOME", &self.base).arg("--db").arg(&self.db).arg("call").args(args).output().unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let out = self.call(args);
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap()
    }
    fn fails(&self, args: &[&str]) -> String {
        let out = self.call(args);
        assert!(!out.status.success(), "{args:?} は失敗する");
        assert!(out.stdout.is_empty(), "{args:?}");
        String::from_utf8_lossy(&out.stderr).into_owned()
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

/// shelf_status / shelf_list / shelf_set_opts が登録表どおりに通り、設定が走査に効く
#[test]
fn call_reads_and_writes_the_shelf() {
    let s = Scratch::new();
    let st = s.ok(&["shelf_status"]);
    assert_eq!(st["shelves"][0]["root"], s.root);
    assert_eq!(st["shelves"][0]["name"], "教科書");
    assert_eq!(st["shelves"][0]["exists"], true);
    let args = json!({"root": s.root}).to_string();
    let t = s.ok(&["shelf_list", &args]);
    assert_eq!(t["tree"]["totalBooks"], 2);
    assert_eq!(t["tree"]["children"][0]["name"], "解剖学");
    assert_eq!(t["tree"]["children"][0]["children"][0]["name"], "1冊だけ");
    let set = json!({"root": s.root, "collapse_single": true, "aliases": {"解剖学": "Anatomy"}}).to_string();
    assert_eq!(s.ok(&["shelf_set_opts", &set])["collapse_single"], true);
    let t = s.ok(&["shelf_list", &args]);
    assert_eq!(t["tree"]["totalBooks"], 2);
    assert_eq!(t["tree"]["children"][0]["name"], "Anatomy");
    assert_eq!(t["tree"]["children"][0]["books"][0]["title"], "a", "畳まれた");
}

/// 止まる物: 履歴に無い root・shelf_* 以外の名前・JSON でない引数・未知の引数（どれも非 0・stdout は空・何も書かない）
#[test]
fn call_refuses_what_is_not_the_shelf() {
    let s = Scratch::new();
    let before = Db::open(&s.db).unwrap().setting(shelf::KEY).unwrap();
    let outside = json!({"root": s.base.to_string_lossy()}).to_string();
    assert!(s.fails(&["shelf_list", &outside]).contains("履歴に無い本棚"));
    let write = json!({"root": s.base.to_string_lossy(), "collapse_single": true}).to_string();
    assert!(s.fails(&["shelf_set_opts", &write]).contains("履歴に無い本棚"));
    assert!(s.fails(&["list_items"]).contains("shelf_* だけ"));
    assert!(s.fails(&["purge_item", r#"{"key": "ABCD1234"}"#]).contains("shelf_* だけ"));
    assert!(s.fails(&["shelf_status", "not json"]).contains("JSON"));
    assert!(s.fails(&["shelf_nope"]).contains("未知のコマンド"));
    assert!(s.fails(&["shelf_list", &json!({"root": s.root, "path": "x"}).to_string()]).contains("未知の引数 path"));
    assert_eq!(Db::open(&s.db).unwrap().setting(shelf::KEY).unwrap(), before);
}
