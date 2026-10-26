//! データの置き場（`~/Library/Application Support/OpenRef/openref.sqlite`）の解決。CLI / MCP / connectd / app の 4 つが
//! DB を開く前に [`db_path`] を呼ぶ。ディレクトリと DB 本体は呼び手の `Db::open` が作る（ここでは何も作らない）。
//!
//! 改名前（Refract）からの自動移行（rename・backup・attachments.path の書き換え・途中再開・使用中の検出）はここでは行わない。
//! 旧 `…/Refract/refract.sqlite` → `…/OpenRef/openref.sqlite` の移し替えは配布の
//! `site/install.sh`（shell + sqlite3）が行う。ここにあるのはガード 1 つだけ:
//! **旧名の DB ファイルが残っている間は、新しい空の DB を作らせない**（移し替えを終えていない蔵書の横に空の蔵書を作ると、
//! それが正と誤認される = 旧バイナリが空の DB を作る事故と同型）。

use crate::{CoreError, Result};
use std::path::{Path, PathBuf};

pub const DIR_NAME: &str = "OpenRef";
pub const DB_FILE: &str = "openref.sqlite";

// 改名前の名前（ガードのためだけに残す）
const OLD_DIR_NAME: &str = "Refract";
const OLD_DB_FILE: &str = "refract.sqlite";

/// DB パス。`OPENREF_DB` が設定されていればそのまま返す（ガードもしない）。
/// 無ければ `$HOME/Library/Application Support` の下の既定パス
pub fn db_path() -> Result<PathBuf> {
    if let Some(p) = crate::env_os("DB") {
        return Ok(PathBuf::from(p));
    }
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty()).ok_or_else(|| {
        CoreError::DataDir("HOME が未設定のため既定の DB パスを決められません（OPENREF_DB で指定してください）".into())
    })?;
    prepare(&PathBuf::from(home).join("Library/Application Support"))
}

/// `base`（= ~/Library/Application Support）の下の DB パスを返す。何も作らない。
/// 新 DB が無いのに旧名の DB（`…/Refract/refract.sqlite`、または移し替え途中の `…/OpenRef/refract.sqlite`）があれば
/// エラーで止める（空の DB を作らせない）。新 DB があれば旧ディレクトリが残っていても新を使う。テストは tempdir を `base` に渡す
pub fn prepare(base: &Path) -> Result<PathBuf> {
    let old_dir = base.join(OLD_DIR_NAME);
    let new_dir = base.join(DIR_NAME);
    let db = new_dir.join(DB_FILE);
    if !db.exists() {
        // DB 本体だけでなく -wal も見る（本体より先に改名する手順なので、-wal だけが旧名で残るのは途中で止まった印）
        let wal = format!("{OLD_DB_FILE}-wal");
        for legacy in [old_dir.join(OLD_DB_FILE), old_dir.join(&wal), new_dir.join(OLD_DB_FILE), new_dir.join(&wal)] {
            if legacy.exists() {
                return Err(CoreError::DataDir(format!(
                    "改名前の蔵書 {legacy} があり、{db} はまだありません。空の蔵書を作らないよう起動を止めました。\
                     移し替えは配布の install.sh が行います（手でやるなら: 旧版を止めて {old} を {new} に rename し、\
                     refract.sqlite-wal / -shm・certs/refract-localhost*.pem を openref.* に改名してから最後に refract.sqlite を openref.sqlite に改名し、\
                     attachments.path の {old}/ を {new}/ に書き換える）",
                    legacy = legacy.display(),
                    db = db.display(),
                    old = old_dir.display(),
                    new = new_dir.display(),
                )));
            }
        }
    }
    Ok(db)
}

#[cfg(test)]
mod tests {
    use super::prepare;
    use crate::Db;
    use std::path::{Path, PathBuf};

    fn base(name: &str) -> PathBuf {
        let b = std::env::temp_dir().join(format!("openref-datadir-{}-{}-{name}", std::process::id(), fastrand::u64(..)));
        std::fs::create_dir_all(&b).unwrap();
        b
    }

    fn touch(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"x").unwrap();
    }

    #[test]
    fn fresh_install_creates_nothing_until_open() {
        let b = base("fresh");
        let p = prepare(&b).unwrap();
        assert_eq!(p, b.join("OpenRef/openref.sqlite"));
        assert!(!b.join("OpenRef").exists() && !b.join("Refract").exists());
        Db::open(&p).unwrap();
        assert_eq!(prepare(&b).unwrap(), p);
    }

    #[test]
    fn never_creates_empty_db_while_legacy_db_exists() {
        // 旧ディレクトリに旧名の DB
        let b = base("legacy-old");
        touch(&b.join("Refract/refract.sqlite"));
        let e = prepare(&b).unwrap_err().to_string();
        assert!(e.contains("refract.sqlite") && e.contains("install.sh"), "{e}");
        assert!(!b.join("OpenRef").exists(), "何も作らない");
        // 移し替え途中（新ディレクトリに旧名の DB だけ）
        let b = base("legacy-half");
        touch(&b.join("OpenRef/refract.sqlite"));
        assert!(prepare(&b).is_err());
        assert!(!b.join("OpenRef/openref.sqlite").exists());
    }

    #[test]
    fn new_db_wins_even_if_old_dir_remains() {
        let b = base("both");
        touch(&b.join("Refract/refract.sqlite"));
        let p = b.join("OpenRef/openref.sqlite");
        Db::open(&p).unwrap();
        assert_eq!(prepare(&b).unwrap(), p);
    }

    #[test]
    fn env_empty_value_counts_as_unset() {
        std::env::set_var("OPENREF_DATADIR_TEST_Y", "");
        assert_eq!(crate::env_os("DATADIR_TEST_Y"), None);
        std::env::set_var("OPENREF_DATADIR_TEST_Y", "v");
        assert_eq!(crate::env_os("DATADIR_TEST_Y").as_deref(), Some(std::ffi::OsStr::new("v")));
        std::env::remove_var("OPENREF_DATADIR_TEST_Y");
        // 旧名 REFRACT_* は読まない
        std::env::set_var("REFRACT_DATADIR_TEST_Y", "old");
        assert_eq!(crate::env_os("DATADIR_TEST_Y"), None);
        std::env::remove_var("REFRACT_DATADIR_TEST_Y");
    }
}
