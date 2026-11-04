//! ワークスペースの中か・隠し区画でないか・symlink で外へ出ないか、の検査（core の読み書きの command と GUI の包みが使う）。
//! 文言は呼び手が付ける（`Reject` の理由ごとに各 command の文を出す = MCP / CLI / 画面の文は変えない）。
//!
//! 「隠し」は 3 種あり、意図して別（統一しない）:
//! - `scan::hidden`: vault 相対のどれかの区画が `.` 始まり（.git / .obsidian / .trash 等 = 走査が拾わない物）
//! - app の `wiki_has_dot_segment`: 区画が `.` / `..` そのもの（字面で外へ出る形。遷移の許可リストの規則なので app に置いたまま）
//! - `dot_component`: 絶対パスのどれかの区画が `.` 始まり（ワークスペースの外の文書の隠しフォルダ = Finder で表示に落とす）

use super::{scan, strip_root};
use std::path::{Component, Path, PathBuf};

/// p が root の中（root そのものは除く）で隠し区画（`scan::hidden`）を含まなければ vault 相対。字面で見る（ファイルシステムに
/// 触らない）= symlink で外へ出ないことまで見るなら canonical なパスを渡す
pub fn rel_inside(root: &Path, p: &Path) -> Option<String> {
    strip_root(root, p).filter(|r| !r.is_empty() && !scan::hidden(r))
}

/// p が base の中（base そのものは除く）か。隠し区画は問わない。両辺とも canonical なパスを渡す前提（strip_root と同じく文字列を
/// NFC に揃えて比べる・`.` の区画は解かない・空の base は false = Path::starts_with とは canonical でない入力で違う）
pub fn strictly_under(base: &Path, p: &Path) -> bool {
    strip_root(base, p).is_some_and(|r| !r.is_empty())
}

/// 絶対パスのどれかの区画が `.` 始まりか
pub fn dot_component(p: &Path) -> bool {
    p.components().any(|c| matches!(c, Component::Normal(s) if s.to_string_lossy().starts_with('.')))
}

/// symlink でないフォルダか（lstat）
pub fn is_real_dir(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_dir())
}

/// symlink でない通常のファイルか（lstat）
pub fn is_real_file(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_file())
}

/// `resolve_existing` の選択
#[derive(Debug, Clone, Copy, Default)]
pub struct Opts {
    /// 通常ファイルに加えてディレクトリも返す（GUI の wiki_open_path = iWork のパッケージ形式）
    pub allow_dir: bool,
    /// 字面で拒否するのは vault 相対の `..` だけにし、残りは canonicalize した実体で判定する（絶対パスの `..`・`.` 始まりの区画・
    /// ワークスペースの外にある symlink も、実体が中で隠し区画でなければ可 = MCP の許可一覧の host_path_in_vault）
    pub judge_by_canonical: bool,
}

/// `resolve_existing` の拒否の理由（判定の順）
#[derive(Debug)]
pub enum Reject {
    /// 絶対パスがワークスペースの外（字面）
    Outside,
    /// `..`・root の区画（字面）
    DotDot,
    /// `.` 始まりの区画（字面）。値 = 字面の vault 相対
    DotStart(String),
    /// 実在しない・辿れない（canonicalize の失敗）
    Missing(std::io::Error),
    /// 実体がワークスペースの外（symlink）
    Escapes,
    /// 実体が root そのもの
    Root,
    /// 実体の vault 相対が `.` 始まりの区画を含む
    Hidden(String),
    /// 実体が通常ファイルでない（allow_dir ならディレクトリでもない）。値 = 実体の vault 相対
    NotFile(String),
}

