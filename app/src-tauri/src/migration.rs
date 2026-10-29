//! Desktop migration jobs use dedicated DB connections. The shared library
//! mutex is only used by the command wrapper to obtain its file path.
use openref_core::import::{
    self, apply, files::AttachmentRoots, plan::ImportPlan, AttachmentMode, ImportInput,
    ImportSource,
};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

#[derive(Clone, Serialize, Deserialize)]
pub struct Config {
    pub format: String,
    pub path: PathBuf,
    pub roots: Vec<PathBuf>,
    pub mode: AttachmentMode,
    pub source_id: Option<String>,
    #[serde(default)]
    pub adopt_legacy_ids: bool,
    #[serde(default)]
    pub decisions: Option<import::decisions::Decisions>,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wait(jobs: &Jobs, phase: &str) -> Status {
        let start = std::time::Instant::now();
        loop {
            let status = jobs.status().unwrap().unwrap();
            if status.phase == phase {
                return status;
            }
            assert!(status.error.is_none(), "{:?}", status.error);
            assert!(
                start.elapsed() < std::time::Duration::from_secs(10),
                "phase={}",
                status.phase
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    #[test]
    fn desktop_job_keeps_preview_readonly_and_crashed_history_is_viewable() {
        let dir = std::env::temp_dir().join(format!(
            "openref-desktop-migration-{}",
            openref_core::keys::new_key()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("export.json");
        let db = dir.join("app/openref.sqlite");
        std::fs::write(&path, r#"[{"_id":"one","title":"Desktop fixture","attachments":[{"_id":"pdf","filename":"note.pdf"}]}]"#).unwrap();
        std::fs::write(
            dir.join("note.pdf"),
            openref_core::pdftools::minimal_pdf("History index fixture", None),
        )
        .unwrap();
        let jobs = Jobs::default();
        let id = jobs
            .preview(
                db.clone(),
                Config {
                    format: "paperpile".into(),
                    path: path.clone(),
                    roots: vec![dir.clone()],
                    mode: AttachmentMode::Copy,
                    source_id: None,
                    adopt_legacy_ids: false,
                    decisions: None,
                },
            )
            .unwrap();
        assert_eq!(wait(&jobs, "ready").plan.unwrap().counts.new_items, 1);
        assert!(!db.parent().unwrap().exists());
        jobs.start(&id).unwrap();
        let report = wait(&jobs, "finished").report.unwrap();
        assert_eq!(report.results.len(), 1);
        let library = openref_core::Db::open(&db).unwrap();
        library
            .conn
            .execute(
                "UPDATE import_runs SET state='running',report_json=NULL",
                [],
            )
            .unwrap();
        drop(library);
        jobs.select_report(db.clone(), &report.run_id).unwrap();
        let historical = jobs.status().unwrap().unwrap();
        assert_eq!(historical.phase, "finished");
        let historical_report = historical.report.unwrap();
        assert_eq!(historical_report.state, "running");
        assert_eq!(
            historical_report.fulltext_pending, 1,
            "the GUI index button must remain available after a crash"
        );
        // Reading an interrupted report does not create a phantom active job.
        jobs.index(&report.run_id).unwrap();
        assert_eq!(wait(&jobs, "finished").report.unwrap().fulltext_pending, 0);
        jobs.select_report(db.clone(), &report.run_id).unwrap();
        assert_eq!(
            jobs.status()
                .unwrap()
                .unwrap()
                .report
                .unwrap()
                .fulltext_pending,
            0
        );
        let id = jobs
            .preview(
                db.clone(),
                Config {
                    format: "paperpile".into(),
                    path,
                    roots: vec![dir.clone()],
                    mode: AttachmentMode::Copy,
                    source_id: None,
                    adopt_legacy_ids: false,
                    decisions: None,
                },
            )
            .unwrap();
        assert_eq!(
            wait(&jobs, "ready").plan.unwrap().source_id,
            report.source_id,
            "omitted source reuses the single existing source"
        );
        jobs.cancel(&id).unwrap();
        drop(jobs);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn desktop_choices_require_a_new_preview_and_apply_only_selected_targets() {
        let dir = std::env::temp_dir().join(format!(
            "openref-desktop-choices-{}",
            openref_core::keys::new_key()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("export.json");
        let db = dir.join("app/library.sqlite");
        let raw = r#"[{"_id":"one","title":"Desktop choices","attachments":[{"_id":"a","filename":"a.pdf"}]}]"#;
        std::fs::write(&path, raw).unwrap();
        let jobs = Jobs::default();
        let mut config = Config {
            format: "paperpile".into(),
            path: path.clone(),
            roots: vec![],
            mode: AttachmentMode::Copy,
            source_id: Some("desktop-choices".into()),
            adopt_legacy_ids: false,
            decisions: None,
        };
        let first = jobs.preview(db.clone(), config.clone()).unwrap();
        wait(&jobs, "ready");
        jobs.start(&first).unwrap();
        let initial = wait(&jobs, "finished").report.unwrap();
        std::fs::write(&path, format!("{raw}\n")).unwrap();
        for name in ["a", "b"] {
            let folder = dir.join(name);
            std::fs::create_dir(&folder).unwrap();
            std::fs::write(folder.join("a.pdf"), format!("%PDF-{name}")).unwrap();
            config.roots.push(folder);
        }
        let prior = jobs.preview(db.clone(), config.clone()).unwrap();
        let plan = wait(&jobs, "ready").plan.unwrap();
        assert_eq!(plan.counts.review, 1);
        assert_eq!(plan.counts.ambiguous_pdfs, 1);
        config.decisions = Some(import::decisions::Decisions {
            source_id: plan.source_id,
            input_sha256: plan.input_sha256,
            items: vec![import::decisions::ItemChoice {
                object_id: plan.items[0].object_id.clone(),
                item_key: initial.results[0].item_key.clone().unwrap(),
                csl_sha256: Some(plan.items[0].candidate_details[0].csl_sha256.clone()),
            }],
            files: vec![plan.items[0].file_options[0].clone()],
        });
        let selected = jobs.preview(db.clone(), config).unwrap();
        let ready = wait(&jobs, "ready");
        assert_eq!(ready.plan.unwrap().counts.review, 0);
        assert!(ready.config.unwrap().decisions.is_some());
        assert!(
            jobs.start(&prior).is_err(),
            "old preview cannot start after decisions change"
        );
        jobs.start(&selected).unwrap();
        let report = wait(&jobs, "finished").report.unwrap();
        assert_eq!(report.results[0].status, "merged");
        assert_eq!(report.results[0].attachments[0].status, "copied");
        drop(jobs);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
struct Approved {
    config: Config,
    input: ImportInput,
    plan: ImportPlan,
    roots: AttachmentRoots,
    db: PathBuf,
}
struct Job {
    id: String,
    db: PathBuf,
    phase: String,
    cancel: Arc<AtomicBool>,
    progress: Option<import::ImportProgress>,
    approved: Option<Approved>,
    report: Option<apply::ImportReport>,
    error: Option<String>,
}
#[derive(Clone, Default)]
pub struct Jobs(Arc<Mutex<Option<Job>>>);
#[derive(Serialize)]
pub struct Status {
    id: String,
    phase: String,
    progress: Option<import::ImportProgress>,
    config: Option<Config>,
    plan: Option<ImportPlan>,
    report: Option<apply::ImportReport>,
    error: Option<String>,
}
impl Jobs {
    pub fn status(&self) -> Result<Option<Status>, String> {
        let slot = self.0.lock().map_err(super::err)?;
        Ok(slot.as_ref().map(|j| Status {
            id: j.id.clone(),
            phase: j.phase.clone(),
            progress: j.progress.clone(),
            config: j.approved.as_ref().map(|a| {
                let mut config = a.config.clone();
                config.source_id = Some(a.input.source.id.clone());
                config
            }),
            plan: j.approved.as_ref().map(|a| a.plan.clone()),
            report: j.report.clone(),
            error: j.error.clone(),
        }))
    }
    fn progress(&self, id: &str, p: &import::ImportProgress) {
        if let Ok(mut slot) = self.0.lock() {
            if let Some(j) = slot.as_mut().filter(|j| j.id == id) {
                j.progress = Some(p.clone());
            }
        }
    }
    pub fn preview(&self, db: PathBuf, config: Config) -> Result<String, String> {
        if !matches!(config.format.as_str(), "paperpile" | "zotero") {
            return Err("対応する移行元を選択してください".into());
        }
        let id = openref_core::keys::new_key();
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut slot = self.0.lock().map_err(super::err)?;
            if slot
                .as_ref()
                .is_some_and(|j| matches!(j.phase.as_str(), "preparing" | "running" | "indexing"))
            {
                return Err("別の移行処理が進行中です".into());
            }
            *slot = Some(Job {
                id: id.clone(),
                db: db.clone(),
                phase: "preparing".into(),
                cancel: cancel.clone(),
                progress: None,
                approved: None,
                report: None,
                error: None,
            });
        }
        let jobs = self.clone();
        let worker_id = id.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                || -> Result<Approved, String> {
                    apply::reject_database_file(&config.path, &db).map_err(super::err)?;
                    let source = ImportSource {
                        id: "pending".into(),
                        format: config.format.clone(),
                    };
                    let mut input = if config.format == "paperpile" {
                        import::paperpile::read_input(&config.path, source)
                    } else {
                        import::zotero_rdf::read_input(&config.path, source)
                    }
                    .map_err(super::err)?;
                    input.source.id = config
                        .source_id
                        .clone()
                        .filter(|s| !s.trim().is_empty())
                        .map(|s| {
                            if s == "@new" {
                                format!("{}-{}", config.format, openref_core::keys::new_key())
                            } else {
                                s
                            }
                        })
                        .map(Ok)
                        .unwrap_or_else(|| apply::default_source(&db, &config.format))
                        .map_err(super::err)?;
                    input.adopt_legacy_paperpile_ids =
                        config.format == "paperpile" && config.adopt_legacy_ids;
                    let mut paths = config.roots.clone();
                    if config.format == "zotero" {
                        paths.push(
                            std::fs::canonicalize(&config.path)
                                .map_err(super::err)?
                                .parent()
                                .ok_or("RDF の親フォルダがありません")?
                                .into(),
                        );
                    }
                    let roots = AttachmentRoots::new(&paths).map_err(super::err)?;
                    let mut plan =
                        apply::prepare_controlled(&db, &input, &roots, config.mode, &cancel, |p| {
                            jobs.progress(&worker_id, p)
                        })
                        .map_err(super::err)?;
                    if let Some(choices) = &config.decisions {
                        import::decisions::resolve(
                            &db, &input, &roots, &mut plan, choices, &cancel,
                        )
                        .map_err(super::err)?;
                    }
                    Ok(Approved {
                        config,
                        input,
                        plan,
                        roots,
                        db,
                    })
                },
            ))
            .unwrap_or_else(|_| {
                Err("移行の事前確認が予期せず終了しました。再試行してください".into())
            });
            if let Ok(mut slot) = jobs.0.lock() {
                if let Some(j) = slot.as_mut().filter(|j| j.id == worker_id) {
                    if cancel.load(Ordering::Relaxed) {
                        j.phase = "cancelled".into();
                    } else {
                        match result {
                            Ok(approved) => {
                                j.approved = Some(approved);
                                j.phase = "ready".into();
                            }
                            Err(error) => {
                                j.error = Some(error);
                                j.phase = "error".into();
                            }
                        }
                    }
                }
            }
        });
        Ok(id)
    }
    pub fn start(&self, id: &str) -> Result<(), String> {
        let (approved, cancel) = {
            let mut slot = self.0.lock().map_err(super::err)?;
            let j = slot
                .as_mut()
                .filter(|j| j.id == id && j.phase == "ready")
                .ok_or("有効な事前確認がありません。もう一度確認してください")?;
            let approved = j.approved.take().ok_or("事前確認の入力がありません")?;
            j.phase = "running".into();
            (approved, j.cancel.clone())
        };
        let jobs = self.clone();
        let id = id.to_string();
        tauri::async_runtime::spawn_blocking(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                apply::execute(
                    &approved.db,
                    &approved.config.path,
                    &approved.input,
                    &approved.plan,
                    &approved.roots,
                    &cancel,
                    |p| jobs.progress(&id, p),
                )
            }))
            .map_err(|_| {
                "移行が予期せず終了しました。履歴を確認して同じ入力から再試行してください"
                    .to_string()
            })
            .and_then(|r| r.map_err(super::err));
            if let Ok(mut slot) = jobs.0.lock() {
                if let Some(j) = slot.as_mut().filter(|j| j.id == id) {
                    match result {
                        Ok(report) => {
                            j.phase = report.state.clone();
                            j.report = Some(report);
                        }
                        Err(error) => {
                            j.phase = "error".into();
                            j.error = Some(error);
                        }
                    }
                }
            }
        });
        Ok(())
    }
    pub fn cancel(&self, id: &str) -> Result<(), String> {
        let mut slot = self.0.lock().map_err(super::err)?;
        let j = slot
            .as_mut()
            .filter(|j| j.id == id)
            .ok_or("移行処理が見つかりません")?;
        j.cancel.store(true, Ordering::Relaxed);
        if j.phase == "ready" {
            j.approved = None;
            j.phase = "cancelled".into();
        }
        Ok(())
    }
    pub fn save_report(&self, id: &str, path: PathBuf) -> Result<(), String> {
        let contents = {
            let slot = self.0.lock().map_err(super::err)?;
            if let Some(job) = slot.as_ref().filter(|j| j.id == id) {
                apply::reject_database_file(&path, &job.db).map_err(super::err)?;
            }
            let report = slot
                .as_ref()
                .filter(|j| j.id == id)
                .and_then(|j| j.report.as_ref())
                .ok_or("保存する移行結果がありません")?;
            serde_json::to_vec_pretty(report).map_err(super::err)?
        };
        std::fs::write(path, contents).map_err(super::err)
    }
    pub fn select_report(&self, db: PathBuf, run: &str) -> Result<(), String> {
        let history = apply::history(&db).map_err(super::err)?;
        let entry = history
            .into_iter()
            .find(|h| h["id"].as_str() == Some(run))
            .ok_or("移行実行が見つかりません")?;
        let mut value = entry["report"].clone();
        // Older interrupted runs only have committed per-item results.
        for (field, fallback) in [
            ("source_id", entry["source_id"].clone()),
            ("total", entry["total"].clone()),
            ("backup_path", entry["backup_path"].clone()),
            ("fulltext_pending", serde_json::json!(0)),
        ] {
            if value.get(field).is_none() {
                value[field] = fallback;
            }
        }
        let report: apply::ImportReport = serde_json::from_value(value).map_err(super::err)?;
        let mut slot = self.0.lock().map_err(super::err)?;
        if slot
            .as_ref()
            .is_some_and(|j| matches!(j.phase.as_str(), "preparing" | "running" | "indexing"))
        {
            return Err("別の移行処理が進行中です".into());
        }
        *slot = Some(Job {
            id: run.into(),
            db,
            phase: "finished".into(),
            cancel: Arc::new(AtomicBool::new(false)),
            progress: None,
            approved: None,
            report: Some(report),
            error: None,
        });
        Ok(())
    }
    pub fn index(&self, id: &str) -> Result<(), String> {
        let (db_path, mut report, cancel) = {
            let mut slot = self.0.lock().map_err(super::err)?;
            let j = slot
                .as_mut()
                .filter(|j| {
                    j.id == id && !matches!(j.phase.as_str(), "preparing" | "running" | "indexing")
                })
                .ok_or("索引処理を開始できません")?;
            let report = j.report.clone().ok_or("移行結果がありません")?;
            j.cancel.store(false, Ordering::Relaxed);
            j.phase = "indexing".into();
            j.error = None;
            (j.db.clone(), report, j.cancel.clone())
        };
        let jobs = self.clone();
        let id = id.to_string();
        tauri::async_runtime::spawn_blocking(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                apply::index_report(&db_path, &mut report, &cancel, |p| jobs.progress(&id, p))
            }))
            .map_err(|_| "全文索引が予期せず終了しました。文献と PDF は保存済みです".to_string())
            .and_then(|r| r.map_err(super::err));
            if let Ok(mut slot) = jobs.0.lock() {
                if let Some(j) = slot.as_mut().filter(|j| j.id == id) {
                    j.phase = "finished".into();
                    j.report = Some(report);
                    if let Err(error) = result {
                        j.error = Some(error);
                    }
                }
            }
        });
        Ok(())
    }
}
