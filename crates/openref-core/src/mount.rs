//! Paperpile 互換の Google Drive mount 検出と cutover（Drive for Desktop 前提）。

use crate::{drive_layout, CoreError, Db, Result};
use std::ops::Deref;
use std::path::{Path, PathBuf};

/// Drive for Desktop の根 = 存在する `~/Library/CloudStorage/GoogleDrive-*/{マイドライブ,My Drive}`（列挙の順）。
fn drive_roots() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(home.join("Library/CloudStorage")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("GoogleDrive-"))
        .flat_map(|e| ["マイドライブ", "My Drive"].map(|dd| e.path().join(dd)))
        .filter(|root| root.is_dir())
        .collect()
}

/// `~/Library/CloudStorage/GoogleDrive-*/{マイドライブ,My Drive}/{Paperpile,OpenRef}` を探す（読むだけ・何も作らない）。
/// Paperpile からの移行ユーザーの既存環境を壊さないよう `Paperpile` を先に探し、無ければ `OpenRef` を探す。
/// `OPENREF_MOUNT` が設定されていればそれを優先（検証・将来の配置替え用）。
pub fn detect() -> Option<PathBuf> {
    if let Some(p) = crate::env_os("MOUNT").map(PathBuf::from) {
        return p.is_dir().then_some(p);
    }
    drive_roots()
        .into_iter()
        .flat_map(|root| ["Paperpile", "OpenRef"].map(|folder| root.join(folder)))
        .find(|cand| cand.is_dir())
}

/// Drive モード有効化時: 既存の `detect()`（`Paperpile` または `OpenRef`）があればそれを返し、無ければ
/// `~/Library/CloudStorage/GoogleDrive-*/{マイドライブ,My Drive}` の下に `OpenRef/All Papers` を作成して返す
/// （`OPENREF_MOUNT` が設定されていればその下に `All Papers` を作る）。
/// Paperpile からの移行でない新規ユーザーは `OpenRef/All Papers` に PDF が保存される。
pub fn detect_or_create() -> Option<PathBuf> {
    if let Some(p) = detect() {
        return Some(p);
    }
    let candidates = match crate::env_os("MOUNT").map(PathBuf::from) {
        Some(p) => vec![p],
        None => drive_roots().into_iter().map(|root| root.join("OpenRef")).collect(),
    };
    candidates.into_iter().find(|p| std::fs::create_dir_all(p.join("All Papers")).is_ok())
}

/// `attachments` の `mode` ごとの件数（`"managed"` / `"linked"`）。
pub fn count_attachments_by_mode(db: &Db, mode: &str) -> Result<i64> {
    Ok(db.conn.query_row("SELECT COUNT(*) FROM attachments WHERE mode = ?1", [mode], |r| r.get(0))?)
}

/// Google Drive モードへの切替結果
#[derive(Debug, Clone)]
pub struct EnabledDrive {
    pub mount: PathBuf,
    pub managed_remaining: i64,
}

/// Google Drive モードを有効化する（preflight + `pdf_write_mode = "drive"`）。
/// 1. Drive mount の検出（`Paperpile` / `OpenRef` フォルダが未作成でも Google Drive for Desktop があれば `OpenRef/All Papers` を自動作成。
///    既存の `Paperpile` に `All Papers` が無ければそれも作る）
/// 2. `All Papers` への書込プローブ（`.openref-write-probe`）
/// 3. `settings.pdf_write_mode = "drive"` の保存
///
/// `db` は DB を借りる関数（GUI = Mutex のロック `|| core_lock(&st.db)` / CLI = `|| Ok(db)`・db: &Db）。1・2 は Drive（File Provider）の応答待ちで
/// 秒単位になりうるのでロックの外で行い、ロックは 3 と件数の 1 回だけ取る。
pub fn enable_drive_mode<G: Deref<Target = Db>>(db: impl Fn() -> Result<G>) -> Result<EnabledDrive> {
    let m = detect_or_create().ok_or_else(|| {
        CoreError::BadInput(
            "Google Drive が見つかりません（Google Drive for Desktop の起動を確認してください。OPENREF_MOUNT で明示も可）"
                .into(),
        )
    })?;
    let all_papers = m.join("All Papers");
    std::fs::create_dir_all(&all_papers)?;
    let probe = all_papers.join(".openref-write-probe");
    std::fs::write(&probe, b"probe").map_err(|e| {
        CoreError::BadInput(format!("Google Drive への書込確認に失敗しました ({}): {e}", probe.display()))
    })?;
    std::fs::remove_file(&probe).ok();
    let db = db()?;
    // 件数を先に: 数えられないときに「失敗」を返しながら設定だけ drive に変わっている、を起こさない
    let managed_remaining = count_attachments_by_mode(&db, "managed")?;
    db.set_setting("pdf_write_mode", "drive")?;
    Ok(EnabledDrive {
        mount: m,
        managed_remaining,
    })
}

