use crate::error::{err, Result};
use crate::model::{
    DraftSummary, GroupPreset, ImportReport, JobFile, JobItem, JobManifest, NamingRule,
    PresetTagItem, TagItem, TaskTemplate,
};
use crate::names::{stem_key, validate_component};
use crate::presets::{category_of, default_naming_for_pattern, EXAMPLE_PRESETS, EXAMPLE_TAGS};
use crate::util::{new_id, now_iso};
use rusqlite::{params, Connection};
use std::path::Path;

pub fn open_db(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| err("E004", format!("无法创建数据目录：{e}")))?;
    }
    let conn = Connection::open(path).map_err(|e| err("", format!("数据库无法打开：{e}")))?;
    conn.execute_batch(
        "
        PRAGMA journal_mode=WAL;
        PRAGMA synchronous=FULL;
        PRAGMA busy_timeout=5000;
        PRAGMA foreign_keys=ON;
        CREATE TABLE IF NOT EXISTS meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS tags (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            short_label TEXT NOT NULL,
            aliases_json TEXT NOT NULL,
            sort_order INTEGER NOT NULL,
            enabled INTEGER NOT NULL,
            source TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            stem_key TEXT NOT NULL UNIQUE
        );
        CREATE TABLE IF NOT EXISTS drafts (
            id TEXT PRIMARY KEY,
            payload TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            source_root TEXT NOT NULL,
            task_name TEXT NOT NULL,
            status TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS jobs (
            id TEXT PRIMARY KEY,
            task_id TEXT NOT NULL,
            manifest TEXT NOT NULL,
            created_at TEXT NOT NULL,
            status TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS job_items (
            job_id TEXT NOT NULL,
            item_index INTEGER NOT NULL,
            photo_id TEXT NOT NULL,
            source_path TEXT NOT NULL,
            target_path TEXT NOT NULL,
            relative_path TEXT NOT NULL,
            target_name TEXT NOT NULL,
            extension TEXT NOT NULL,
            group_name TEXT NOT NULL,
            expected_size INTEGER NOT NULL,
            expected_mtime_ns INTEGER NOT NULL,
            status TEXT NOT NULL,
            message TEXT NOT NULL,
            code TEXT NOT NULL,
            temp_path TEXT,
            sha256 TEXT,
            bytes INTEGER NOT NULL,
            source_name TEXT NOT NULL,
            capture_text TEXT NOT NULL,
            time_source TEXT NOT NULL,
            PRIMARY KEY (job_id, item_index)
        );
        CREATE INDEX IF NOT EXISTS idx_job_items_job_id ON job_items(job_id);
        CREATE TABLE IF NOT EXISTS copy_intents (
            job_id TEXT NOT NULL,
            item_index INTEGER NOT NULL,
            payload TEXT NOT NULL,
            PRIMARY KEY (job_id, item_index)
        );
        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS templates (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            task_name TEXT NOT NULL,
            dir_layout TEXT NOT NULL,
            dir_pattern TEXT NOT NULL,
            naming_json TEXT NOT NULL,
            groups_json TEXT NOT NULL,
            tags_json TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS presets (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            scene_type TEXT NOT NULL DEFAULT '',
            groups_json TEXT NOT NULL DEFAULT '[]',
            items_json TEXT NOT NULL DEFAULT '[]',
            naming_json TEXT NOT NULL DEFAULT '{}',
            version INTEGER NOT NULL DEFAULT 1,
            source TEXT NOT NULL DEFAULT 'custom',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        ",
    )
    .map_err(|e| err("", format!("数据库初始化失败：{e}")))?;
    meta_set(&conn, "schema_version", "1")?;
    seed_examples(&conn)?;
    Ok(conn)
}

pub fn open_worker_db(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path).map_err(|e| err("", format!("数据库无法打开：{e}")))?;
    conn.execute_batch(
        "
        PRAGMA journal_mode=WAL;
        PRAGMA synchronous=FULL;
        PRAGMA busy_timeout=5000;
        PRAGMA foreign_keys=ON;
        ",
    )
    .map_err(|e| err("", format!("数据库配置失败：{e}")))?;
    Ok(conn)
}

pub fn meta_get(conn: &Connection, key: &str) -> Result<Option<String>> {
    match conn.query_row("SELECT value FROM meta WHERE key=?1", params![key], |row| {
        row.get(0)
    }) {
        Ok(value) => Ok(Some(value)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(err("", e.to_string())),
    }
}

pub fn meta_set(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, value],
    )
    .map_err(|e| err("", e.to_string()))?;
    Ok(())
}

fn seed_examples(conn: &Connection) -> Result<()> {
    if meta_get(conn, "examples_seeded")?.as_deref() == Some("1") {
        return Ok(());
    }
    let tag_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM tags", [], |r| r.get(0))
        .map_err(|e| err("", e.to_string()))?;
    if tag_count == 0 {
        let now = now_iso();
        for spec in EXAMPLE_TAGS {
            let aliases = serde_json::to_string(&spec.aliases).unwrap_or_else(|_| "[]".into());
            conn.execute(
                "INSERT OR IGNORE INTO tags (id, name, short_label, aliases_json, sort_order, enabled, source, updated_at, stem_key)
                 VALUES (?1,?2,?3,?4,?5,1,'example',?6,?7)",
                params![
                    spec.id,
                    spec.name,
                    spec.short_label,
                    aliases,
                    spec.sort_order,
                    now,
                    stem_key(spec.name)
                ],
            )
            .map_err(|e| err("", e.to_string()))?;
        }
    }
    let preset_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM presets", [], |r| r.get(0))
        .map_err(|e| err("", e.to_string()))?;
    if preset_count == 0 {
        let now = now_iso();
        let all_tags = list_tags(conn)?;
        for spec in EXAMPLE_PRESETS {
            let groups_json = serde_json::to_string(&spec.group_names).unwrap_or_else(|_| "[]".into());
            let items: Vec<PresetTagItem> = spec
                .tag_names
                .iter()
                .filter_map(|tn| all_tags.iter().find(|t| t.name == *tn))
                .map(|t| PresetTagItem {
                    tag_id: t.id.clone(),
                    name: t.name.clone(),
                    short_label: t.short_label.clone(),
                })
                .collect();
            let items_json = serde_json::to_string(&items).unwrap_or_else(|_| "[]".into());
            let rule = default_naming_for_pattern(spec.default_pattern);
            let naming_json = serde_json::to_string(&rule).unwrap_or_else(|_| "{}".into());
            conn.execute(
                "INSERT OR IGNORE INTO presets (id, name, description, scene_type, groups_json, items_json, naming_json, version, source, created_at, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,1,'example',?8,?9)",
                params![
                    spec.id,
                    spec.name,
                    spec.description,
                    spec.scene_type,
                    groups_json,
                    items_json,
                    naming_json,
                    now,
                    now
                ],
            )
            .map_err(|e| err("", e.to_string()))?;
        }
    }
    meta_set(conn, "examples_seeded", "1")?;
    Ok(())
}

pub fn list_tags(conn: &Connection) -> Result<Vec<TagItem>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, name, short_label, aliases_json, sort_order, enabled, source, updated_at
             FROM tags ORDER BY sort_order, name",
        )
        .map_err(|e| err("", e.to_string()))?;
    let rows = stmt
        .query_map([], |r| {
            let name: String = r.get(1)?;
            let category = category_of(&name).to_string();
            Ok(TagItem {
                id: r.get(0)?,
                name,
                short_label: r.get(2)?,
                aliases: serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or_default(),
                sort_order: r.get(4)?,
                enabled: r.get::<_, i64>(5)? != 0,
                source: r.get(6)?,
                category,
                updated_at: r.get(7)?,
            })
        })
        .map_err(|e| err("", e.to_string()))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| err("", e.to_string()))?);
    }
    Ok(out)
}

