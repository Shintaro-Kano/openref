//! DB のスナップショット（安全網）。MCP はプロセスで最初の書込コマンドの前に 1 個取る。
//! `<app_dir>/backup/<prefix><YYYYMMDD-HHMMSS>.sqlite` へ `VACUUM INTO`（WAL の未チェックポイント分も含む一貫した写し）。
//! 同じ prefix で時刻の名前のもの（`<prefix>YYYYMMDD-HHMMSS[-pid[-nn]].sqlite`）だけを新しい順に残す（他の `pre-rename-*` `v1-no-ai-*`・
//! 手で置いた名前には触らない）。
//! 戻すときは書き手（app・connectd・他の MCP）を止めて `sqlite3 <db> ".restore '<写し>'"`（backup API）。**cp で本体を置き換えない**:
//! `-wal` が残っているうちに置き換えると壊れる（他のプロセスが開いている間は `-wal` が残るのが常態。手順は MCP の schema §4）。

use crate::{CoreError, Db, Result};
use std::path::{Path, PathBuf};

/// エージェント（MCP）の書込の前のスナップショット
pub const AGENT_PREFIX: &str = "pre-agent-";
/// 残す個数（新しい順）
pub const AGENT_KEEP: usize = 5;
/// ライブラリ移行の開始前のスナップショット（実行との対応は import_runs.backup_path が持つ）
pub const IMPORT_PREFIX: &str = "pre-import-";
/// 残す個数（新しい順。古い実行の backup_path は消えたファイルを指しうる）
pub const IMPORT_KEEP: usize = 3;

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
    // 同じ名前へ書き合って片方が失敗する）。取られていたら -<pid>、それも取られていたら（同じプロセスが同じ秒に
    // 3 個目 = 移行の再試行が続いたとき）-<pid>-01〜99 で取り直す（2 桁 = 名前順が時刻順のまま）。VACUUM INTO は空のファイルへ書ける。
    // この秒にこのプロセスが付けた名前（剪定で消えたものを含む）より前の名前は使わない（再利用すると名前順 = 時刻順が崩れ、
    // 剪定が今取った写しを消す）。剪定は名前の最も新しいものを消さないので、残っている最大の名前より後ろを取れば足りる
    let pid = std::process::id();
    let own = format!("{prefix}{ts}-{pid}");
    let latest = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str()?.strip_suffix(".sqlite").map(str::to_string))
        .filter(|n| {
            *n == format!("{prefix}{ts}") || *n == own || n.starts_with(&format!("{own}-"))
        })
        .map(|n| n[prefix.len()..].to_string())
        .max();
    let names = std::iter::once(ts.clone())
        .chain(std::iter::once(format!("{ts}-{pid}")))
        .chain((1..100).map(|n| format!("{ts}-{pid}-{n:02}")))
        .filter(|n| latest.as_ref().is_none_or(|l| n > l));
    let mut dest = None;
    for name in names {
        let p = dir.join(format!("{prefix}{name}.sqlite"));
        match reserve(&p) {
            Ok(()) => {
                dest = Some(p);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    let dest = dest.ok_or_else(|| {
        CoreError::Conflict(format!("スナップショットの名前が衝突しました（{prefix}{ts}）"))
    })?;
    if let Err(e) = db.conn.execute("VACUUM INTO ?1", [dest.to_string_lossy()]) {
        let _ = std::fs::remove_file(&dest); // 確保した空のファイルを残さない
        return Err(e.into());
    }
    for (p, e) in prune(&dir, prefix, keep, &dest) {
        eprintln!("[openref] 古いスナップショットを消せません {}: {e}", p.display());
    }
    // 今取った写しが残っていることを確かめてから返す（呼び手は backup_path として記録する）
    if !dest.is_file() {
        return Err(CoreError::Conflict(format!(
            "取ったスナップショットが見つかりません: {}",
            dest.display()
        )));
    }
    Ok(Some(dest))
}

/// 空のファイルを新しく作って名前を確保する（既にあれば AlreadyExists）
fn reserve(p: &Path) -> std::io::Result<()> {
    std::fs::OpenOptions::new().write(true).create_new(true).open(p).map(|_| ())
}

/// dir の `<prefix>YYYYMMDD-HHMMSS[-pid[-nn]].sqlite` を名前（= 時刻）の新しい順に keep 個残して消す。時刻でない名前
/// （`pre-agent-manual.sqlite` 等）は数えも消しもしない。`taken`（今取った写し）は名前順に依らず必ず残し、keep 個の 1 つに数える
/// （時計の巻き戻し・TZ の変更・同じ秒の別プロセスの `-<pid>` 系の名前で、今取った写しが名前順で最古になっても消さない）。
/// 戻り値 = 消せなかったもの
pub fn prune(dir: &Path, prefix: &str, keep: usize, taken: &Path) -> Vec<(PathBuf, std::io::Error)> {
    let pat = regex::Regex::new(&format!(r"^{}(\d{{8}}-\d{{6}}(?:-\d+){{0,2}})\.sqlite$", regex::escape(prefix)))
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
    // 時刻の文字列は固定長なので名前順 = 時刻順（同じ秒の "-<pid>" は元の名前より後・"-<pid>-nn" はさらに後 = 新しい）
    snaps.sort_by(|a, b| b.0.cmp(&a.0));
    let before = snaps.len();
    snaps.retain(|(_, p)| p != taken);
    let keep = if snaps.len() < before { keep.saturating_sub(1) } else { keep };
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
    /// それも取られていたら -<pid>-01〜99。全部取られていたら名前の衝突として Err（書込はしない = fail-closed）で、空のファイルを残さない
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
        // -<pid> も取られている（同じプロセスが同じ秒に 3 個目 = 移行の再試行）→ -<pid>-01
        // （ここから先は剪定が仕掛けた名前を消さないよう keep を大きく）
        let pid = std::process::id();
        for ts in &stamps {
            std::fs::write(bdir.join(format!("{AGENT_PREFIX}{ts}-{pid}.sqlite")), b"taken").unwrap();
        }
        let p = snapshot(&db, AGENT_PREFIX, 10_000).unwrap().unwrap();
        assert!(p.to_string_lossy().ends_with(&format!("-{pid}-01.sqlite")), "{}", p.display());
        // -<pid>-01〜99 まで全部取られている → 衝突のエラー
        for ts in &stamps {
            for n in 1..100 {
                let _ = std::fs::write(bdir.join(format!("{AGENT_PREFIX}{ts}-{pid}-{n:02}.sqlite")), b"taken");
            }
        }
        let n_before = std::fs::read_dir(&bdir).unwrap().count();
        let e = snapshot(&db, AGENT_PREFIX, 10_000).unwrap_err().to_string();
        assert!(e.contains("名前が衝突"), "{e}");
        assert_eq!(std::fs::read_dir(&bdir).unwrap().count(), n_before, "空のファイルを残さない");
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 今取った写しが名前順で最古になっても剪定で消さない（時計の巻き戻し = 未来の日時の写し 3 本 /
    /// 同じ秒の別プロセスの `-1`・`-1-01` 系の名前）。返す path は実在し、全体は keep 個に収まる
    #[test]
    fn prune_never_deletes_the_snapshot_just_taken() {
        let dir = std::env::temp_dir().join(format!("openref-backup-taken-{}-{}", std::process::id(), crate::keys::new_key()));
        let db = Db::open(&dir.join("openref.sqlite")).unwrap();
        let bdir = dir.join("backup");
        std::fs::create_dir_all(&bdir).unwrap();
        let count = |prefix: &str| std::fs::read_dir(&bdir).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().starts_with(prefix)).count();
        // 巻き戻し: 未来の日時の写しが 3 本ある
        for name in ["29990101-000001", "29990101-000002", "29990101-000003"] {
            std::fs::write(bdir.join(format!("{IMPORT_PREFIX}{name}.sqlite")), b"future").unwrap();
        }
        let p = snapshot(&db, IMPORT_PREFIX, IMPORT_KEEP).unwrap().unwrap();
        assert!(p.is_file(), "{}", p.display());
        assert_eq!(count(IMPORT_PREFIX), IMPORT_KEEP);
        assert!(!bdir.join(format!("{IMPORT_PREFIX}29990101-000001.sqlite")).exists(), "未来の名前の最古を代わりに消す");
        // 同じ秒の別プロセス（pid 1）の名前が今の名前より後ろに並ぶ
        let ts: String = db.conn.query_row("SELECT strftime('%Y%m%d-%H%M%S', 'now', 'localtime')", [], |r| r.get(0)).unwrap();
        for i in 0..3 {
            let suffix = if i == 0 { String::new() } else { format!("-{i:02}") };
            for s in ["+1 seconds", "+2 seconds"] {
                let t: String = db.conn.query_row(&format!("SELECT strftime('%Y%m%d-%H%M%S', 'now', '{s}', 'localtime')"), [], |r| r.get(0)).unwrap();
                std::fs::write(bdir.join(format!("{AGENT_PREFIX}{t}-1{suffix}.sqlite")), b"other").unwrap();
            }
            std::fs::write(bdir.join(format!("{AGENT_PREFIX}{ts}-1{suffix}.sqlite")), b"other").unwrap();
        }
        let p = snapshot(&db, AGENT_PREFIX, AGENT_KEEP).unwrap().unwrap();
        assert!(p.is_file(), "{}", p.display());
        rusqlite::Connection::open(&p).unwrap().query_row("SELECT count(*) FROM items", [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(count(AGENT_PREFIX), AGENT_KEEP);
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn in_memory_has_no_snapshot() {
        let db = Db::open_in_memory().unwrap();
        assert!(snapshot(&db, AGENT_PREFIX, AGENT_KEEP).unwrap().is_none());
    }
}