/// ローカル（managed）モードへ戻す（既存の linked 添付には影響しない）。
pub fn disable_drive_mode(db: &Db) -> Result<()> {
    db.set_setting("pdf_write_mode", "managed")
}

/// ローカル（managed）添付 1 件の Drive 移行結果
#[derive(Debug, Clone)]
pub struct MigratedAttachment {
    pub item_key: String,
    pub src_path: String,
    pub dest_path: PathBuf,
}

/// 移行の間に変わったので移さなかった添付 1 件
#[derive(Debug, Clone)]
pub struct SkippedAttachment {
    pub item_key: String,
    pub src_path: String,
    /// 移さなかった理由（表示用）
    pub reason: &'static str,
    /// 書いたのに消せなかったコピー（パスと理由）。誰も指さないファイルが Drive に残っている
    pub leftover: Option<(PathBuf, String)>,
}

/// 移行の結果: 移した添付（dry-run なら移す予定）と、途中で変わったので移さなかった添付
#[derive(Debug, Clone, Default)]
pub struct Migration {
    pub moved: Vec<MigratedAttachment>,
    pub skipped: Vec<SkippedAttachment>,
}

/// 実行中の本番の移行（プロセス内で 1 本）。drop で空く
static MIGRATING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct MigrationFlight;

impl MigrationFlight {
    fn take() -> Result<Self> {
        use std::sync::atomic::Ordering;
        MIGRATING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| MigrationFlight)
            .map_err(|_| CoreError::BadInput("Google Drive への移行が実行中です（終わってからもう一度試してください）".into()))
    }
}

impl Drop for MigrationFlight {
    fn drop(&mut self) {
        MIGRATING.store(false, std::sync::atomic::Ordering::Release);
    }
}