pub fn tag_by_id(conn: &Connection, id: &str) -> Result<TagItem> {
    list_tags(conn)?
        .into_iter()
        .find(|t| t.id == id)
        .ok_or_else(|| err("", "标签不存在"))
}

pub fn insert_tag(conn: &Connection, mut tag: TagItem) -> Result<TagItem> {
    validate_component(&tag.name, "标签名称")?;
    if tag.short_label.trim().is_empty() {
        tag.short_label = tag.name.clone();
    }
    let key = stem_key(&tag.name);
    if stem_taken(conn, &key, None)? {
        return Err(err(
            "E008",
            format!("标签「{}」与已有标签在 Windows 下不区分大小写地重复", tag.name),
        ));
    }
    if tag.id.is_empty() {
        tag.id = new_id("tag");
    }
    tag.updated_at = now_iso();
    if tag.source.is_empty() {
        tag.source = "user".into();
    }
    let aliases = serde_json::to_string(&tag.aliases).unwrap_or_else(|_| "[]".into());
    conn.execute(
        "INSERT INTO tags (id, name, short_label, aliases_json, sort_order, enabled, source, updated_at, stem_key)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            tag.id,
            tag.name,
            tag.short_label,
            aliases,
            tag.sort_order,
            if tag.enabled { 1 } else { 0 },
            tag.source,
            tag.updated_at,
            key
        ],
    )
    .map_err(|e| err("E008", e.to_string()))?;
    Ok(tag)
}