/// 存在するエントリの解決: given（vault 相対かワークスペースの中の絶対パス・呼び手が trim / NFC する）→ (実体の vault 相対, canonical)。
/// 字面の検査（ワークスペースの外・`..`・`.` 始まり。judge_by_canonical なら vault 相対の `..` だけ）→ canonicalize →
/// 実体の検査（外・root・隠し・種類）の順
pub fn resolve_existing(root: &Path, given: &Path, opts: Opts) -> Result<(String, PathBuf), Reject> {
    let dotdot = |p: &Path| p.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir));
    let canon = if opts.judge_by_canonical {
        if !given.is_absolute() && dotdot(given) {
            return Err(Reject::DotDot);
        }
        std::fs::canonicalize(root.join(given)).map_err(Reject::Missing)?
    } else {
        // 比べる両側を NFC に（root が NFD で実在するとき、NFC で渡された vault 内の絶対パスを外と誤らない = strip_root）
        let rel_given = if given.is_absolute() { PathBuf::from(strip_root(root, given).ok_or(Reject::Outside)?) } else { given.to_path_buf() };
        if dotdot(&rel_given) {
            return Err(Reject::DotDot);
        }
        if rel_given.components().any(|c| matches!(c, Component::Normal(s) if s.to_string_lossy().starts_with('.'))) {
            return Err(Reject::DotStart(rel_given.to_string_lossy().into_owned()));
        }
        std::fs::canonicalize(root.join(&rel_given)).map_err(Reject::Missing)?
    };
    let rel = strip_root(root, &canon).ok_or(Reject::Escapes)?;
    if rel.is_empty() {
        return Err(Reject::Root);
    }
    if scan::hidden(&rel) {
        return Err(Reject::Hidden(rel));
    }
    if !(canon.is_file() || opts.allow_dir && canon.is_dir()) {
        return Err(Reject::NotFile(rel));
    }
    Ok((rel, canon))
}

#[cfg(test)]
mod tests {
    use super::super::{commands, edit, index, Ctx};
    use serde_json::json;
    use std::path::{Path, PathBuf};

    /// 同値表の場（ワークスペース = home/Documents/vault）。base は canonical
    pub(crate) struct Fix {
        pub base: PathBuf,
        pub root: PathBuf,
        pub ctx: Ctx,
    }
    impl Drop for Fix {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    pub(crate) fn fix(tag: &str) -> Fix {
        let base = std::env::temp_dir().join(format!("openref-paths-{tag}-{}-{:x}", std::process::id(), fastrand::u64(..)));
        std::fs::create_dir_all(&base).unwrap();
        let base = std::fs::canonicalize(&base).unwrap();
        let home = base.join("home");
        let root = home.join("Documents/vault");
        let app = home.join("scratch-app");
        for d in [root.join("sub"), root.join(".obsidian"), app.clone()] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(root.join("a.pdf"), "%PDF-1.4").unwrap();
        std::fs::write(root.join("sub/b.md"), "b").unwrap();
        std::fs::write(root.join(".obsidian/x.md"), "x").unwrap();
        std::fs::write(base.join("outside.md"), "o").unwrap();
        std::fs::create_dir_all(base.join(".hid")).unwrap();
        std::fs::write(base.join(".hid/y.md"), "y").unwrap();
        std::os::unix::fs::symlink(base.join("outside.md"), root.join("out-link.md")).unwrap();
        std::os::unix::fs::symlink(root.join("a.pdf"), root.join("in-link.pdf")).unwrap();
        std::os::unix::fs::symlink(root.join("a.pdf"), base.join("to-in.pdf")).unwrap();
        std::os::unix::fs::symlink(root.join("sub"), root.join(".alias")).unwrap();
        std::os::unix::fs::symlink(root.join(".obsidian/x.md"), root.join("vis-link.md")).unwrap();
        let ctx = Ctx {
            enabled: true,
            root: Some(root.to_string_lossy().into_owned()),
            app_dir: Some(app),
            home: Some(home),
            watched: false,
            ..Default::default()
        };
        Fix { base, root, ctx }
    }

    /// 同値表の入力（ラベル, パス）。R = ワークスペース・B = その外の場
    pub(crate) fn inputs(f: &Fix) -> Vec<(&'static str, String)> {
        let r = |s: &str| f.root.join(s).to_string_lossy().into_owned();
        let b = |s: &str| f.base.join(s).to_string_lossy().into_owned();
        vec![
            ("中(相対)", "a.pdf".into()),
            ("中(絶対)", r("a.pdf")),
            ("外", b("outside.md")),
            ("外の隠し", b(".hid/y.md")),
            ("隠し(相対)", ".obsidian/x.md".into()),
            ("隠し(絶対)", r(".obsidian/x.md")),
            ("隠し(相対・無い)", ".obsidian/nope.md".into()),
            ("隠しsymlinkで中(相対)", ".alias/b.md".into()),
            ("隠しsymlinkで中(絶対)", r(".alias/b.md")),
            ("symlinkで外", r("out-link.md")),
            ("symlinkで中", r("in-link.pdf")),
            ("symlinkで隠し", r("vis-link.md")),
            ("外のsymlinkで中", b("to-in.pdf")),
            ("..入り(相対)", "sub/../a.pdf".into()),
            ("..入り(絶対)", r("sub/../a.pdf")),
            ("..で外(相対)", "../../../outside.md".into()),
            ("./入り(相対)", "./a.pdf".into()),
            ("空白付き(相対)", " a.pdf".into()),
            ("存在しない", r("nope.pdf")),
            ("dir", r("sub")),
            ("末尾/(絶対)", format!("{}/", r("sub"))),
            ("末尾/(相対)", "sub/".into()),
            ("root", f.root.to_string_lossy().into_owned()),
        ]
    }

