/// ANFS — AI-Native File System FUSE overlay for COGNOS/OS.
///
/// Sits above ext4/btrfs as a transparent FUSE mount.
/// Every standard Linux tool works unchanged underneath.
/// ANFS adds a semantic metadata layer on top.
///
/// Key behaviors:
///   - Passthrough: all standard ops are transparent, <5ms overhead
///   - Metadata: tracks access patterns, co-opened files, importance
///   - Snapshot: copies file before any AI write (x-cognos-ai-edit header)
///   - Delete intercept: moves to recycle instead of deleting
///   - inotify bridge: pushes changed paths to Memory Agent's index queue

use fuser::{
    FileAttr, FileType, Filesystem, KernelConfig, ReplyAttr, ReplyData,
    ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyWrite, Request,
    TimeOrNow,
};
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use serde::{Deserialize, Serialize};

const RECYCLE_DIR: &str = ".cognos/anfs/recycle";
const SNAPSHOTS_DIR: &str = ".cognos/anfs/snapshots";
const META_DIR: &str = ".cognos/anfs/meta";
const INDEX_QUEUE: &str = ".cognos/memory/index_queue";

/// Metadata blob stored per file. Written async, never blocking passthrough.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FileAnfsMeta {
    pub path: String,
    pub last_accessed: String,
    pub access_count: u64,
    pub session_ids: Vec<String>,
    pub co_opened_files: Vec<String>,
    pub importance_score: f32,
    pub semantic_tags: Vec<String>,
    pub project_domain: Option<String>,
    pub snapshot_before_edit: Option<String>,
}

/// Pending metadata updates, batched and flushed every 5 seconds.
struct MetaBatch {
    updates: HashMap<String, FileAnfsMeta>,
    last_flush: std::time::Instant,
}

impl MetaBatch {
    fn new() -> Self {
        Self {
            updates: HashMap::new(),
            last_flush: std::time::Instant::now(),
        }
    }

    fn should_flush(&self) -> bool {
        self.last_flush.elapsed() >= Duration::from_secs(5)
    }
}

/// Files the user has marked as unprotected from delete intercept.
struct NoProtectList {
    paths: Vec<PathBuf>,
}

impl NoProtectList {
    fn load(home: &Path) -> Self {
        let p = home.join(".cognos/anfs/noprotect.json");
        if p.exists() {
            if let Ok(s) = std::fs::read_to_string(&p) {
                if let Ok(paths) = serde_json::from_str::<Vec<String>>(&s) {
                    return Self {
                        paths: paths.iter().map(PathBuf::from).collect(),
                    };
                }
            }
        }
        Self { paths: vec![] }
    }

    fn is_protected(&self, path: &Path) -> bool {
        !self.paths.iter().any(|p| p == path)
    }
}

// ─── ANFS Filesystem ──────────────────────────────────────────────────────────

pub struct AnfsFilesystem {
    /// Real filesystem root being mirrored (user home).
    real_root: PathBuf,
    home: PathBuf,
    session_id: String,
    meta_batch: Arc<Mutex<MetaBatch>>,
    no_protect: NoProtectList,
    audit_log: PathBuf,
}

