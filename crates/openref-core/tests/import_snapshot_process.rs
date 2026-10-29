//! POSIX record locks belong to a process, not to a single file descriptor.
//! Opening/closing a raw DB fd beside a live SQLite connection can detach WAL.
#[cfg(unix)]
mod unix {
    use openref_core::{import::inventory, Db};
    use std::{path::PathBuf, process::Command};
    #[test]
    fn snapshot_child_connection() {
        let Some(path) = std::env::var_os("OPENREF_SNAPSHOT_CHILD_DB") else {
            return;
        };
        let db = Db::open(&PathBuf::from(path)).unwrap();
        if let Ok(expected) = std::env::var("OPENREF_SNAPSHOT_EXPECT_GUI") {
            assert_eq!(db.setting("snapshot_gui"), Some(expected));
        }
        db.set_setting("snapshot_cli", "external commit").unwrap();
    }
    #[test]
    fn snapshot_preserves_live_connection_locks_across_external_writer_close() {
        use std::os::unix::fs::MetadataExt;
        let root = std::env::temp_dir().join(format!(
            "openref-snapshot-locks-{}",
            openref_core::keys::new_key()
        ));
        let path = root.join("library.sqlite");
        let db = Db::open(&path).unwrap();
        db.set_setting("snapshot_gui", "before preview").unwrap();
        let wal = path.with_extension("sqlite-wal");
        let inode = std::fs::metadata(&wal).unwrap().ino();
        inventory::read(&path).unwrap();
        let child = |expected: &str| {
            let out = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "unix::snapshot_child_connection", "--nocapture"])
                .env("OPENREF_SNAPSHOT_CHILD_DB", &path)
                .env("OPENREF_SNAPSHOT_EXPECT_GUI", expected)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        };
        child("before preview");
        assert!(
            wal.exists(),
            "external close deleted WAL while GUI connection remains open"
        );
        assert_eq!(
            std::fs::metadata(&wal).unwrap().ino(),
            inode,
            "WAL was replaced"
        );
        db.set_setting("snapshot_gui", "after preview").unwrap();
        child("after preview");
        assert_eq!(db.setting("snapshot_cli"), Some("external commit".into()));
        drop(db);
        let reopened = Db::open(&path).unwrap();
        assert_eq!(
            reopened.setting("snapshot_gui"),
            Some("after preview".into())
        );
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn attachment_entry_points_reject_database_aliases_without_releasing_locks() {
        use std::os::unix::fs::MetadataExt;
        let root = std::env::temp_dir().join(format!(
            "openref-alias-locks-{}",
            openref_core::keys::new_key()
        ));
        let path = root.join("library.sqlite");
        let db = Db::open(&path).unwrap();
        db.set_setting("snapshot_gui", "protected").unwrap();
        db.conn
            .execute(
                "INSERT INTO items(key,csl) VALUES('ITEM','{\"title\":\"Alias test\"}')",
                [],
            )
            .unwrap();
        let wal = root.join("library.sqlite-wal");
        let inode = std::fs::metadata(&wal).unwrap().ino();
        for suffix in ["", "-wal", "-shm"] {
            let alias = root.join(format!("alias{suffix}.pdf"));
            std::fs::hard_link(root.join(format!("library.sqlite{suffix}")), &alias).unwrap();
            let error = openref_core::edit::attach_auto(&db, "ITEM", &alias)
                .err()
                .unwrap();
            assert!(error.to_string().contains("DB・WAL・SHM"), "{error}");
            let error = openref_core::pdfingest::attach_or_skip(&db, "ITEM", &alias)
                .err()
                .unwrap();
            assert!(error.to_string().contains("DB・WAL・SHM"), "{error}");
            db.conn.execute("INSERT INTO attachments(key,item_id,kind,mode,path) VALUES(?1,1,'pdf','linked',?2)", rusqlite::params![format!("ATT{suffix}"), alias.to_string_lossy()]).unwrap();
            let error =
                openref_core::fulltext::index_attachment(&db.conn, db.conn.last_insert_rowid())
                    .err()
                    .unwrap();
            assert!(error.to_string().contains("DB・WAL・SHM"), "{error}");
        }
        let ordinary = root.join("ordinary.pdf");
        std::fs::write(&ordinary, b"%PDF-generated").unwrap();
        openref_core::db::reject_connection_file(&db.conn, &ordinary).unwrap();
        let out = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "unix::snapshot_child_connection", "--nocapture"])
            .env("OPENREF_SNAPSHOT_CHILD_DB", &path)
            .env("OPENREF_SNAPSHOT_EXPECT_GUI", "protected")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert_eq!(std::fs::metadata(&wal).unwrap().ino(), inode);
        db.set_setting("snapshot_gui", "after child").unwrap();
        drop(db);
        let reopened = Db::open(&path).unwrap();
        assert_eq!(reopened.setting("snapshot_gui"), Some("after child".into()));
        assert_eq!(
            reopened.setting("snapshot_cli"),
            Some("external commit".into())
        );
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn job_lock_release_is_not_delayed_by_an_unrelated_fork() {
        use openref_core::import::apply::JobLock;
        let root = std::env::temp_dir().join(format!("openref-fork-lock-{}", openref_core::keys::new_key()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("library.sqlite");
        let lock = JobLock::acquire(&path).unwrap();
        // Only async-signal-safe libc operations in the forked child. Simulate
        // another thread spawning a process while this job holds its lock.
        let mut pipe = [0; 2];
        assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0);
        if pid == 0 {
            unsafe {
                libc::close(pipe[1]);
                let mut byte = 0u8;
                libc::read(pipe[0], (&mut byte as *mut u8).cast(), 1);
                libc::_exit(0);
            }
        }
        unsafe { libc::close(pipe[0]); }
        drop(lock);
        let next = JobLock::acquire(&path);
        unsafe {
            let byte = 1u8;
            libc::write(pipe[1], (&byte as *const u8).cast(), 1);
            libc::close(pipe[1]);
            libc::waitpid(pid, std::ptr::null_mut(), 0);
        }
        assert!(next.is_ok(), "finished job remained locked by unrelated fork");
        drop(next);
        std::fs::remove_dir_all(root).unwrap();
    }

}
