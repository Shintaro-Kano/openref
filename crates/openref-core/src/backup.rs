//! DB のスナップショット（安全網）。MCP はプロセスで最初の書込コマンドの前に 1 個取る。
//! `<app_dir>/backup/<prefix><YYYYMMDD-HHMMSS>.sqlite` へ `VACUUM INTO`（WAL の未チェックポイント分も含む一貫した写し）。
//! 同じ prefix で時刻の名前のもの（`<prefix>YYYYMMDD-HHMMSS[-n].sqlite`）だけを新しい順に残す（他の `pre-rename-*` `v1-no-ai-*`・
//! 手で置いた名前には触らない）。
//! 戻すときは書き手（app・connectd・他の MCP）を止めて `sqlite3 <db> ".restore '<写し>'"`（backup API）。**cp で本体を置き換えない**:
//! `-wal` が残っているうちに置き換えると壊れる（他のプロセスが開いている間は `-wal` が残るのが常態。手順は MCP の schema §4）。

use crate::{CoreError, Db, Result};
use std::path::{Path, PathBuf};

/// エージェント（MCP）の書込の前のスナップショット
pub const AGENT_PREFIX: &str = "pre-agent-";
/// 残す個数（新しい順）
pub const AGENT_KEEP: usize = 5;

/// `<app_dir>/backup`（in-memory は None）
pub fn backup_dir(db: &Db) -> Option<PathBuf> {
    Some(db.app_dir()?.join("backup"))
}

/// スナップショットを取り、同じ prefix の古いものを keep 個まで減らす。in-memory は Ok(None)。
/// 取れなければ Err（呼び手は書込を実行しない = fail-closed）。古いものの削除の失敗は stderr に出すだけ（写しは取れている）
pub fn snapshot(db: &Db, prefix: &str, keep: usize) -> Result<Option<PathBuf>> {
    let Some(dir) = backup_dir(db) else { return Ok(None) };
    std::fs::create_dir_all(&dir)?;
    // 名前の時刻は端末の現地時刻（既存の pre-rename-* などと同じ読み方）
    let ts: String = db
        .conn
        .query_row("SELECT strftime('%Y%m%d-%H%M%S', 'now', 'localtime')", [], |r| r.get(0))?;
    // 名前は create_new で先に確保する（同じ秒に別のプロセスも取ると、存在の確認から VACUUM INTO までの間に
    // 同じ名前へ書き合って片方が失敗する）。取られていたら -<pid> を付けて 1 回だけ取り直す。VACUUM INTO は空のファイルへ書ける
    let mut dest = dir.join(format!("{prefix}{ts}.sqlite"));
    if let Err(e) = reserve(&dest) {
        if e.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(e.into());
        }
        dest = dir.join(format!("{prefix}{ts}-{}.sqlite", std::process::id()));
        reserve(&dest).map_err(|e| {
            CoreError::Conflict(format!("スナップショットの名前が衝突しました（{}）: {e}", dest.display()))
        })?;
    }
    if let Err(e) = db.conn.execute("VACUUM INTO ?1", [dest.to_string_lossy()]) {
        let _ = std::fs::remove_file(&dest); // 確保した空のファイルを残さない
        return Err(e.into());
    }
    for (p, e) in prune(&dir, prefix, keep) {
        eprintln!("[openref] 古いスナップショットを消せません {}: {e}", p.display());
    }
    Ok(Some(dest))
}

/// 空のファイルを新しく作って名前を確保する（既にあれば AlreadyExists）
fn reserve(p: &Path) -> std::io::Result<()> {
    std::fs::OpenOptions::new().write(true).create_new(true).open(p).map(|_| ())
}