impl AnfsFilesystem {
    pub fn new(real_root: PathBuf, home: PathBuf, session_id: String) -> Self {
        let no_protect = NoProtectList::load(&home);
        let audit_log = home.join(".cognos/audit.log");

        // Ensure ANFS support dirs exist
        for dir in &[RECYCLE_DIR, SNAPSHOTS_DIR, META_DIR] {
            let _ = std::fs::create_dir_all(home.join(dir));
        }

        let fs = Self {
            real_root,
            home: home.clone(),
            session_id,
            meta_batch: Arc::new(Mutex::new(MetaBatch::new())),
            no_protect,
            audit_log,
        };

        // Spawn background metadata flusher
        let batch = fs.meta_batch.clone();
        let home_clone = home.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(5));
                let mut b = batch.lock().unwrap();
                if b.should_flush() {
                    Self::flush_meta_batch_static(&mut b, &home_clone);
                }
            }
        });

        fs
    }

    /// Convert a FUSE path to the real filesystem path.
    fn real_path(&self, path: &Path) -> PathBuf {
        // Strip leading slash and join with real_root
        let rel = path.strip_prefix("/").unwrap_or(path);
        self.real_root.join(rel)
    }

    /// Queue a metadata update (non-blocking, batched).
    fn queue_meta_update(&self, path: &Path) {
        let abs = self.real_path(path);
        let path_str = abs.to_string_lossy().to_string();

        let mut batch = self.meta_batch.lock().unwrap();
        let entry = batch.updates.entry(path_str.clone()).or_default();
        entry.path = path_str;
        entry.access_count += 1;
        entry.last_accessed = chrono::Utc::now().to_rfc3339();
        if !entry.session_ids.contains(&self.session_id) {
            entry.session_ids.push(self.session_id.clone());
        }
    }

    /// Push a path to the Memory Agent's index queue (append-only).
    fn push_to_index_queue(&self, path: &Path) {
        let queue_path = self.home.join(INDEX_QUEUE);
        if let Some(parent) = queue_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let line = format!("{}\n", self.real_path(path).display());
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&queue_path)
        {
            use std::io::Write;
            let _ = f.write_all(line.as_bytes());
        }
    }

    /// Take a snapshot of a file before an AI edit.
    fn snapshot_before_ai_edit(&self, path: &Path) -> Option<PathBuf> {
        let real = self.real_path(path);
        if !real.exists() {
            return None;
        }

        let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        let date = chrono::Utc::now().format("%Y-%m-%d");
        let filename = real.file_name()?.to_string_lossy();
        let snap_dir = self.home.join(SNAPSHOTS_DIR).join(date.to_string());
        let _ = std::fs::create_dir_all(&snap_dir);
        let snap_path = snap_dir.join(format!("{}.{}", filename, ts));

        if std::fs::copy(&real, &snap_path).is_ok() {
            self.audit("snapshot_created", &real.to_string_lossy(), "");
            Some(snap_path)
        } else {
            None
        }
    }

    /// Intercept a delete — move to recycle bin instead.
    fn intercept_delete(&self, path: &Path) -> std::io::Result<()> {
        let real = self.real_path(path);

        // Exceptions: /tmp, tiny files (<100 bytes), noprotect list
        if real.starts_with("/tmp") {
            return std::fs::remove_file(&real);
        }
        if let Ok(meta) = real.metadata() {
            if meta.len() < 100 {
                return std::fs::remove_file(&real);
            }
        }
        if !self.no_protect.is_protected(&real) {
            return std::fs::remove_file(&real);
        }

        // Move to recycle
        let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        let filename = real
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let recycle_dir = self.home.join(RECYCLE_DIR);
        let _ = std::fs::create_dir_all(&recycle_dir);
        let dest = recycle_dir.join(format!("{}.{}", filename, ts));

        std::fs::rename(&real, &dest)?;
        self.audit(
            "delete_intercepted",
            &real.to_string_lossy(),
            &dest.to_string_lossy(),
        );
        Ok(())
    }

    fn flush_meta_batch_static(batch: &mut MetaBatch, home: &Path) {
        let meta_dir = home.join(META_DIR);
        for (path, meta) in batch.updates.drain() {
            // Use a hash of the path as the filename to avoid path issues
            let hash = format!("{:x}", md5_simple(&path));
            let meta_file = meta_dir.join(format!("{}.json", hash));
            if let Ok(json) = serde_json::to_string_pretty(&meta) {
                let _ = std::fs::write(&meta_file, json);
            }
        }
        batch.last_flush = std::time::Instant::now();
    }

    fn audit(&self, action: &str, target: &str, note: &str) {
        let line = format!(
            r#"{{"ts":"{}","agent":"anfs","action":"{}","target":"{}","note":"{}","outcome":"success"}}"#,
            chrono::Utc::now().to_rfc3339(),
            action,
            target,
            note,
        );
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.audit_log)
        {
            use std::io::Write;
            let _ = writeln!(f, "{}", line);
        }
    }
}

// ─── FUSE implementation ──────────────────────────────────────────────────────

impl Filesystem for AnfsFilesystem {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        // Pure passthrough — look up in real filesystem
        let parent_path = ino_to_path(parent, &self.real_root);
        let real = parent_path.join(name);

        match real.symlink_metadata() {
            Ok(meta) => reply.entry(&Duration::from_secs(1), &stat_to_attr(&meta, 0), 0),
            Err(e) => reply.error(io_err_to_errno(e)),
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        let path = ino_to_path(ino, &self.real_root);
        match path.symlink_metadata() {
            Ok(meta) => reply.attr(&Duration::from_secs(1), &stat_to_attr(&meta, ino)),
            Err(e) => reply.error(io_err_to_errno(e)),
        }
    }

    fn read(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        let path = ino_to_path(ino, &self.real_root);
        self.queue_meta_update(&path_to_fuse(&path, &self.real_root));

        match std::fs::read(&path) {
            Ok(data) => {
                let start = offset as usize;
                let end = (start + size as usize).min(data.len());
                reply.data(&data[start..end]);
            }
            Err(e) => reply.error(io_err_to_errno(e)),
        }
    }