pub fn update_tag(conn: &Connection, tag: &TagItem) -> Result<()> {
    validate_component(&tag.name, "标签名称")?;
    let key = stem_key(&tag.name);
    if stem_taken(conn, &key, Some(&tag.id))? {
        return Err(err("E008", format!("标签「{}」与另一标签冲突", tag.name)));
    }
    let aliases = serde_json::to_string(&tag.aliases).unwrap_or_else(|_| "[]".into());
    let n = conn
        .execute(
            "UPDATE tags SET name=?1, short_label=?2, aliases_json=?3, sort_order=?4, enabled=?5, source=?6, updated_at=?7, stem_key=?8 WHERE id=?9",
            params![
                tag.name,
                tag.short_label,
                aliases,
                tag.sort_order,
                if tag.enabled { 1 } else { 0 },
                tag.source,
                now_iso(),
                key,
                tag.id
            ],
        )
        .map_err(|e| err("", e.to_string()))?;
    if n == 0 {
        return Err(err("", "标签不存在"));
    }
    Ok(())
}

fn stem_taken(conn: &Connection, key: &str, except: Option<&str>) -> Result<bool> {
    let mut stmt = conn
        .prepare("SELECT id FROM tags WHERE stem_key=?1")
        .map_err(|e| err("", e.to_string()))?;
    let mut rows = stmt.query(params![key]).map_err(|e| err("", e.to_string()))?;
    while let Some(row) = rows.next().map_err(|e| err("", e.to_string()))? {
        let id: String = row.get(0).map_err(|e| err("", e.to_string()))?;
        if except != Some(id.as_str()) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn delete_tag(conn: &Connection, id: &str) -> Result<()> {
    let n = conn
        .execute("DELETE FROM tags WHERE id=?1", params![id])
        .map_err(|e| err("", e.to_string()))?;
    if n == 0 {
        return Err(err("", "标签不存在"));
    }
    Ok(())
}

pub fn reorder_tags(conn: &Connection, ids: &[String]) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| err("", e.to_string()))?;
    for (i, id) in ids.iter().enumerate() {
        tx.execute(
            "UPDATE tags SET sort_order=?1 WHERE id=?2",
            params![(i as i64 + 1) * 10, id],
        )
        .map_err(|e| err("", e.to_string()))?;
    }
    tx.commit().map_err(|e| err("", e.to_string()))?;
    Ok(())
}

