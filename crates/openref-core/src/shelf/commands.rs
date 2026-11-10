//! 本棚の登録表の command（読み = status / list・書き = set_opts）。GUI の包みも同じ関数を呼ぶ。
//! 開く / 切替 / 履歴から外すは GUI だけ（鍵を足せない = agent は履歴にある root しか走査できない）。
//! 読みは Ctx（Db を要しない）。走査はロックの外

use super::{with_workspaces, Ctx, Opts, Scanned};
use crate::wiki::{strip_root, workspaces};
use crate::{CoreError, Db, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use unicode_normalization::UnicodeNormalization;

fn bad(msg: impl Into<String>) -> CoreError {
    CoreError::BadInput(msg.into())
}

/// 履歴の 1 行
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ShelfRoot {
    pub root: String,
    pub name: String,
    pub last_used: i64,
    pub exists: bool,
    pub collapse_single: bool,
    pub issue_titles: bool,
    pub aliases: HashMap<String, String>,
}

/// 引数 `root`（文字列）
pub fn root_arg(a: &Value) -> Result<String> {
    match a.get("root") {
        Some(Value::String(s)) => Ok(s.clone()),
        None | Some(Value::Null) => Err(bad("引数 root は必須です（shelf_status の root をそのまま渡す）")),
        Some(_) => Err(bad("root は文字列で渡してください（shelf_status の root）")),
    }
}

/// 履歴（新しい順）。exists = フォルダが今もある（無いフォルダは GUI で灰色）
pub fn status(ctx: &Ctx, _a: &Value) -> Result<Value> {
    let shelves: Vec<ShelfRoot> = workspaces::order(&ctx.workspaces)
        .into_iter()
        .map(|(root, last_used)| {
            let o = ctx.opts(&root);
            let p = Path::new(&root);
            ShelfRoot {
                name: o.display_name(p),
                exists: p.is_dir(),
                collapse_single: o.collapse_single,
                issue_titles: o.issue_titles,
                aliases: o.aliases,
                root,
                last_used,
            }
        })
        .collect();
    Ok(json!({"shelves": shelves}))
}

/// 走査結果の JSON（dir = root 相対のフォルダ・無ければ全体）。shelf_list と GUI の shelf_tree の共通の形
pub fn tree_json(root: &str, s: &Scanned, dir: Option<&str>) -> Result<Value> {
    let dir = dir.map(|d| d.trim().trim_matches('/')).filter(|d| !d.is_empty());
    let tree = match dir {
        None => &s.tree,
        Some(d) => {
            let want: String = d.nfc().collect();
            find_dir(&s.tree, &s.root, &want).ok_or_else(|| bad(format!("本棚に無いフォルダです: {d}（root 相対・畳まれたフォルダは本になっている）")))?
        }
    };
    Ok(json!({"root": root, "dir": dir, "tree": tree, "unreadable": s.unreadable, "used_find": s.used_find}))
}

/// root 相対（NFC）が want のフォルダ（木の中だけを見る = 隠したフォルダ・`..` は当たらない）
fn find_dir<'a>(f: &'a super::scan::ShelfFolder, root: &Path, want: &str) -> Option<&'a super::scan::ShelfFolder> {
    if strip_root(root, Path::new(&f.path)).as_deref() == Some(want) {
        return Some(f);
    }
    f.children.iter().find_map(|c| find_dir(c, root, want))
}

/// 本の木（毎回走査）。root は履歴にある物だけ
pub fn list(ctx: &Ctx, a: &Value) -> Result<Value> {
    let root = root_arg(a)?;
    let dir = match a.get("dir") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.as_str()),
        Some(_) => return Err(bad("dir は文字列で渡してください（root 相対のフォルダ）")),
    };
    let s = Scanned::of(ctx, &root)?;
    tree_json(&root, &s, dir)
}

/// 引数の値を項目へ（無い・null = 変えない）。name の空白・aliases の空 = 項目を消す（既定に戻す）
fn apply_opts(o: &mut serde_json::Map<String, Value>, a: &Value) -> Result<()> {
    for k in ["collapse_single", "issue_titles"] {
        match a.get(k) {
            None | Some(Value::Null) => {}
            Some(Value::Bool(b)) => {
                o.insert(k.to_string(), json!(b));
            }
            Some(_) => return Err(bad(format!("{k} は true / false で渡してください"))),
        }
    }
    match a.get("aliases") {
        None | Some(Value::Null) => {}
        Some(Value::Object(m)) => {
            if m.iter().any(|(k, v)| k.trim().is_empty() || !v.as_str().is_some_and(|s| !s.trim().is_empty())) {
                return Err(bad("aliases は {\"フォルダ名\": \"表示名\"}（どちらも空でない文字列）で渡してください"));
            }
            if m.is_empty() {
                o.remove("aliases");
            } else {
                o.insert("aliases".into(), Value::Object(m.clone()));
            }
        }
        Some(_) => return Err(bad("aliases は {\"フォルダ名\": \"表示名\"} の object で渡してください")),
    }
    match a.get("name") {
        None | Some(Value::Null) => {}
        Some(Value::String(s)) if s.trim().is_empty() => {
            o.remove("name");
        }
        Some(Value::String(s)) => {
            o.insert("name".into(), json!(s.trim()));
        }
        Some(_) => return Err(bad("name は文字列で渡してください（空 = フォルダ名に戻す）")),
    }
    Ok(())
}

