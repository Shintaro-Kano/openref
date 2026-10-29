//! Paperpile 互換の Google Drive mount 検出と cutover（Drive for Desktop 前提）。

use crate::{drive_layout, CoreError, Db, Result};
use std::path::PathBuf;

/// `~/Library/CloudStorage/GoogleDrive-*/{マイドライブ,My Drive}/{Paperpile,OpenRef}` を探す。
/// Paperpile からの移行ユーザーの既存環境を壊さないよう `Paperpile` を先に探し、無ければ `OpenRef` を探す。
/// `OPENREF_MOUNT` が設定されていればそれを優先（検証・将来の配置替え用）。
pub fn detect() -> Option<PathBuf> {
    if let Some(p) = crate::env_os("MOUNT").map(PathBuf::from) {
        return p.is_dir().then_some(p);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let cs = home.join("Library/CloudStorage");
    for entry in std::fs::read_dir(&cs).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("GoogleDrive-") {
            continue;
        }
        for dd in ["マイドライブ", "My Drive"] {
            let root = entry.path().join(dd);
            for folder in ["Paperpile", "OpenRef"] {
                let cand = root.join(folder);
                if cand.is_dir() {
                    return Some(cand);
                }
            }
        }
    }
    None
}

/// Drive モード有効化時: 既存の `detect()`（`Paperpile` または `OpenRef`）があればそれを返し、無ければ
/// `~/Library/CloudStorage/GoogleDrive-*/{マイドライブ,My Drive}` の下に `OpenRef/All Papers` を作成して返す。
/// Paperpile からの移行でない新規ユーザーは `OpenRef/All Papers` に PDF が保存される。
pub fn detect_or_create() -> Option<PathBuf> {
    if let Some(p) = detect() {
        return Some(p);
    }
    if let Some(p) = crate::env_os("MOUNT").map(PathBuf::from) {
        std::fs::create_dir_all(p.join("All Papers")).ok()?;
        return Some(p);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let cs = home.join("Library/CloudStorage");
    for entry in std::fs::read_dir(&cs).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("GoogleDrive-") {
            continue;
        }
        for dd in ["マイドライブ", "My Drive"] {
            let root = entry.path().join(dd);
            if root.is_dir() {
                let openref_dir = root.join("OpenRef");
                if std::fs::create_dir_all(openref_dir.join("All Papers")).is_ok() {
                    return Some(openref_dir);
                }
            }
        }
    }
    None
}

/// `attachments` の `mode` ごとの件数（`"managed"` / `"linked"`）。
pub fn count_attachments_by_mode(db: &Db, mode: &str) -> i64 {
    db.conn
        .query_row("SELECT COUNT(*) FROM attachments WHERE mode = ?1", [mode], |r| r.get(0))
        .unwrap_or(0)
}

/// Google Drive モードへの切替結果
#[derive(Debug, Clone)]
pub struct EnabledDrive {
    pub mount: PathBuf,
    pub managed_remaining: i64,
}

/// Google Drive モードを有効化する（preflight + `pdf_write_mode = "drive"`）。
/// 1. Drive mount の検出（Paperpile フォルダが未作成でも Google Drive for Desktop があれば `Paperpile/All Papers` を自動作成）
/// 2. `All Papers` への書込プローブ（`.openref-write-probe`）
/// 3. `settings.pdf_write_mode = "drive"` の保存
pub fn enable_drive_mode(db: &Db) -> Result<EnabledDrive> {
    let m = detect_or_create().ok_or_else(|| {
        CoreError::BadInput(
            "Google Drive が見つかりません（Google Drive for Desktop の起動を確認してください。OPENREF_MOUNT で明示も可）"
                .into(),
        )
    })?;
    let all_papers = m.join("All Papers");
    if !all_papers.is_dir() {
        std::fs::create_dir_all(&all_papers)?;
    }
    let probe = all_papers.join(".openref-write-probe");
    std::fs::write(&probe, b"probe").map_err(|e| {
        CoreError::BadInput(format!("Google Drive への書込確認に失敗しました ({}): {e}", probe.display()))
    })?;
    std::fs::remove_file(&probe).ok();
    db.set_setting("pdf_write_mode", "drive")?;
    Ok(EnabledDrive {
        mount: m,
        managed_remaining: count_attachments_by_mode(db, "managed"),
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

/// `mode = 'managed'` の添付 PDF をすべて Drive 互換レイアウトへコピーし、読み戻し照合の上で `mode = 'linked'` に切り替える。
/// 元の `storage/` ファイルは安全のため削除せず残す。
pub fn migrate_managed_to_drive(db: &Db, dry_run: bool) -> Result<Vec<MigratedAttachment>> {
    let m = if dry_run {
        detect().or_else(detect_or_create)
    } else {
        detect_or_create()
    }
    .ok_or_else(|| CoreError::BadInput("Google Drive が見つかりません".into()))?;
    let rows: Vec<(i64, String, String, String)> = {
        let mut stmt = db.conn.prepare(
            "SELECT a.id, a.path, i.key, i.csl FROM attachments a
             JOIN items i ON i.id = a.item_id WHERE a.mode = 'managed' ORDER BY a.id",
        )?;
        let r = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        r
    };
    let mut out = Vec::with_capacity(rows.len());
    for (att_id, src_path, item_key, csl_s) in rows {
        let csl: serde_json::Value = serde_json::from_str(&csl_s)?;
        if dry_run {
            let (initial, filename) = drive_layout::relative_target(&csl);
            out.push(MigratedAttachment {
                item_key,
                src_path,
                dest_path: m.join("All Papers").join(initial).join(filename),
            });
            continue;
        }
        crate::db::reject_connection_file(&db.conn, std::path::Path::new(&src_path))?;
        let bytes = std::fs::read(&src_path).map_err(|e| {
            CoreError::BadInput(format!("ローカルの原本を読めません ({src_path}): {e}"))
        })?;
        let dest = drive_layout::place_bytes(&m, &csl, &bytes)?;
        // 読み戻し照合（サイズ + 内容。Drive 書込の完全性を確認してから DB を書き換える）
        crate::db::reject_connection_file(&db.conn, &dest)?;
        let back = std::fs::read(&dest)?;
        if back != bytes {
            return Err(CoreError::BadInput(format!(
                "照合不一致: {} — DB は変更していません",
                dest.display()
            )));
        }
        db.conn.execute(
            "UPDATE attachments SET mode = 'linked', path = ?1 WHERE id = ?2",
            (dest.to_string_lossy().to_string(), att_id),
        )?;
        out.push(MigratedAttachment {
            item_key,
            src_path,
            dest_path: dest,
        });
    }
    Ok(out)
}