    /// 結果の文字列から場の絶対パスを `<B>` に置き換える
    pub(crate) fn redact(f: &Fix, s: String) -> String {
        s.replace(&*f.base.to_string_lossy(), "<B>")
    }

    /// core の 6 箇所の同値表: 3-1 の前（直前の main）と後で同じ文字列になること（commit 本文の手順で旧にも同じ試験を当てて確かめた）
    fn core_table() -> String {
        let f = fix("table");
        let _serial = index::test_serial();
        let (ip, root) = f.ctx.ready().unwrap();
        let ix = index::Index::open(&ip, &root).unwrap();
        let mut out = String::new();
        for (label, s) in inputs(&f) {
            let rows: Vec<(&str, String)> = vec![
                ("resolve_entry", format!("{:?}", commands::resolve_entry(&root, &s, false).map_err(|e| e.to_string()))),
                ("resolve_entry(dir)", format!("{:?}", commands::resolve_entry(&root, &s, true).map_err(|e| e.to_string()))),
                ("host_path_in_vault", format!("{:?}", f.ctx.host_path_in_vault(Path::new(&s)).map_err(|e| e.to_string()))),
                (
                    "target_path",
                    format!("{:?}", edit::target_path(&root, &s).map(|t| (t.rel, t.abs, t.existing)).map_err(|e| e.to_string())),
                ),
                ("reject_symlink", format!("{:?}", edit::reject_symlink(&root, &s).map_err(|e| e.to_string()))),
                (
                    "import_file",
                    format!("{:?}", edit::import_file(&f.ctx, &json!({"abs": s})).map(|v| v.to_string()).map_err(|e| e.to_string())),
                ),
                ("Index::rel_of", format!("{:?}", ix.rel_of(Path::new(&s)))),
            ];
            for (site, r) in rows {
                out.push_str(&format!("{site} | {label} | {}\n", redact(&f, r)));
            }
        }
        out
    }

    /// 書き換えた式（旧の写し）と paths の関数が同じ入力で同じ値: 退避の置き場（wiki_chat の undo）・wiki_open_target の外の枝の隠し・
    /// wiki_ref_target の inside と import_file・wiki_chat_log の is_real_dir / is_real_file（本体は置き場を移しただけ）
    #[test]
    fn replaced_expressions_match() {
        use super::super::{scan, strip_root};
        use super::*;
        let f = fix("expr");
        let old_dot = |q: &Path| q.components().any(|c| matches!(c, Component::Normal(s) if s.to_string_lossy().starts_with('.')));
        let old_inside = |d: &Path, q: &Path| strip_root(d, q).filter(|r| !r.is_empty() && !scan::hidden(r));
        let bases = [f.root.clone(), f.root.join("sub"), f.base.clone(), f.root.join(".obsidian")];
        for (label, s) in inputs(&f) {
            let p = Path::new(&s);
            let canon = std::fs::canonicalize(p).ok();
            for q in [Some(p), canon.as_deref()].into_iter().flatten() {
                for b in &bases {
                    assert_eq!(q.starts_with(b) && q != b, strictly_under(b, q), "{label} {} {}", b.display(), q.display());
                    assert_eq!(old_inside(b, q), rel_inside(b, q), "{label} {} {}", b.display(), q.display());
                }
                assert_eq!(old_dot(q), dot_component(q), "{label} {}", q.display());
                assert_eq!(std::fs::symlink_metadata(q).is_ok_and(|m| m.file_type().is_dir()), is_real_dir(q), "{label}");
                assert_eq!(std::fs::symlink_metadata(q).is_ok_and(|m| m.file_type().is_file()), is_real_file(q), "{label}");
            }
        }
        // 値の向き（同値だけでなく、隠し・symlink を本当に見分けていること）
        assert_eq!(rel_inside(&f.root, &f.root.join("sub/b.md")).as_deref(), Some("sub/b.md"));
        assert_eq!(rel_inside(&f.root, &f.root.join(".obsidian/x.md")), None);
        assert_eq!(rel_inside(&f.root, &f.root), None);
        assert!(dot_component(&f.base.join(".hid/y.md")) && !dot_component(&f.base.join("outside.md")));
        assert!(is_real_file(&f.root.join("a.pdf")) && !is_real_file(&f.root.join("in-link.pdf")));
        assert!(is_real_dir(&f.root.join("sub")) && !is_real_dir(&f.root.join(".alias")));
        // strictly_under は旧の式（Path の区画比較 = `.` の区画を落とす・空の base は何でも含む）と関数としては同じでない（文字列を NFC に
        // 揃えて比べる）。下の行は旧の式と違う = 仕様として固定。呼び手（wiki_chat の undo_change）は両辺を canonicalize してから渡すので
        // canonical なパスでは到達しない（上の突き合わせは canonical と字面の入力で一致）
        for (base, p, old, new) in [
            ("/h", "/h/.", false, true),
            ("/h/.", "/h/x", true, false),
            ("", "/", true, false),
            ("/h/\u{30DC}", "/h/\u{30DB}\u{3099}/x", false, true),
        ] {
            let (b, q) = (Path::new(base), Path::new(p));
            assert_eq!((q.starts_with(b) && q != b, strictly_under(b, q)), (old, new), "{base} {p}");
        }
    }