pub fn save_draft(
    conn: &Connection,
    id: &str,
    payload: &str,
    source: &str,
    task_name: &str,
    status: &str,
) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| err("", e.to_string()))?;
    tx.execute(
        "INSERT INTO drafts (id, payload, updated_at, source_root, task_name, status)
         VALUES (?1,?2,?3,?4,?5,?6)
         ON CONFLICT(id) DO UPDATE SET payload=excluded.payload, updated_at=excluded.updated_at,
           source_root=excluded.source_root, task_name=excluded.task_name, status=excluded.status",
        params![id, payload, now_iso(), source, task_name, status],
    )
    .map_err(|e| err("", e.to_string()))?;
    tx.commit().map_err(|e| err("", e.to_string()))?;
    Ok(())
}

fn draft_photo_count(payload: &str) -> u32 {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return 0;
    };
    let photos = value
        .get("task")
        .and_then(|task| task.get("photos"))
        .or_else(|| value.get("photos"));
    photos
        .and_then(|v| v.as_array())
        .map(|items| items.len() as u32)
        .unwrap_or(0)
}

pub fn load_draft(conn: &Connection, id: &str) -> Result<String> {
    conn.query_row("SELECT payload FROM drafts WHERE id=?1", params![id], |r| {
        r.get(0)
    })
    .map_err(|_| err("", "找不到该任务草稿"))
}

pub fn list_drafts(conn: &Connection) -> Result<Vec<DraftSummary>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, source_root, task_name, status, updated_at, payload FROM drafts ORDER BY updated_at DESC",
        )
        .map_err(|e| err("", e.to_string()))?;
    let rows = stmt
        .query_map([], |r| {
            let payload: String = r.get(5)?;
            let photo_count = draft_photo_count(&payload);
            Ok(DraftSummary {
                id: r.get(0)?,
                source_root: r.get(1)?,
                task_name: r.get(2)?,
                status: r.get(3)?,
                updated_at: r.get(4)?,
                photo_count,
            })
        })
        .map_err(|e| err("", e.to_string()))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| err("", e.to_string()))?);
    }
    Ok(out)
}

fn job_header_payload(job: &JobManifest, include_files: bool) -> Result<String> {
    let header = JobManifest {
        job_id: job.job_id.clone(),
        task_id: job.task_id.clone(),
        output_root: job.output_root.clone(),
        task_name: job.task_name.clone(),
        strict_hash: job.strict_hash,
        status: job.status.clone(),
        created_at: job.created_at.clone(),
        items: Vec::new(),
        created_dirs: job.created_dirs.clone(),
        files: if include_files {
            job.files.clone()
        } else {
            Vec::new()
        },
    };
    serde_json::to_string(&header).map_err(|e| err("", e.to_string()))
}

pub fn save_job(conn: &Connection, job: &JobManifest) -> Result<()> {
    let payload = job_header_payload(job, true)?;
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| err("", e.to_string()))?;
    tx.execute(
        "INSERT INTO jobs (id, task_id, manifest, created_at, status) VALUES (?1,?2,?3,?4,?5)
         ON CONFLICT(id) DO UPDATE SET manifest=excluded.manifest, status=excluded.status",
        params![job.job_id, job.task_id, payload, job.created_at, job.status],
    )
    .map_err(|e| err("", e.to_string()))?;
    tx.execute("DELETE FROM job_items WHERE job_id=?1", params![job.job_id])
        .map_err(|e| err("", e.to_string()))?;
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO job_items (
                    job_id, item_index, photo_id, source_path, target_path, relative_path,
                    target_name, extension, group_name, expected_size, expected_mtime_ns,
                    status, message, code, temp_path, sha256, bytes, source_name, capture_text, time_source
                 ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)",
            )
            .map_err(|e| err("", e.to_string()))?;
        for (idx, item) in job.items.iter().enumerate() {
            stmt.execute(params![
                job.job_id,
                idx as i64,
                item.photo_id,
                item.source_path,
                item.target_path,
                item.relative_path,
                item.target_name,
                item.extension,
                item.group_name,
                item.expected_size as i64,
                item.expected_mtime_ns,
                item.status,
                item.message,
                item.code,
                item.temp_path,
                item.sha256,
                item.bytes as i64,
                item.source_name,
                item.capture_text,
                item.time_source
            ])
            .map_err(|e| err("", e.to_string()))?;
        }
    }
    tx.commit().map_err(|e| err("", e.to_string()))?;
    Ok(())
}