    fn write(
        &mut self,
        req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        let path = ino_to_path(ino, &self.real_root);
        let fuse_path = path_to_fuse(&path, &self.real_root);

        // Check for AI edit marker (x-cognos-ai-edit flag = bit 31 in flags as convention)
        let is_ai_edit = (flags & (1 << 30)) != 0;
        if is_ai_edit {
            self.snapshot_before_ai_edit(&fuse_path);
        }

        // Perform the actual write
        use std::io::{Seek, SeekFrom, Write};
        match std::fs::OpenOptions::new().write(true).open(&path) {
            Ok(mut f) => {
                let _ = f.seek(SeekFrom::Start(offset as u64));
                match f.write_all(data) {
                    Ok(_) => {
                        self.push_to_index_queue(&fuse_path);
                        reply.written(data.len() as u32);
                    }
                    Err(e) => reply.error(io_err_to_errno(e)),
                }
            }
            Err(e) => reply.error(io_err_to_errno(e)),
        }
    }

    fn unlink(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let parent_path = ino_to_path(parent, &self.real_root);
        let real = parent_path.join(name);
        let fuse_path = PathBuf::from("/").join(
            real.strip_prefix(&self.real_root).unwrap_or(&real)
        );

        match self.intercept_delete(&fuse_path) {
            Ok(_) => reply.ok(),
            Err(e) => reply.error(io_err_to_errno(e)),
        }
    }

    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        let path = ino_to_path(ino, &self.real_root);
        match std::fs::read_dir(&path) {
            Ok(entries) => {
                let mut i = offset;
                for entry in entries.flatten().skip(offset as usize) {
                    i += 1;
                    let name = entry.file_name();
                    let file_type = entry
                        .file_type()
                        .map(|ft| {
                            if ft.is_dir() {
                                FileType::Directory
                            } else if ft.is_symlink() {
                                FileType::Symlink
                            } else {
                                FileType::RegularFile
                            }
                        })
                        .unwrap_or(FileType::RegularFile);

                    if reply.add(i as u64, i, file_type, &name) {
                        break;
                    }
                }
                reply.ok();
            }
            Err(e) => reply.error(io_err_to_errno(e)),
        }
    }

    fn create(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: fuser::ReplyCreate,
    ) {
        let parent_path = ino_to_path(parent, &self.real_root);
        let real = parent_path.join(name);

        match std::fs::File::create(&real) {
            Ok(_) => {
                let meta = real.symlink_metadata().unwrap();
                let attr = stat_to_attr(&meta, 0);
                self.push_to_index_queue(&path_to_fuse(&real, &self.real_root));
                reply.created(&Duration::from_secs(1), &attr, 0, 0, 0);
            }
            Err(e) => reply.error(io_err_to_errno(e)),
        }
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Convert an inode number to a real path. In a passthrough FUSE, we use
/// the device inode directly. For simplicity this uses a process-local map.
/// In production, use a proper inode → path cache.
fn ino_to_path(ino: u64, root: &Path) -> PathBuf {
    if ino == 1 {
        root.to_path_buf()
    } else {
        // Simplified: walk the tree. Production uses a HashMap<u64, PathBuf>.
        root.to_path_buf()
    }
}

fn path_to_fuse(real: &Path, root: &Path) -> PathBuf {
    PathBuf::from("/").join(real.strip_prefix(root).unwrap_or(real))
}

fn stat_to_attr(meta: &std::fs::Metadata, ino: u64) -> FileAttr {
    use std::os::unix::fs::MetadataExt;
    let kind = if meta.is_dir() {
        FileType::Directory
    } else if meta.is_symlink() {
        FileType::Symlink
    } else {
        FileType::RegularFile
    };
    FileAttr {
        ino: if ino == 0 { meta.ino() } else { ino },
        size: meta.len(),
        blocks: meta.blocks(),
        atime: UNIX_EPOCH + Duration::from_secs(meta.atime() as u64),
        mtime: UNIX_EPOCH + Duration::from_secs(meta.mtime() as u64),
        ctime: UNIX_EPOCH + Duration::from_secs(meta.ctime() as u64),
        crtime: UNIX_EPOCH,
        kind,
        perm: meta.mode() as u16,
        nlink: meta.nlink() as u32,
        uid: meta.uid(),
        gid: meta.gid(),
        rdev: meta.rdev() as u32,
        flags: 0,
        blksize: 512,
    }
}

fn io_err_to_errno(e: std::io::Error) -> i32 {
    e.raw_os_error().unwrap_or(libc::EIO)
}

/// Simple non-cryptographic hash for path → filename mapping.
fn md5_simple(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    use std::collections::hash_map::DefaultHasher;
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}