/// 表示の設定を書く。root は履歴にある物だけ（鍵を足さない・last_used を変えない）。戻り値 = 書いた後の設定
pub fn set_opts(db: &Db, a: &Value) -> Result<Value> {
    let root = root_arg(a)?;
    let o: Opts = with_workspaces(db, |m| {
        let e = m.get_mut(&root).and_then(Value::as_object_mut).ok_or_else(|| bad(format!("履歴に無い本棚です: {root}（root には shelf_status の root をそのまま渡す）")))?;
        apply_opts(e, a)?;
        Ok(Opts::of(m.get(&root)))
    })?;
    Ok(json!({
        "root": root,
        "name": o.display_name(Path::new(&root)),
        "collapse_single": o.collapse_single,
        "issue_titles": o.issue_titles,
        "aliases": o.aliases,
    }))
}

#[cfg(test)]
mod tests {
    use super::super::tests::fix;
    use super::super::{touch, KEY};
    use super::*;

    /// status: 新しい順・exists は今のディスク・表示名は name かフォルダ名・設定も返す
    #[test]
    fn status_lists_history() {
        let f = fix("status");
        let mut ctx = f.ctx.clone();
        let gone = f.base.join("home/gone").to_string_lossy().into_owned();
        touch(&mut ctx.workspaces, &gone, 500);
        ctx.workspaces.get_mut(&f.root).unwrap()["name"] = json!("教科書棚");
        ctx.workspaces.get_mut(&f.root).unwrap()["aliases"] = json!({"a": "A"});
        let v = status(&ctx, &json!({})).unwrap();
        let rows = v["shelves"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], json!({"root": gone, "name": "gone", "last_used": 500, "exists": false, "collapse_single": false, "issue_titles": false, "aliases": {}}));
        assert_eq!(rows[1]["name"], "教科書棚");
        assert_eq!(rows[1]["exists"], true);
        assert_eq!(rows[1]["aliases"], json!({"a": "A"}));
        assert_eq!(status(&Ctx::default(), &json!({})).unwrap(), json!({"shelves": []}));
    }

    /// list: Folder / Book の形（camelCase の totalBooks）・dir で絞る・履歴に無い root は BadInput・無い dir も BadInput
    #[test]
    fn list_walks_the_tree() {
        let f = fix("list");
        let v = list(&f.ctx, &json!({"root": f.root})).unwrap();
        assert_eq!(v["root"], f.root);
        assert_eq!(v["dir"], Value::Null);
        assert_eq!(v["unreadable"], 0);
        assert_eq!(v["used_find"], false);
        let t = &v["tree"];
        assert_eq!(t["name"], "教科書");
        assert_eq!(t["totalBooks"], 2);
        assert_eq!(t["books"][0]["title"], "b");
        let b = t["books"][0].as_object().unwrap();
        let mut keys: Vec<&str> = b.keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, ["folder", "id", "mtime", "path", "size", "title"]);
        assert_eq!(t["children"][0]["name"], "解剖学");

        for d in ["解剖学", "/解剖学/", " 解剖学"] {
            let v = list(&f.ctx, &json!({"root": f.root, "dir": d})).unwrap();
            assert_eq!(v["dir"], "解剖学", "{d:?}");
            assert_eq!(v["tree"]["name"], "解剖学");
            assert_eq!(v["tree"]["totalBooks"], 1);
        }
        assert_eq!(list(&f.ctx, &json!({"root": f.root, "dir": ""})).unwrap()["tree"]["totalBooks"], 2);
        for d in ["nope", "..", "解剖学/..", "../教科書"] {
            let e = list(&f.ctx, &json!({"root": f.root, "dir": d})).unwrap_err();
            assert!(matches!(e, CoreError::BadInput(_)) && e.to_string().contains("本棚に無いフォルダ"), "{d}: {e}");
        }
        let e = list(&f.ctx, &json!({"root": f.base.to_string_lossy()})).unwrap_err();
        assert!(matches!(e, CoreError::BadInput(_)) && e.to_string().contains("履歴に無い本棚"), "{e}");
        assert!(list(&f.ctx, &json!({})).unwrap_err().to_string().contains("root は必須"));
        assert!(list(&f.ctx, &json!({"root": 1})).unwrap_err().to_string().contains("root は文字列"));
        assert!(list(&f.ctx, &json!({"root": f.root, "dir": 1})).unwrap_err().to_string().contains("dir は文字列"));
    }

    fn db_with(root: &str) -> Db {
        let db = Db::open_in_memory().unwrap();
        with_workspaces(&db, |m| {
            touch(m, root, 7);
            m.get_mut(root).unwrap()["future"] = json!(1);
            Ok(())
        })
        .unwrap();
        db
    }

    fn raw(db: &Db, root: &str) -> Value {
        let m: Value = serde_json::from_str(&db.setting(KEY).unwrap().unwrap()).unwrap();
        m[root].clone()
    }

    /// set_opts: 渡した項目だけ変える・未知の項目と last_used は保つ・name の空白 / aliases の空で既定へ・履歴に無い root は書かない
    #[test]
    fn set_opts_merges_and_keeps_rest() {
        let db = db_with("/s/教科書");
        let v = set_opts(&db, &json!({"root": "/s/教科書", "collapse_single": true, "aliases": {"journal-a": "雑誌 A"}, "name": " 雑誌 ", "issue_titles": true})).unwrap();
        assert_eq!(v, json!({"root": "/s/教科書", "name": "雑誌", "collapse_single": true, "issue_titles": true, "aliases": {"journal-a": "雑誌 A"}}));
        assert_eq!(raw(&db, "/s/教科書"), json!({"last_used": 7, "collapse_single": true, "future": 1, "aliases": {"journal-a": "雑誌 A"}, "name": "雑誌", "issue_titles": true}));
        // 省略・null は変えない
        set_opts(&db, &json!({"root": "/s/教科書", "collapse_single": false, "name": null})).unwrap();
        let r = raw(&db, "/s/教科書");
        assert_eq!((r["collapse_single"].clone(), r["name"].clone()), (json!(false), json!("雑誌")));
        // 既定へ戻す
        let v = set_opts(&db, &json!({"root": "/s/教科書", "name": "  ", "aliases": {}})).unwrap();
        assert_eq!(v["name"], "教科書");
        assert_eq!(v["aliases"], json!({}));
        let r = raw(&db, "/s/教科書");
        assert!(r.get("name").is_none() && r.get("aliases").is_none(), "{r}");

        let before = db.setting(KEY).unwrap();
        for (a, want) in [
            (json!({"root": "/other", "collapse_single": true}), "履歴に無い本棚"),
            (json!({"root": "/s/教科書/", "collapse_single": true}), "履歴に無い本棚"),
            (json!({"root": "/s/教科書", "collapse_single": "yes"}), "true / false"),
            (json!({"root": "/s/教科書", "aliases": ["a"]}), "object"),
            (json!({"root": "/s/教科書", "aliases": {"a": 1}}), "空でない文字列"),
            (json!({"root": "/s/教科書", "aliases": {"a": " "}}), "空でない文字列"),
            (json!({"root": "/s/教科書", "aliases": {" ": "x"}}), "空でない文字列"),
            (json!({"root": "/s/教科書", "name": 3}), "name は文字列"),
            (json!({"collapse_single": true}), "root は必須"),
        ] {
            let e = set_opts(&db, &a).unwrap_err();
            assert!(matches!(e, CoreError::BadInput(_)) && e.to_string().contains(want), "{a}: {e}");
        }
        assert_eq!(db.setting(KEY).unwrap(), before, "拒否は何も書かない");
    }

    /// 設定 → 走査: collapse_single / aliases / name が shelf_list に効く（set_opts と list の合流）
    #[test]
    fn set_opts_reaches_the_scan() {
        let f = fix("reach");
        let root = Path::new(&f.root);
        std::fs::create_dir_all(root.join("journal-a/art")).unwrap();
        std::fs::write(root.join("journal-a/art/original.pdf"), "%PDF-1.4").unwrap();
        let db = Db::open_in_memory().unwrap();
        db.set_setting(KEY, &workspaces::serialize(&f.ctx.workspaces)).unwrap();
        set_opts(&db, &json!({"root": f.root, "collapse_single": true, "aliases": {"journal-a": "雑誌 A"}, "name": "本棚 A"})).unwrap();
        let ctx = Ctx { workspaces: workspaces::parse_for(KEY, db.setting(KEY).unwrap().as_deref()).unwrap(), ..f.ctx.clone() };
        let t = list(&ctx, &json!({"root": f.root})).unwrap()["tree"].clone();
        assert_eq!(t["name"], "本棚 A");
        assert_eq!(t["totalBooks"], 3, "畳んでも冊数は変わらない");
        let names: Vec<&str> = t["children"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["雑誌 A"], "alias・解剖学/a.pdf は畳まれた");
        assert_eq!(t["children"][0]["books"][0]["title"], "art", "original → 記事フォルダ名");
    }
}