pub fn checkpoint_job(
    conn: &Connection,
    job: &JobManifest,
    dirty_indices: &[usize],
    include_files: bool,
) -> Result<()> {
    let payload = job_header_payload(job, include_files)?;
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| err("", e.to_string()))?;
    tx.execute(
        "UPDATE jobs SET manifest=?1, status=?2 WHERE id=?3",
        params![payload, job.status, job.job_id],
    )
    .map_err(|e| err("", e.to_string()))?;
    crate::archive::fault_point("manifest_updated")?;
    if !dirty_indices.is_empty() {
        let mut stmt = tx
            .prepare(
                "UPDATE job_items
                 SET status=?1, message=?2, code=?3, temp_path=?4, sha256=?5, bytes=?6
                 WHERE job_id=?7 AND item_index=?8",
            )
            .map_err(|e| err("", e.to_string()))?;
        for &idx in dirty_indices {
            let Some(item) = job.items.get(idx) else {
                continue;
            };
            stmt.execute(params![
                item.status,
                item.message,
                item.code,
                item.temp_path,
                item.sha256,
                item.bytes as i64,
                job.job_id,
                idx as i64
            ])
            .map_err(|e| err("", e.to_string()))?;
        }
    }
    crate::archive::fault_point("checkpoint")?;
    tx.commit().map_err(|e| err("", e.to_string()))?;
    crate::archive::fault_point("after_checkpoint")?;
    Ok(())
}

fn load_job_items(conn: &Connection, job_id: &str) -> Result<Vec<JobItem>> {
    let mut stmt = conn
        .prepare(
            "SELECT photo_id, source_path, target_path, relative_path, target_name, extension,
                    group_name, expected_size, expected_mtime_ns, status, message, code,
                    temp_path, sha256, bytes, source_name, capture_text, time_source
             FROM job_items
             WHERE job_id=?1
             ORDER BY item_index ASC",
        )
        .map_err(|e| err("", e.to_string()))?;
    let rows = stmt
        .query_map(params![job_id], |r| {
            let expected_size: i64 = r.get(7)?;
            let bytes: i64 = r.get(14)?;
            Ok(JobItem {
                photo_id: r.get(0)?,
                source_path: r.get(1)?,
                target_path: r.get(2)?,
                relative_path: r.get(3)?,
                target_name: r.get(4)?,
                extension: r.get(5)?,
                group_name: r.get(6)?,
                expected_size: expected_size.max(0) as u64,
                expected_mtime_ns: r.get(8)?,
                status: r.get(9)?,
                message: r.get(10)?,
                code: r.get(11)?,
                temp_path: r.get(12)?,
                sha256: r.get(13)?,
                bytes: bytes.max(0) as u64,
                source_name: r.get(15)?,
                capture_text: r.get(16)?,
                time_source: r.get(17)?,
            })
        })
        .map_err(|e| err("", e.to_string()))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| err("", e.to_string()))?);
    }
    Ok(out)
}

fn hydrate_job(conn: &Connection, mut job: JobManifest) -> Result<JobManifest> {
    let items = load_job_items(conn, &job.job_id)?;
    if !items.is_empty() {
        job.items = items;
    }
    crate::archive::reconcile_intents(conn, &mut job)?;
    for item in &job.items {
        if item.status == "succeeded" && !item.target_path.is_empty() {
            if let Some(hash) = &item.sha256 {
                if !job.files.iter().any(|f| f.path == item.target_path) {
                    job.files.push(JobFile {
                        path: item.target_path.clone(),
                        size: item.bytes,
                        sha256: hash.clone(),
                        created_by_job: true,
                        identity: None,
                    });
                }
            }
        }
    }
    Ok(job)
}