    #[test]
    fn core_sites_equivalence_table() {
        let got = core_table();
        if std::env::var_os("PATHS_TABLE_DUMP").is_some() {
            print!("{got}");
        }
        assert_eq!(got, CORE_TABLE);
    }

    const CORE_TABLE: &str = r#"resolve_entry | 中(相対) | Ok(("a.pdf", "<B>/home/Documents/vault/a.pdf"))
resolve_entry(dir) | 中(相対) | Ok(("a.pdf", "<B>/home/Documents/vault/a.pdf"))
host_path_in_vault | 中(相対) | Ok("<B>/home/Documents/vault/a.pdf")
target_path | 中(相対) | Ok(("a.pdf", "<B>/home/Documents/vault/a.pdf", Some(("a.pdf", "<B>/home/Documents/vault/a.pdf"))))
reject_symlink | 中(相対) | Ok(())
import_file | 中(相対) | Err("入力を解釈できません: wiki_import_file: abs は絶対パスで渡してください: a.pdf")
Index::rel_of | 中(相対) | Some("a.pdf")
resolve_entry | 中(絶対) | Ok(("a.pdf", "<B>/home/Documents/vault/a.pdf"))
resolve_entry(dir) | 中(絶対) | Ok(("a.pdf", "<B>/home/Documents/vault/a.pdf"))
host_path_in_vault | 中(絶対) | Ok("<B>/home/Documents/vault/a.pdf")
target_path | 中(絶対) | Ok(("a.pdf", "<B>/home/Documents/vault/a.pdf", Some(("a.pdf", "<B>/home/Documents/vault/a.pdf"))))
reject_symlink | 中(絶対) | Ok(())
import_file | 中(絶対) | Ok("{\"copied\":false,\"embed\":\"![[a.pdf]]\",\"path\":\"a.pdf\"}")
Index::rel_of | 中(絶対) | Some("a.pdf")
resolve_entry | 外 | Err("入力を解釈できません: ワークスペースの外のパスは読めません: <B>/outside.md")
resolve_entry(dir) | 外 | Err("入力を解釈できません: ワークスペースの外のパスは読めません: <B>/outside.md")
host_path_in_vault | 外 | Err("入力を解釈できません: ワークスペース（<B>/home/Documents/vault）の外です: <B>/outside.md")
target_path | 外 | Err("入力を解釈できません: ワークスペースの外には書けません: <B>/outside.md")
reject_symlink | 外 | Ok(())
import_file | 外 | Err("入力を解釈できません: ワークスペースの外から取り込めるのは画像・PDF・動画・音声だけです: outside.md")
Index::rel_of | 外 | None
resolve_entry | 外の隠し | Err("入力を解釈できません: ワークスペースの外のパスは読めません: <B>/.hid/y.md")
resolve_entry(dir) | 外の隠し | Err("入力を解釈できません: ワークスペースの外のパスは読めません: <B>/.hid/y.md")
host_path_in_vault | 外の隠し | Err("入力を解釈できません: ワークスペース（<B>/home/Documents/vault）の外です: <B>/.hid/y.md")
target_path | 外の隠し | Err("入力を解釈できません: ワークスペースの外には書けません: <B>/.hid/y.md")
reject_symlink | 外の隠し | Ok(())
import_file | 外の隠し | Err("入力を解釈できません: ワークスペースの外から取り込めるのは画像・PDF・動画・音声だけです: y.md")
Index::rel_of | 外の隠し | None
resolve_entry | 隠し(相対) | Err("入力を解釈できません: パスに `..`・`.` 始まりの区画・root は使えません: .obsidian/x.md")
resolve_entry(dir) | 隠し(相対) | Err("入力を解釈できません: パスに `..`・`.` 始まりの区画・root は使えません: .obsidian/x.md")
host_path_in_vault | 隠し(相対) | Err("入力を解釈できません: `.` で始まるフォルダ・ファイルの中は使えません: .obsidian/x.md")
target_path | 隠し(相対) | Err("入力を解釈できません: パスに `.` 始まりの区画・制御文字は使えません: .obsidian/x.md")
reject_symlink | 隠し(相対) | Ok(())
import_file | 隠し(相対) | Err("入力を解釈できません: wiki_import_file: abs は絶対パスで渡してください: .obsidian/x.md")
Index::rel_of | 隠し(相対) | Some(".obsidian/x.md")
resolve_entry | 隠し(絶対) | Err("入力を解釈できません: パスに `..`・`.` 始まりの区画・root は使えません: <B>/home/Documents/vault/.obsidian/x.md")
resolve_entry(dir) | 隠し(絶対) | Err("入力を解釈できません: パスに `..`・`.` 始まりの区画・root は使えません: <B>/home/Documents/vault/.obsidian/x.md")
host_path_in_vault | 隠し(絶対) | Err("入力を解釈できません: `.` で始まるフォルダ・ファイルの中は使えません: .obsidian/x.md")
target_path | 隠し(絶対) | Err("入力を解釈できません: パスに `.` 始まりの区画・制御文字は使えません: <B>/home/Documents/vault/.obsidian/x.md")
reject_symlink | 隠し(絶対) | Ok(())
import_file | 隠し(絶対) | Err("入力を解釈できません: ワークスペースの外から取り込めるのは画像・PDF・動画・音声だけです: x.md")
Index::rel_of | 隠し(絶対) | Some(".obsidian/x.md")
resolve_entry | 隠し(相対・無い) | Err("入力を解釈できません: パスに `..`・`.` 始まりの区画・root は使えません: .obsidian/nope.md")
resolve_entry(dir) | 隠し(相対・無い) | Err("入力を解釈できません: パスに `..`・`.` 始まりの区画・root は使えません: .obsidian/nope.md")
host_path_in_vault | 隠し(相対・無い) | Err("入力を解釈できません: .obsidian/nope.md を開けません: No such file or directory (os error 2)")
target_path | 隠し(相対・無い) | Err("入力を解釈できません: パスに `.` 始まりの区画・制御文字は使えません: .obsidian/nope.md")
reject_symlink | 隠し(相対・無い) | Ok(())
import_file | 隠し(相対・無い) | Err("入力を解釈できません: wiki_import_file: abs は絶対パスで渡してください: .obsidian/nope.md")
Index::rel_of | 隠し(相対・無い) | Some(".obsidian/nope.md")
resolve_entry | 隠しsymlinkで中(相対) | Err("入力を解釈できません: パスに `..`・`.` 始まりの区画・root は使えません: .alias/b.md")
resolve_entry(dir) | 隠しsymlinkで中(相対) | Err("入力を解釈できません: パスに `..`・`.` 始まりの区画・root は使えません: .alias/b.md")
host_path_in_vault | 隠しsymlinkで中(相対) | Ok("<B>/home/Documents/vault/sub/b.md")
target_path | 隠しsymlinkで中(相対) | Err("入力を解釈できません: パスに `.` 始まりの区画・制御文字は使えません: .alias/b.md")
reject_symlink | 隠しsymlinkで中(相対) | Ok(())
import_file | 隠しsymlinkで中(相対) | Err("入力を解釈できません: wiki_import_file: abs は絶対パスで渡してください: .alias/b.md")
Index::rel_of | 隠しsymlinkで中(相対) | Some(".alias/b.md")
resolve_entry | 隠しsymlinkで中(絶対) | Err("入力を解釈できません: パスに `..`・`.` 始まりの区画・root は使えません: <B>/home/Documents/vault/.alias/b.md")
resolve_entry(dir) | 隠しsymlinkで中(絶対) | Err("入力を解釈できません: パスに `..`・`.` 始まりの区画・root は使えません: <B>/home/Documents/vault/.alias/b.md")
host_path_in_vault | 隠しsymlinkで中(絶対) | Ok("<B>/home/Documents/vault/sub/b.md")
target_path | 隠しsymlinkで中(絶対) | Err("入力を解釈できません: パスに `.` 始まりの区画・制御文字は使えません: <B>/home/Documents/vault/.alias/b.md")
reject_symlink | 隠しsymlinkで中(絶対) | Ok(())
import_file | 隠しsymlinkで中(絶対) | Ok("{\"copied\":false,\"embed\":\"[[sub/b.md]]\",\"path\":\"sub/b.md\"}")
Index::rel_of | 隠しsymlinkで中(絶対) | Some(".alias/b.md")
resolve_entry | symlinkで外 | Err("入力を解釈できません: ワークスペースの外を指しています（symlink）: <B>/home/Documents/vault/out-link.md")
resolve_entry(dir) | symlinkで外 | Err("入力を解釈できません: ワークスペースの外を指しています（symlink）: <B>/home/Documents/vault/out-link.md")
host_path_in_vault | symlinkで外 | Err("入力を解釈できません: ワークスペース（<B>/home/Documents/vault）の外です: <B>/home/Documents/vault/out-link.md")
target_path | symlinkで外 | Err("入力を解釈できません: symlink には書けません: <B>/home/Documents/vault/out-link.md")
reject_symlink | symlinkで外 | Err("入力を解釈できません: symlink は扱えません: <B>/home/Documents/vault/out-link.md")
import_file | symlinkで外 | Err("入力を解釈できません: ワークスペースの外から取り込めるのは画像・PDF・動画・音声だけです: outside.md")
Index::rel_of | symlinkで外 | Some("out-link.md")
resolve_entry | symlinkで中 | Ok(("a.pdf", "<B>/home/Documents/vault/a.pdf"))
resolve_entry(dir) | symlinkで中 | Ok(("a.pdf", "<B>/home/Documents/vault/a.pdf"))
host_path_in_vault | symlinkで中 | Ok("<B>/home/Documents/vault/a.pdf")
target_path | symlinkで中 | Err("入力を解釈できません: symlink には書けません: <B>/home/Documents/vault/in-link.pdf")
reject_symlink | symlinkで中 | Err("入力を解釈できません: symlink は扱えません: <B>/home/Documents/vault/in-link.pdf")
import_file | symlinkで中 | Ok("{\"copied\":false,\"embed\":\"![[a.pdf]]\",\"path\":\"a.pdf\"}")
Index::rel_of | symlinkで中 | Some("in-link.pdf")
resolve_entry | symlinkで隠し | Err("入力を解釈できません: このパスは読めません: <B>/home/Documents/vault/vis-link.md")
resolve_entry(dir) | symlinkで隠し | Err("入力を解釈できません: このパスは読めません: <B>/home/Documents/vault/vis-link.md")
host_path_in_vault | symlinkで隠し | Err("入力を解釈できません: `.` で始まるフォルダ・ファイルの中は使えません: .obsidian/x.md")
target_path | symlinkで隠し | Err("入力を解釈できません: symlink には書けません: <B>/home/Documents/vault/vis-link.md")
reject_symlink | symlinkで隠し | Err("入力を解釈できません: symlink は扱えません: <B>/home/Documents/vault/vis-link.md")
import_file | symlinkで隠し | Err("入力を解釈できません: ワークスペースの外から取り込めるのは画像・PDF・動画・音声だけです: x.md")
Index::rel_of | symlinkで隠し | Some("vis-link.md")
resolve_entry | 外のsymlinkで中 | Err("入力を解釈できません: ワークスペースの外のパスは読めません: <B>/to-in.pdf")
resolve_entry(dir) | 外のsymlinkで中 | Err("入力を解釈できません: ワークスペースの外のパスは読めません: <B>/to-in.pdf")
host_path_in_vault | 外のsymlinkで中 | Ok("<B>/home/Documents/vault/a.pdf")
target_path | 外のsymlinkで中 | Err("入力を解釈できません: ワークスペースの外には書けません: <B>/to-in.pdf")
reject_symlink | 外のsymlinkで中 | Ok(())
import_file | 外のsymlinkで中 | Ok("{\"copied\":false,\"embed\":\"![[a.pdf]]\",\"path\":\"a.pdf\"}")
Index::rel_of | 外のsymlinkで中 | Some("a.pdf")
resolve_entry | ..入り(相対) | Err("入力を解釈できません: パスに `..`・`.` 始まりの区画・root は使えません: sub/../a.pdf")
resolve_entry(dir) | ..入り(相対) | Err("入力を解釈できません: パスに `..`・`.` 始まりの区画・root は使えません: sub/../a.pdf")
host_path_in_vault | ..入り(相対) | Err("入力を解釈できません: パスに `..` は使えません: sub/../a.pdf")
target_path | ..入り(相対) | Err("入力を解釈できません: パスに `..`・root は使えません: sub/../a.pdf")
reject_symlink | ..入り(相対) | Ok(())
import_file | ..入り(相対) | Err("入力を解釈できません: wiki_import_file: abs は絶対パスで渡してください: sub/../a.pdf")
Index::rel_of | ..入り(相対) | Some("a.pdf")
resolve_entry | ..入り(絶対) | Err("入力を解釈できません: パスに `..`・`.` 始まりの区画・root は使えません: <B>/home/Documents/vault/sub/../a.pdf")
resolve_entry(dir) | ..入り(絶対) | Err("入力を解釈できません: パスに `..`・`.` 始まりの区画・root は使えません: <B>/home/Documents/vault/sub/../a.pdf")
host_path_in_vault | ..入り(絶対) | Ok("<B>/home/Documents/vault/a.pdf")
target_path | ..入り(絶対) | Err("入力を解釈できません: パスに `..`・root は使えません: <B>/home/Documents/vault/sub/../a.pdf")
reject_symlink | ..入り(絶対) | Ok(())
import_file | ..入り(絶対) | Ok("{\"copied\":false,\"embed\":\"![[a.pdf]]\",\"path\":\"a.pdf\"}")
Index::rel_of | ..入り(絶対) | Some("a.pdf")
resolve_entry | ..で外(相対) | Err("入力を解釈できません: パスに `..`・`.` 始まりの区画・root は使えません: ../../../outside.md")
resolve_entry(dir) | ..で外(相対) | Err("入力を解釈できません: パスに `..`・`.` 始まりの区画・root は使えません: ../../../outside.md")
host_path_in_vault | ..で外(相対) | Err("入力を解釈できません: パスに `..` は使えません: ../../../outside.md")
target_path | ..で外(相対) | Err("入力を解釈できません: パスに `..`・root は使えません: ../../../outside.md")
reject_symlink | ..で外(相対) | Ok(())
import_file | ..で外(相対) | Err("入力を解釈できません: wiki_import_file: abs は絶対パスで渡してください: ../../../outside.md")
Index::rel_of | ..で外(相対) | None
resolve_entry | ./入り(相対) | Ok(("a.pdf", "<B>/home/Documents/vault/a.pdf"))
resolve_entry(dir) | ./入り(相対) | Ok(("a.pdf", "<B>/home/Documents/vault/a.pdf"))
host_path_in_vault | ./入り(相対) | Ok("<B>/home/Documents/vault/a.pdf")
target_path | ./入り(相対) | Ok(("a.pdf", "<B>/home/Documents/vault/a.pdf", Some(("a.pdf", "<B>/home/Documents/vault/a.pdf"))))
reject_symlink | ./入り(相対) | Ok(())
import_file | ./入り(相対) | Err("入力を解釈できません: wiki_import_file: abs は絶対パスで渡してください: ./a.pdf")
Index::rel_of | ./入り(相対) | Some("a.pdf")
resolve_entry | 空白付き(相対) | Ok(("a.pdf", "<B>/home/Documents/vault/a.pdf"))
resolve_entry(dir) | 空白付き(相対) | Ok(("a.pdf", "<B>/home/Documents/vault/a.pdf"))
host_path_in_vault | 空白付き(相対) | Err("入力を解釈できません:  a.pdf を開けません: No such file or directory (os error 2)")
target_path | 空白付き(相対) | Ok(("a.pdf", "<B>/home/Documents/vault/a.pdf", Some(("a.pdf", "<B>/home/Documents/vault/a.pdf"))))
reject_symlink | 空白付き(相対) | Ok(())
import_file | 空白付き(相対) | Err("入力を解釈できません: wiki_import_file: abs は絶対パスで渡してください:  a.pdf")
Index::rel_of | 空白付き(相対) | Some(" a.pdf")
resolve_entry | 存在しない | Err("not found: ワークスペースのファイル <B>/home/Documents/vault/nope.pdf")
resolve_entry(dir) | 存在しない | Err("not found: ワークスペースのファイル <B>/home/Documents/vault/nope.pdf")
host_path_in_vault | 存在しない | Err("入力を解釈できません: <B>/home/Documents/vault/nope.pdf を開けません: No such file or directory (os error 2)")
target_path | 存在しない | Ok(("nope.pdf", "<B>/home/Documents/vault/nope.pdf", None))
reject_symlink | 存在しない | Ok(())
import_file | 存在しない | Err("not found: ファイル <B>/home/Documents/vault/nope.pdf")
Index::rel_of | 存在しない | Some("nope.pdf")
resolve_entry | dir | Err("入力を解釈できません: ファイルではありません: <B>/home/Documents/vault/sub")
resolve_entry(dir) | dir | Ok(("sub", "<B>/home/Documents/vault/sub"))
host_path_in_vault | dir | Err("入力を解釈できません: 通常のファイルではありません: sub")
target_path | dir | Err("入力を解釈できません: 同じ名前のフォルダがあります: <B>/home/Documents/vault/sub")
reject_symlink | dir | Ok(())
import_file | dir | Err("入力を解釈できません: ファイルではありません（フォルダは取り込めません）: <B>/home/Documents/vault/sub")
Index::rel_of | dir | Some("sub")
resolve_entry | 末尾/(絶対) | Err("入力を解釈できません: ファイルではありません: <B>/home/Documents/vault/sub/")
resolve_entry(dir) | 末尾/(絶対) | Ok(("sub", "<B>/home/Documents/vault/sub"))
host_path_in_vault | 末尾/(絶対) | Err("入力を解釈できません: 通常のファイルではありません: sub")
target_path | 末尾/(絶対) | Err("入力を解釈できません: 同じ名前のフォルダがあります: <B>/home/Documents/vault/sub/")
reject_symlink | 末尾/(絶対) | Ok(())
import_file | 末尾/(絶対) | Err("入力を解釈できません: ファイルではありません（フォルダは取り込めません）: <B>/home/Documents/vault/sub/")
Index::rel_of | 末尾/(絶対) | Some("sub")
resolve_entry | 末尾/(相対) | Err("入力を解釈できません: ファイルではありません: sub/")
resolve_entry(dir) | 末尾/(相対) | Ok(("sub", "<B>/home/Documents/vault/sub"))
host_path_in_vault | 末尾/(相対) | Err("入力を解釈できません: 通常のファイルではありません: sub")
target_path | 末尾/(相対) | Err("入力を解釈できません: 同じ名前のフォルダがあります: sub/")
reject_symlink | 末尾/(相対) | Ok(())
import_file | 末尾/(相対) | Err("入力を解釈できません: wiki_import_file: abs は絶対パスで渡してください: sub/")
Index::rel_of | 末尾/(相対) | Some("sub")
resolve_entry | root | Err("入力を解釈できません: このパスは読めません: <B>/home/Documents/vault")
resolve_entry(dir) | root | Err("入力を解釈できません: このパスは読めません: <B>/home/Documents/vault")
host_path_in_vault | root | Err("入力を解釈できません: ワークスペース（<B>/home/Documents/vault）の外です: <B>/home/Documents/vault")
target_path | root | Err("入力を解釈できません: ファイル名がありません: <B>/home/Documents/vault")
reject_symlink | root | Ok(())
import_file | root | Err("入力を解釈できません: ファイルではありません（フォルダは取り込めません）: <B>/home/Documents/vault")
Index::rel_of | root | None
"#;
}