/// `mode = 'managed'` の添付 PDF をすべて Drive 互換レイアウトへコピーし、読み戻し照合の上で `mode = 'linked'` に切り替える。
/// 元の `storage/` ファイルは安全のため削除せず残す。
/// `dry_run` は何も書かない（Drive のフォルダも作らない）: 移行先が未作成なら Err（`cutover enable` か移行の本番で作られる）。
///
/// `db` は DB を借りる関数（GUI = Mutex のロック `|| core_lock(&st.db)` / CLI = `|| Ok(db)`・db: &Db）。ロックは一覧の読出しと 1 件ごとの
/// 確認・書換えの間だけ取り、原本の読込・Drive への書込・読み戻し（1 件で秒単位になりうる）はロックの外で行う。
/// ロックを離している間に添付が消された・置き換えられた（`mode` か `path` が変わった）ときは、その 1 件は書き換えず、
/// 書いたばかりのコピーを（それを指す行が無いことをロックの中で確かめてから）消し、`skipped` に入れる。
/// 本番の移行は 1 プロセスに同時に 1 本だけ（二度押し = 2 本目は「実行中」の Err をすぐ返す）。dry-run は数えない。
pub fn migrate_managed_to_drive<G: Deref<Target = Db>>(
    db: impl Fn() -> Result<G>,
    dry_run: bool,
) -> Result<Migration> {
    let _flight = if dry_run { None } else { Some(MigrationFlight::take()?) };
    let not_found = || CoreError::BadInput("Google Drive が見つかりません".into());
    let m = if dry_run {
        detect().ok_or_else(|| {
            // Google Drive 自体が無い（Drive の根も OPENREF_MOUNT も無い）なら従来の文言。根はあるが移行先が未作成なら、作らないことを言う
            if crate::env_os("MOUNT").is_none() && drive_roots().is_empty() {
                return not_found();
            }
            CoreError::BadInput(
                "Google Drive の移行先フォルダ（Paperpile / OpenRef）がまだありません（dry-run は作りません。`cutover enable` で作成できます）"
                    .into(),
            )
        })?
    } else {
        detect_or_create().ok_or_else(not_found)?
    };
    let rows: Vec<(i64, String, String, String)> = {
        let db = db()?;
        let mut stmt = db.conn.prepare(
            "SELECT a.id, a.path, i.key, i.csl FROM attachments a
             JOIN items i ON i.id = a.item_id WHERE a.mode = 'managed' ORDER BY a.id",
        )?;
        let r = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        r
    };
    let mut out = Migration::default();
    for (att_id, src_path, item_key, csl_s) in rows {
        let csl: serde_json::Value = serde_json::from_str(&csl_s)?;
        if dry_run {
            let (initial, filename) = drive_layout::relative_target(&csl);
            out.moved.push(MigratedAttachment {
                item_key,
                src_path,
                dest_path: m.join("All Papers").join(initial).join(filename),
            });
            continue;
        }
        crate::db::reject_connection_file(&db()?.conn, Path::new(&src_path))?;
        let bytes = match std::fs::read(&src_path) {
            Ok(b) => b,
            Err(e) => {
                // 読めないのが「一覧の後に添付が消された・置き換えられた」せいなら、その 1 件だけ移さない。
                // 行がそのままなら原本の欠落 = 従来どおり移行を止めて知らせる
                let still: bool = db()?.conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM attachments WHERE id = ?1 AND mode = 'managed' AND path = ?2)",
                    (att_id, &src_path),
                    |r| r.get(0),
                )?;
                if still {
                    return Err(CoreError::BadInput(format!("ローカルの原本を読めません ({src_path}): {e}")));
                }
                out.skipped.push(SkippedAttachment {
                    item_key,
                    src_path,
                    reason: "原本を読む前に添付が変わった（削除・置換）",
                    leftover: None,
                });
                continue;
            }
        };
        let dest = drive_layout::place_bytes(&m, &csl, &bytes)?;
        // 読み戻し照合（サイズ + 内容。Drive 書込の完全性を確認してから DB を書き換える）
        crate::db::reject_connection_file(&db()?.conn, &dest)?;
        let back = std::fs::read(&dest)?;
        if back != bytes {
            return Err(CoreError::BadInput(format!(
                "照合不一致: {} — DB は変更していません",
                dest.display()
            )));
        }
        let dest_s = dest.to_string_lossy().to_string();
        let skipped = {
            let db = db()?;
            let changed = db.conn.execute(
                "UPDATE attachments SET mode = 'linked', path = ?1 WHERE id = ?2 AND mode = 'managed' AND path = ?3",
                (&dest_s, att_id, &src_path),
            )?;
            if changed > 0 {
                None
            } else {
                // コピーの間に添付が変わった（削除・置換）→ 書いたコピーを指す行が無いことをロックの中で確かめてから消す
                let pointed: i64 =
                    db.conn.query_row("SELECT COUNT(*) FROM attachments WHERE path = ?1", [&dest_s], |r| r.get(0))?;
                let leftover = if pointed > 0 {
                    None
                } else {
                    std::fs::remove_file(&dest).err().map(|e| (dest.clone(), e.to_string()))
                };
                Some(leftover)
            }
        };
        match skipped {
            None => out.moved.push(MigratedAttachment {
                item_key,
                src_path,
                dest_path: dest,
            }),
            Some(leftover) => out.skipped.push(SkippedAttachment {
                item_key,
                src_path,
                reason: "コピーの間に添付が変わった（削除・置換）",
                leftover,
            }),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::sync::Mutex;

    /// `OPENREF_MOUNT` はプロセス共有 → この module の試験は 1 本ずつ（lib の他の試験は MOUNT を読まない）
    static ENV: Mutex<()> = Mutex::new(());

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("openref-mount-{tag}-{}-{}", std::process::id(), crate::keys::new_key()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// managed 添付 1 件（原本 = dir/src.pdf）の in-memory DB
    fn db_with_managed(dir: &Path) -> (Db, PathBuf) {
        let db = Db::open_in_memory().unwrap();
        let src = dir.join("src.pdf");
        std::fs::write(&src, b"%PDF-1.4 test").unwrap();
        db.conn
            .execute_batch(
                "INSERT INTO items (key, csl) VALUES ('ITEM0001', '{\"title\":\"Mount test\",\"type\":\"article-journal\",\"author\":[{\"family\":\"Kano\"}],\"issued\":{\"date-parts\":[[2026]]}}')",
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO attachments (key, item_id, kind, mode, path) VALUES ('ATT00001', 1, 'pdf', 'managed', ?1)",
                [src.to_string_lossy()],
            )
            .unwrap();
        (db, src)
    }

    /// managed 添付 n 件（原本の中身は件ごとに別）の DB。ファイル DB（2 スレッドで Mutex 越しに共有する）
    fn db_with_n_managed(dir: &Path, n: usize) -> Db {
        let db = Db::open(&dir.join("db.sqlite")).unwrap();
        for i in 0..n {
            let csl = format!(
                "{{\"title\":\"Race {i}\",\"type\":\"article-journal\",\"author\":[{{\"family\":\"Kano\"}}],\"issued\":{{\"date-parts\":[[2026]]}}}}"
            );
            db.conn.execute("INSERT INTO items (key, csl) VALUES (?1, ?2)", (format!("ITEM{i:04}"), &csl)).unwrap();
            let src = dir.join(format!("src{i}.pdf"));
            std::fs::write(&src, vec![i as u8; 100_000]).unwrap();
            db.conn
                .execute(
                    "INSERT INTO attachments (key, item_id, kind, mode, path) VALUES (?1, ?2, 'pdf', 'managed', ?3)",
                    (format!("ATT{i:05}"), db.conn.last_insert_rowid(), src.to_string_lossy()),
                )
                .unwrap();
        }
        db
    }

    /// 全件 linked・各行の指す先に原本と同じ中身・Drive 側に誰も指さないファイル（孤児・一時ファイル）が無い
    fn assert_all_moved_no_orphans(db: &Db, drive: &Path, n: usize) {
        let rows: Vec<(String, String)> = db
            .conn
            .prepare("SELECT mode, path FROM attachments ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(rows.len(), n);
        for (i, (mode, path)) in rows.iter().enumerate() {
            assert_eq!(mode, "linked");
            assert_eq!(std::fs::read(path).unwrap(), vec![i as u8; 100_000]);
        }
        let pointed: std::collections::HashSet<_> = rows.iter().map(|(_, p)| PathBuf::from(p)).collect();
        let files: Vec<_> = tree(drive).into_iter().map(|r| drive.join(r)).filter(|p| p.is_file()).collect();
        let orphans: Vec<_> = files.iter().filter(|p| !pointed.contains(*p)).collect();
        assert!(orphans.is_empty(), "孤児: {orphans:?}");
    }

    /// 「Drive へ移行」の二度押し（競合の再現）: 同時に始めた 2 本のうち 1 本だけが移し、もう 1 本は「実行中」で断る・孤児 0
    #[test]
    fn concurrent_migrations_one_runs_the_other_is_rejected() {
        use std::sync::{mpsc, Arc, Barrier};
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        fn lock(m: &Mutex<Db>) -> Result<std::sync::MutexGuard<'_, Db>> {
            m.lock().map_err(|e| CoreError::BadInput(e.to_string()))
        }
        // (1) 重なりを決める: A は最初の借用（フライトを取った後）で B が返るのを待つ → B は必ず断られる
        {
            let dir = scratch("flight");
            std::env::set_var("OPENREF_MOUNT", dir.join("drive/OpenRef"));
            let db = Arc::new(Mutex::new(db_with_n_managed(&dir, 10)));
            let (in_tx, in_rx) = mpsc::channel();
            let (b_tx, b_rx) = mpsc::channel::<()>();
            let a = {
                let db = db.clone();
                std::thread::spawn(move || {
                    let first = Cell::new(true);
                    migrate_managed_to_drive(
                        || {
                            if first.replace(false) {
                                in_tx.send(()).unwrap();
                                b_rx.recv().unwrap();
                            }
                            lock(&db)
                        },
                        false,
                    )
                })
            };
            in_rx.recv().unwrap();
            let b = migrate_managed_to_drive(|| lock(&db), false);
            b_tx.send(()).unwrap();
            assert!(b.unwrap_err().to_string().contains("実行中"));
            assert_eq!(a.join().unwrap().unwrap().moved.len(), 10);
            assert_all_moved_no_orphans(&db.lock().unwrap(), &dir.join("drive"), 10);
            // 終われば次の移行は通る（フライトが空く）
            assert!(migrate_managed_to_drive(|| lock(&db), false).unwrap().moved.is_empty());
            std::fs::remove_dir_all(&dir).ok();
        }
        // (2) Barrier で同時に始める × 10 回: 移した件数の合計 = 10・Err は「実行中」だけ・孤児 0
        for round in 0..10 {
            let dir = scratch(&format!("race{round}"));
            std::env::set_var("OPENREF_MOUNT", dir.join("drive/OpenRef"));
            let db = Arc::new(Mutex::new(db_with_n_managed(&dir, 10)));
            let bar = Arc::new(Barrier::new(2));
            let hs: Vec<_> = (0..2)
                .map(|_| {
                    let (db, bar) = (db.clone(), bar.clone());
                    std::thread::spawn(move || {
                        bar.wait();
                        migrate_managed_to_drive(|| lock(&db), false)
                    })
                })
                .collect();
            let mut moved = 0;
            for h in hs {
                match h.join().unwrap() {
                    Ok(mig) => {
                        assert!(mig.skipped.is_empty());
                        moved += mig.moved.len()
                    }
                    Err(e) => assert!(e.to_string().contains("実行中"), "round {round}: {e}"),
                }
            }
            assert_eq!(moved, 10, "round {round}");
            assert_all_moved_no_orphans(&db.lock().unwrap(), &dir.join("drive"), 10);
            std::fs::remove_dir_all(&dir).ok();
        }
        std::env::remove_var("OPENREF_MOUNT");
    }

    /// enable で件数が数えられない → Err で、設定は managed のまま（数えてから保存する）
    #[test]
    fn enable_keeps_setting_when_count_fails() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let dir = scratch("m1");
        std::env::set_var("OPENREF_MOUNT", dir.join("drive/OpenRef"));
        let (db, _) = db_with_managed(&dir);
        db.conn.execute_batch("ALTER TABLE attachments RENAME TO attachments_gone").unwrap();
        assert!(enable_drive_mode(|| Ok(&db)).is_err());
        assert_eq!(db.pdf_write_mode().unwrap(), "managed");
        std::env::remove_var("OPENREF_MOUNT");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 原本を読む前に添付が消された（一覧の後・ロックの外）→ その 1 件だけ移さず残りは移す。行がそのままの原本欠落は従来どおり Err
    #[test]
    fn purged_before_read_skips_only_that_attachment() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let dir = scratch("purge");
        std::env::set_var("OPENREF_MOUNT", dir.join("drive/OpenRef"));
        let db = db_with_n_managed(&dir, 2);
        let n = Cell::new(0);
        let mig = migrate_managed_to_drive(
            || {
                n.set(n.get() + 1);
                if n.get() == 2 {
                    // 1 件目の原本を読む直前: 添付ごと消す（行と storage の原本）
                    db.conn.execute("DELETE FROM attachments WHERE key = 'ATT00000'", []).unwrap();
                    std::fs::remove_file(dir.join("src0.pdf")).unwrap();
                }
                Ok(&db)
            },
            false,
        )
        .unwrap();
        assert_eq!(mig.moved.len(), 1);
        assert_eq!(mig.moved[0].item_key, "ITEM0001");
        assert_eq!(mig.skipped.len(), 1);
        assert_eq!(mig.skipped[0].item_key, "ITEM0000");

        let dir2 = scratch("purge-missing");
        let db2 = db_with_n_managed(&dir2, 1);
        std::fs::remove_file(dir2.join("src0.pdf")).unwrap();
        let e = migrate_managed_to_drive(|| Ok(&db2), false).unwrap_err();
        assert!(e.to_string().contains("ローカルの原本を読めません"), "{e}");

        std::env::remove_var("OPENREF_MOUNT");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&dir2).ok();
    }

    /// 守りで書き換えなかったコピー: 指す行があれば消さない・消せなければ leftover で知らせる
    #[test]
    fn skipped_copy_is_removed_only_if_unreferenced_and_failures_are_reported() {
        use std::os::unix::fs::PermissionsExt;
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let dir = scratch("skip");
        let mount = dir.join("drive/OpenRef");
        std::env::set_var("OPENREF_MOUNT", &mount);
        let shelf = mount.join("All Papers/K");
        // 4 回目の借用 = 書換えの直前（コピーは置かれた後）に、原本の行を置き換え、f でもう一手
        let run = |f: &dyn Fn(&Db)| {
            let (db, _) = db_with_managed(&dir);
            let n = Cell::new(0);
            let mig = migrate_managed_to_drive(
                || {
                    n.set(n.get() + 1);
                    if n.get() == 4 {
                        db.conn.execute("UPDATE attachments SET path = '/replaced.pdf'", []).unwrap();
                        f(&db);
                    }
                    Ok(&db)
                },
                false,
            )
            .unwrap();
            assert!(mig.moved.is_empty());
            assert_eq!(mig.skipped.len(), 1);
            mig.skipped.into_iter().next().unwrap()
        };
        let newest = || {
            std::fs::read_dir(&shelf)
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .max_by_key(|p| std::fs::metadata(p).unwrap().modified().unwrap())
                .unwrap()
        };
        // (1) コピーを別の行が指している → 消さない
        let s = run(&|db| {
            db.conn
                .execute(
                    "INSERT INTO attachments (key, item_id, kind, mode, path) VALUES ('ATT00002', 1, 'pdf', 'linked', ?1)",
                    [newest().to_string_lossy()],
                )
                .unwrap();
        });
        assert!(s.leftover.is_none());
        assert_eq!(std::fs::read_dir(&shelf).unwrap().count(), 1, "指されているコピーは残る");
        // (2) 消せない（棚が読み取り専用）→ leftover にパスと理由
        let s = run(&|_| std::fs::set_permissions(&shelf, std::fs::Permissions::from_mode(0o555)).unwrap());
        std::fs::set_permissions(&shelf, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (p, e) = s.leftover.expect("消せなかったコピーを知らせる");
        assert!(p.starts_with(&shelf) && p.exists() && !e.is_empty());

        std::env::remove_var("OPENREF_MOUNT");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// dir 以下の全エントリ（相対パス・整列）
    fn tree(dir: &Path) -> Vec<String> {
        fn walk(base: &Path, d: &Path, out: &mut Vec<String>) {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                out.push(e.path().strip_prefix(base).unwrap().to_string_lossy().into_owned());
                if e.path().is_dir() {
                    walk(base, &e.path(), out);
                }
            }
        }
        let mut out = Vec::new();
        walk(dir, dir, &mut out);
        out.sort();
        out
    }

    #[test]
    fn dry_run_writes_nothing() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let dir = scratch("dry");
        let (db, _) = db_with_managed(&dir);
        let mount = dir.join("drive/OpenRef");
        std::env::set_var("OPENREF_MOUNT", &mount);

        // 移行先が未作成: Err で、何も作らない（従来は detect_or_create が drive/OpenRef/All Papers を作っていた）
        let before = tree(&dir);
        let e = migrate_managed_to_drive(|| Ok(&db), true).unwrap_err();
        assert!(e.to_string().contains("dry-run は作りません"), "{e}");
        assert_eq!(tree(&dir), before);
        assert!(!mount.exists());

        // 移行先がある: 予定を返すだけ（All Papers も頭文字のフォルダも作らない・DB も不変）
        std::fs::create_dir_all(&mount).unwrap();
        let before = tree(&dir);
        let plan = migrate_managed_to_drive(|| Ok(&db), true).unwrap().moved;
        assert_eq!(plan.len(), 1);
        assert!(plan[0].dest_path.starts_with(mount.join("All Papers")));
        assert_eq!(tree(&dir), before);
        let mode: String = db.conn.query_row("SELECT mode FROM attachments", [], |r| r.get(0)).unwrap();
        assert_eq!(mode, "managed");

        std::env::remove_var("OPENREF_MOUNT");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn enable_and_migrate_take_the_lock_only_briefly() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let dir = scratch("mig");
        let mount = dir.join("drive/OpenRef");
        std::env::set_var("OPENREF_MOUNT", &mount);

        // enable: Drive 側を作ってプローブ（ロックの外）→ ロック 1 回で設定と件数
        let (db, src) = db_with_managed(&dir);
        let locks = Cell::new(0);
        let borrow = || {
            locks.set(locks.get() + 1);
            Ok(&db)
        };
        let en = enable_drive_mode(borrow).unwrap();
        assert_eq!(en.mount, mount);
        assert_eq!(en.managed_remaining, 1);
        assert_eq!(locks.get(), 1);
        assert!(mount.join("All Papers").is_dir());
        assert!(!mount.join("All Papers/.openref-write-probe").exists());
        assert_eq!(db.pdf_write_mode().unwrap(), "drive");

        // migrate: 一覧 1 回 + 1 件ごとに 3 回（原本の確認・コピー先の確認・書換え）。コピーは linked に・原本は残す
        locks.set(0);
        let moved = migrate_managed_to_drive(borrow, false).unwrap().moved;
        assert_eq!(locks.get(), 4);
        assert_eq!(moved.len(), 1);
        let dest = moved[0].dest_path.clone();
        assert!(dest.starts_with(mount.join("All Papers")));
        assert_eq!(std::fs::read(&dest).unwrap(), b"%PDF-1.4 test");
        assert!(src.exists());
        let (mode, path): (String, String) = db
            .conn
            .query_row("SELECT mode, path FROM attachments WHERE key = 'ATT00001'", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!((mode.as_str(), path.as_str()), ("linked", dest.to_string_lossy().as_ref()));

        // ロックを離している間（コピー中）に添付が置き換えられた → 書き換えず・書いたコピーを消し・結果に含めない
        let (db2, _) = db_with_managed(&dir);
        let n = Cell::new(0);
        let racing = || {
            n.set(n.get() + 1);
            if n.get() == 4 {
                db2.conn.execute("UPDATE attachments SET path = '/replaced.pdf'", []).unwrap();
            }
            Ok(&db2)
        };
        let shelf = dest.parent().unwrap();
        let before = std::fs::read_dir(shelf).unwrap().count();
        let mig = migrate_managed_to_drive(racing, false).unwrap();
        assert!(mig.moved.is_empty());
        assert_eq!(mig.skipped.len(), 1);
        assert!(mig.skipped[0].leftover.is_none(), "書いたコピーは消せた");
        assert_eq!(n.get(), 4);
        assert_eq!(std::fs::read_dir(shelf).unwrap().count(), before);
        let (mode, path): (String, String) =
            db2.conn.query_row("SELECT mode, path FROM attachments", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((mode.as_str(), path.as_str()), ("managed", "/replaced.pdf"));

        std::env::remove_var("OPENREF_MOUNT");
        std::fs::remove_dir_all(&dir).ok();
    }
}