pub fn load_job(conn: &Connection, id: &str) -> Result<JobManifest> {
    let payload: String = conn
        .query_row("SELECT manifest FROM jobs WHERE id=?1", params![id], |r| {
            r.get(0)
        })
        .map_err(|_| err("", "找不到该归档作业"))?;
    let job: JobManifest = serde_json::from_str(&payload).map_err(|e| err("", e.to_string()))?;
    hydrate_job(conn, job)
}

pub fn list_jobs(conn: &Connection) -> Result<Vec<JobManifest>> {
    let mut stmt = conn
        .prepare("SELECT manifest FROM jobs ORDER BY created_at DESC")
        .map_err(|e| err("", e.to_string()))?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| err("", e.to_string()))?;
    let mut out = Vec::new();
    for row in rows {
        let payload = row.map_err(|e| err("", e.to_string()))?;
        if let Ok(job) = serde_json::from_str::<JobManifest>(&payload) {
            match hydrate_job(conn, job.clone()) {
                Ok(job) => out.push(job),
                Err(error) => {
                    let mut damaged = job;
                    damaged.status = "failed".into();
                    for item in &mut damaged.items {
                        item.status = "failed".into();
                        item.code = "E007".into();
                        item.message = error.message.clone();
                    }
                    out.push(damaged);
                }
            }
        }
    }
    Ok(out)
}

pub fn clear_jobs_and_drafts(conn: &Connection) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| err("", e.to_string()))?;
    tx.execute("DELETE FROM drafts", [])
        .map_err(|e| err("", e.to_string()))?;
    tx.execute("DELETE FROM copy_intents", [])
        .map_err(|e| err("", e.to_string()))?;
    tx.execute("DELETE FROM job_items", [])
        .map_err(|e| err("", e.to_string()))?;
    tx.execute("DELETE FROM jobs", [])
        .map_err(|e| err("", e.to_string()))?;
    tx.commit().map_err(|e| err("", e.to_string()))?;
    Ok(())
}

pub fn get_setting(conn: &Connection, key: &str) -> Result<Option<String>> {
    match conn.query_row(
        "SELECT value FROM settings WHERE key=?1",
        params![key],
        |r| r.get(0),
    ) {
        Ok(v) => Ok(Some(v)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(err("", e.to_string())),
    }
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, value],
    )
    .map_err(|e| err("", e.to_string()))?;
    Ok(())
}

pub fn save_template(conn: &Connection, template: &TaskTemplate) -> Result<()> {
    let naming_json =
        serde_json::to_string(&template.naming_rule).map_err(|e| err("", e.to_string()))?;
    let groups_json =
        serde_json::to_string(&template.group_names).map_err(|e| err("", e.to_string()))?;
    let tags_json =
        serde_json::to_string(&template.tag_ids).map_err(|e| err("", e.to_string()))?;
    conn.execute(
        "INSERT INTO templates (id, name, task_name, dir_layout, dir_pattern, naming_json, groups_json, tags_json, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            template.id,
            template.name,
            template.task_name,
            template.dir_layout,
            template.dir_pattern,
            naming_json,
            groups_json,
            tags_json,
            template.created_at
        ],
    )
    .map_err(|e| err("", e.to_string()))?;
    Ok(())
}

pub fn list_templates(conn: &Connection) -> Result<Vec<TaskTemplate>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, name, task_name, dir_layout, dir_pattern, naming_json, groups_json, tags_json, created_at
             FROM templates ORDER BY created_at DESC",
        )
        .map_err(|e| err("", e.to_string()))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
            ))
        })
        .map_err(|e| err("", e.to_string()))?;
    let mut out = Vec::new();
    for row in rows {
        let (id, name, task_name, dir_layout, dir_pattern, naming_json, groups_json, tags_json, created_at) =
            row.map_err(|e| err("", e.to_string()))?;
        out.push(TaskTemplate {
            id,
            name,
            task_name,
            dir_layout,
            dir_pattern,
            naming_rule: serde_json::from_str(&naming_json).unwrap_or_default(),
            group_names: serde_json::from_str(&groups_json).unwrap_or_default(),
            tag_ids: serde_json::from_str(&tags_json).unwrap_or_default(),
            created_at,
        });
    }
    Ok(out)
}