/// dir の `<prefix>YYYYMMDD-HHMMSS[-n].sqlite` を名前（= 時刻）の新しい順に keep 個残して消す。時刻でない名前
/// （`pre-agent-manual.sqlite` 等）は数えも消しもしない。戻り値 = 消せなかったもの
pub fn prune(dir: &Path, prefix: &str, keep: usize) -> Vec<(PathBuf, std::io::Error)> {
    let pat = regex::Regex::new(&format!(r"^{}(\d{{8}}-\d{{6}}(?:-\d+)?)\.sqlite$", regex::escape(prefix)))
        .expect("固定の正規表現");
    let mut snaps: Vec<(String, PathBuf)> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                let stem = pat.captures(&name)?[1].to_string();
                Some((stem, e.path()))
            })
            .collect(),
        Err(e) => return vec![(dir.to_path_buf(), e)],
    };
    // 時刻の文字列は固定長なので名前順 = 時刻順（同じ秒の "-<pid>" は元の名前より後 = 新しい）
    snaps.sort_by(|a, b| b.0.cmp(&a.0));
    snaps
        .into_iter()
        .skip(keep)
        .filter_map(|(_, p)| std::fs::remove_file(&p).err().map(|e| (p, e)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_keeps_newest_and_leaves_other_backups() {
        let dir = std::env::temp_dir().join(format!("openref-backup-{}-{}", std::process::id(), crate::keys::new_key()));
        let db = Db::open(&dir.join("openref.sqlite")).unwrap();
        db.conn.execute("INSERT INTO items (key, csl) VALUES ('K1', '{\"title\":\"t\"}')", []).unwrap();
        let bdir = dir.join("backup");
        std::fs::create_dir_all(&bdir).unwrap();
        // 古いスナップショット 5 個と、触ってはいけない他の写し
        for i in 0..5 {
            std::fs::write(bdir.join(format!("pre-agent-2000010{i}-000000.sqlite")), b"old").unwrap();
        }
        std::fs::write(bdir.join("pre-rename-20261020-041939.sqlite"), b"keep").unwrap();
        std::fs::write(bdir.join("v1-no-ai-20261021.sqlite"), b"keep").unwrap();
        // 時刻でない名前は数えも消しもしない（最新扱いで残り続けない）
        std::fs::write(bdir.join("pre-agent-manual.sqlite"), b"keep").unwrap();

        let p = snapshot(&db, AGENT_PREFIX, AGENT_KEEP).unwrap().unwrap();
        assert!(p.starts_with(&bdir));
        // 写しは開けて中身がある
        let c = rusqlite::Connection::open(&p).unwrap();
        let n: i64 = c.query_row("SELECT count(*) FROM items", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        let mut names: Vec<String> =
            std::fs::read_dir(&bdir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
        names.sort();
        let agent: Vec<&String> = names.iter().filter(|n| n.starts_with("pre-agent-2")).collect();
        assert_eq!(agent.len(), 5, "{names:?}");
        assert!(!names.iter().any(|n| n == "pre-agent-20000100-000000.sqlite"), "最古が消える: {names:?}");
        assert!(names.iter().any(|n| n == "pre-rename-20261020-041939.sqlite"));
        assert!(names.iter().any(|n| n == "v1-no-ai-20261021.sqlite"));
        assert!(names.iter().any(|n| n == "pre-agent-manual.sqlite"));

        // 同じ秒にもう 1 個取っても上書きしない（-<pid> が付く）
        let p2 = snapshot(&db, AGENT_PREFIX, AGENT_KEEP).unwrap().unwrap();
        assert_ne!(p, p2);
        assert!(p.exists() && p2.exists());
        drop(c);
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 同じ秒の名前を別のプロセスが先に取っていたら -<pid> に逃げる（相手の書きかけを上書きしない）。
    /// それも取られていたら名前の衝突として Err（書込はしない = fail-closed）で、空のファイルを残さない
    #[test]
    fn snapshot_name_collision() {
        let dir = std::env::temp_dir().join(format!("openref-backup-col-{}-{}", std::process::id(), crate::keys::new_key()));
        let db = Db::open(&dir.join("openref.sqlite")).unwrap();
        let bdir = dir.join("backup");
        std::fs::create_dir_all(&bdir).unwrap();
        // 秒の境目をまたいでも当たるよう、今から 3 秒分の名前を「別のプロセスが書きかけ」にしておく
        let stamps: Vec<String> = (0..3)
            .map(|i| {
                db.conn
                    .query_row(&format!("SELECT strftime('%Y%m%d-%H%M%S', 'now', '+{i} seconds', 'localtime')"), [], |r| r.get(0))
                    .unwrap()
            })
            .collect();
        for ts in &stamps {
            std::fs::write(bdir.join(format!("{AGENT_PREFIX}{ts}.sqlite")), b"other process").unwrap();
        }
        let p = snapshot(&db, AGENT_PREFIX, AGENT_KEEP).unwrap().unwrap();
        assert!(p.to_string_lossy().ends_with(&format!("-{}.sqlite", std::process::id())), "{}", p.display());
        rusqlite::Connection::open(&p).unwrap().query_row("SELECT count(*) FROM items", [], |r| r.get::<_, i64>(0)).unwrap();
        for ts in &stamps {
            assert_eq!(std::fs::read(bdir.join(format!("{AGENT_PREFIX}{ts}.sqlite"))).unwrap(), b"other process");
        }
        // -<pid> も取られている → 衝突のエラー
        for ts in &stamps {
            std::fs::write(bdir.join(format!("{AGENT_PREFIX}{ts}-{}.sqlite", std::process::id())), b"taken").unwrap();
        }
        let n_before = std::fs::read_dir(&bdir).unwrap().count();
        let e = snapshot(&db, AGENT_PREFIX, AGENT_KEEP).unwrap_err().to_string();
        assert!(e.contains("名前が衝突"), "{e}");
        assert_eq!(std::fs::read_dir(&bdir).unwrap().count(), n_before, "空のファイルを残さない");
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn in_memory_has_no_snapshot() {
        let db = Db::open_in_memory().unwrap();
        assert!(snapshot(&db, AGENT_PREFIX, AGENT_KEEP).unwrap().is_none());
    }
}