pub fn load_template(conn: &Connection, id: &str) -> Result<TaskTemplate> {
    list_templates(conn)?
        .into_iter()
        .find(|template| template.id == id)
        .ok_or_else(|| err("", "找不到该任务模板"))
}

pub fn delete_template(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM templates WHERE id=?1", params![id])
        .map_err(|e| err("", e.to_string()))?;
    Ok(())
}

pub fn cleanup_records(conn: &Connection, days: u32) -> Result<u32> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| err("", e.to_string()))?;
    let count = if days == 0 {
        let drafts = tx
            .execute("DELETE FROM drafts", [])
            .map_err(|e| err("", e.to_string()))? as u32;
        tx.execute("DELETE FROM copy_intents", [])
            .map_err(|e| err("", e.to_string()))?;
        tx.execute("DELETE FROM job_items", [])
            .map_err(|e| err("", e.to_string()))?;
        let jobs = tx
            .execute("DELETE FROM jobs", [])
            .map_err(|e| err("", e.to_string()))? as u32;
        drafts + jobs
    } else {
        let cutoff = cutoff_iso(days);
        let drafts = tx
            .execute("DELETE FROM drafts WHERE updated_at < ?1", params![cutoff])
            .map_err(|e| err("", e.to_string()))? as u32;
        tx.execute(
            "DELETE FROM copy_intents WHERE job_id IN (SELECT id FROM jobs WHERE created_at < ?1)",
            params![cutoff],
        )
        .map_err(|e| err("", e.to_string()))?;
        tx.execute(
            "DELETE FROM job_items WHERE job_id IN (SELECT id FROM jobs WHERE created_at < ?1)",
            params![cutoff],
        )
        .map_err(|e| err("", e.to_string()))?;
        let jobs = tx
            .execute("DELETE FROM jobs WHERE created_at < ?1", params![cutoff])
            .map_err(|e| err("", e.to_string()))? as u32;
        drafts + jobs
    };
    tx.commit().map_err(|e| err("", e.to_string()))?;
    Ok(count)
}

fn cutoff_iso(days: u32) -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .saturating_sub(u64::from(days) * 24 * 60 * 60);
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs as i64, 0)
        .map(|d| d.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".into())
}

pub fn list_presets(conn: &Connection) -> Result<Vec<GroupPreset>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, name, description, scene_type, groups_json, items_json, naming_json, version, source, created_at, updated_at
             FROM presets
             ORDER BY CASE source WHEN 'custom' THEN 0 ELSE 1 END, updated_at DESC, name ASC",
        )
        .map_err(|e| err("", e.to_string()))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, u32>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, String>(10)?,
            ))
        })
        .map_err(|e| err("", e.to_string()))?;
    let mut out = Vec::new();
    for row in rows {
        let (
            id,
            name,
            description,
            scene_type,
            groups_json,
            items_json,
            naming_json,
            version,
            source,
            created_at,
            updated_at,
        ) = row.map_err(|e| err("", e.to_string()))?;
        let default_naming: Option<NamingRule> = serde_json::from_str(&naming_json).ok();
        out.push(GroupPreset {
            id,
            name,
            description,
            scene_type,
            group_names: serde_json::from_str(&groups_json).unwrap_or_default(),
            items: serde_json::from_str(&items_json).unwrap_or_default(),
            default_naming,
            version,
            source,
            created_at,
            updated_at,
        });
    }
    Ok(out)
}

pub fn load_preset(conn: &Connection, id: &str) -> Result<GroupPreset> {
    list_presets(conn)?
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| err("", "找不到该分组方案"))
}

pub fn upsert_preset(conn: &Connection, preset: &GroupPreset) -> Result<GroupPreset> {
    let groups_json =
        serde_json::to_string(&preset.group_names).map_err(|e| err("", e.to_string()))?;
    let items_json = serde_json::to_string(&preset.items).map_err(|e| err("", e.to_string()))?;
    let naming_json = match &preset.default_naming {
        Some(rule) => serde_json::to_string(rule).map_err(|e| err("", e.to_string()))?,
        None => "null".to_string(),
    };
    conn.execute(
        "INSERT INTO presets (id, name, description, scene_type, groups_json, items_json, naming_json, version, source, created_at, updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
         ON CONFLICT(id) DO UPDATE SET
           name=excluded.name,
           description=excluded.description,
           scene_type=excluded.scene_type,
           groups_json=excluded.groups_json,
           items_json=excluded.items_json,
           naming_json=excluded.naming_json,
           version=excluded.version,
           source=excluded.source,
           updated_at=excluded.updated_at",
        params![
            preset.id,
            preset.name,
            preset.description,
            preset.scene_type,
            groups_json,
            items_json,
            naming_json,
            preset.version,
            preset.source,
            preset.created_at,
            preset.updated_at
        ],
    )
    .map_err(|e| err("", e.to_string()))?;
    load_preset(conn, &preset.id)
}

pub fn delete_preset(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM presets WHERE id=?1", params![id])
        .map_err(|e| err("", e.to_string()))?;
    Ok(())
}

pub fn import_tags(conn: &Connection, names: &[String]) -> Result<ImportReport> {
    let existing = list_tags(conn)?;
    let mut added = 0u32;
    for raw in names {
        let name = raw.trim();
        if name.is_empty() {
            continue;
        }
        validate_component(name, "标签名称")?;
        if existing.iter().any(|t| t.name.eq_ignore_ascii_case(name)) {
            continue;
        }
        let next_order = (existing.len() + added as usize + 1) as i64 * 10;
        let tag = TagItem {
            id: new_id("tag"),
            name: name.to_string(),
            short_label: name.to_string(),
            aliases: Vec::new(),
            sort_order: next_order,
            enabled: true,
            source: "user".into(),
            category: "自定义标签".into(),
            updated_at: now_iso(),
        };
        insert_tag(conn, tag)?;
        added += 1;
    }
    Ok(ImportReport { added, updated: 0 })
}

pub(crate) fn save_copy_intent(
    conn: &Connection,
    intent: &crate::archive::CopyIntent,
) -> Result<()> {
    let payload = serde_json::to_string(intent).map_err(|e| err("E007", e.to_string()))?;
    conn.execute(
        "INSERT INTO copy_intents(job_id,item_index,payload) VALUES (?1,?2,?3)
        ON CONFLICT(job_id,item_index) DO UPDATE SET payload=excluded.payload",
        params![intent.job_id, intent.index as i64, payload],
    )
    .map_err(|e| err("E007", format!("提交意图保存失败：{e}")))?;
    Ok(())
}

pub(crate) fn save_copy_intents(
    conn: &Connection,
    intents: &[crate::archive::CopyIntent],
) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| err("E007", e.to_string()))?;
    for intent in intents {
        save_copy_intent(&tx, intent)?;
    }
    tx.commit().map_err(|e| err("E007", e.to_string()))
}

pub(crate) fn copy_intents(
    conn: &Connection,
    job_id: &str,
) -> Result<Vec<crate::archive::CopyIntent>> {
    let mut stmt = conn
        .prepare("SELECT payload FROM copy_intents WHERE job_id=?1 ORDER BY item_index")
        .map_err(|e| err("E007", e.to_string()))?;
    let rows = stmt
        .query_map(params![job_id], |r| r.get::<_, String>(0))
        .map_err(|e| err("E007", e.to_string()))?;
    let mut intents = Vec::new();
    for row in rows {
        intents.push(
            serde_json::from_str(&row.map_err(|e| err("E007", e.to_string()))?)
                .map_err(|e| err("E007", format!("恢复凭据损坏，请保留文件并导出诊断：{e}")))?,
        );
    }
    Ok(intents)
}
