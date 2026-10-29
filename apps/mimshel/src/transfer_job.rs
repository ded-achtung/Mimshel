//! Background transfer tracking and cancellation. No X11 calls live here.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc::Sender};
use std::time::{Duration, Instant, SystemTime};

use mimshel_folder::transfer::{Amount, Phase, Row};

use crate::ui::Msg;

static SENDER: OnceLock<Sender<Msg>> = OnceLock::new();
static NEXT: AtomicU64 = AtomicU64::new(1);

pub fn connect(tx: Sender<Msg>) {
    let _ = SENDER.set(tx);
}

struct State {
    row: Row,
    stop: bool,
    active: Option<PathBuf>,
    finished: Amount,
    copying: Option<Instant>,
}

#[derive(Clone)]
pub struct Handle {
    started: Instant,
    state: Arc<Mutex<State>>,
}

impl Handle {
    pub fn snapshot(&self) -> Row {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let mut row = state.row.clone();
        row.elapsed = self.started.elapsed();
        row.copying_for = state.copying.map_or(Duration::ZERO, |t| t.elapsed());
        row
    }

    pub fn stop(&self) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.row.stoppable() {
            state.stop = true;
            state.row.phase = Phase::Stopping;
        }
    }

    fn stopped(&self) -> bool {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).stop
    }

    /// The same lock serializes the last Stop request with committing an item.
    fn commit(&self, moving: bool) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.stop {
            return false;
        }
        state.row.phase = if moving {
            Phase::Removing
        } else {
            Phase::Finishing
        };
        true
    }
}

pub struct Job {
    pub handle: Handle,
    manifests: Vec<Option<Manifest>>,
}

impl Job {
    #[cfg(test)]
    pub fn preview(row: Row) -> Self {
        let mut job = Self::new(&[], row.moving);
        job.handle.started = Instant::now() - row.elapsed;
        {
            let mut state = job.handle.state.lock().unwrap_or_else(|p| p.into_inner());
            state.copying = Some(Instant::now() - row.copying_for);
            state.row = row;
        }
        job
    }

    pub fn new(pairs: &[(PathBuf, PathBuf)], moving: bool) -> Self {
        let handle = Handle {
            started: Instant::now(),
            state: Arc::new(Mutex::new(State {
                row: Row {
                    name: pairs
                        .first()
                        .and_then(|p| p.0.file_name())
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into(),
                    folder: pairs
                        .first()
                        .and_then(|p| p.1.parent())
                        .and_then(Path::file_name)
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into(),
                    moving,
                    phase: Phase::Counting,
                    total: None,
                    arrived: Amount::default(),
                    elapsed: Duration::ZERO,
                    copying_for: Duration::ZERO,
                },
                stop: false,
                active: None,
                finished: Amount::default(),
                copying: None,
            })),
        };
        if let Some(tx) = SENDER.get() {
            let _ = tx.send(Msg::TransferStarted(handle.clone()));
        }
        if let Some(tx) = SENDER.get().cloned() {
            let ticker = handle.clone();
            std::thread::spawn(move || {
                loop {
                    if tx.send(Msg::TransferTick).is_err() || ticker.snapshot().phase == Phase::Done
                    {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            });
        }
        let observer = handle.clone();
        std::thread::spawn(move || {
            loop {
                let (phase, active, finished) = {
                    let state = observer.state.lock().unwrap_or_else(|p| p.into_inner());
                    (state.row.phase, state.active.clone(), state.finished)
                };
                if phase == Phase::Done {
                    break;
                }
                if let Some(path) = active {
                    let arrived = measure(&path, &|| {
                        observer
                            .state
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .row
                            .phase
                            == Phase::Done
                    });
                    let mut state = observer.state.lock().unwrap_or_else(|p| p.into_inner());
                    if state.active.as_ref() == Some(&path) {
                        state.row.arrived = finished.plus(arrived);
                        if state.copying.is_none() && (arrived.bytes > 0 || arrived.files > 0) {
                            state.copying = Some(Instant::now());
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(400));
            }
        });
        Self {
            handle,
            manifests: Vec::new(),
        }
    }

    pub fn count(&mut self, pairs: &[(PathBuf, PathBuf)]) {
        let mut total = Some(Amount::default());
        for (src, _) in pairs {
            let manifest = manifest(src, &|| self.handle.stopped()).ok();
            total = total
                .zip(manifest.as_ref())
                .map(|(sum, m)| sum.plus(m.amount()));
            self.manifests.push(manifest);
            if self.handle.stopped() {
                break;
            }
        }
        let mut state = self.handle.state.lock().unwrap_or_else(|p| p.into_inner());
        state.row.total = total;
        if !state.stop {
            state.row.phase = Phase::Copying;
        }
    }

    pub fn stopped(&self) -> bool {
        self.handle.stopped()
    }

    pub fn item(&self, src: &Path, dest: &Path) {
        let mut state = self.handle.state.lock().unwrap_or_else(|p| p.into_inner());
        state.row.name = src.file_name().unwrap_or_default().to_string_lossy().into();
        state.row.folder = dest
            .parent()
            .and_then(Path::file_name)
            .unwrap_or_default()
            .to_string_lossy()
            .into();
        if !state.stop {
            state.row.phase = Phase::Copying;
        }
    }

    /// Stage local copies privately; gvfs destinations receive a direct copy
    /// because MTP cannot reliably move objects between directories.
    pub fn copy_item(
        &self,
        cmd: &mut Command,
        src: &Path,
        dest: &Path,
        index: usize,
        moving: bool,
    ) -> io::Result<bool> {
        self.copy_item_mode(
            cmd,
            src,
            dest,
            index,
            moving,
            dest.starts_with(mimshel_folder::gvfs_dir()),
        )
    }

    fn copy_item_mode(
        &self,
        cmd: &mut Command,
        src: &Path,
        dest: &Path,
        index: usize,
        moving: bool,
        direct: bool,
    ) -> io::Result<bool> {
        if self.stopped() {
            return Ok(false);
        }
        let mut direct_copy = if direct {
            Some(DirectCopy::new(dest)?)
        } else {
            None
        };
        let staging = if direct {
            None
        } else {
            Some(Staging::new(dest)?)
        };
        let partial = staging
            .as_ref()
            .map_or_else(|| dest.to_path_buf(), |s| s.path.join("item"));
        self.handle
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .active = Some(partial.clone());
        cmd.arg("--").arg(src).arg(&partial);
        if !run_child(cmd, &self.handle)? || fs::symlink_metadata(&partial).is_err() {
            return Ok(false);
        }
        let expected = self.manifests.get(index).and_then(Option::as_ref);
        if moving {
            let expected =
                expected.ok_or_else(|| io::Error::other("source inventory unavailable"))?;
            verify(src, &partial, expected, &|| self.stopped())?;
        }
        if !self.handle.commit(moving) {
            return Ok(false);
        }
        {
            let mut state = self.handle.state.lock().unwrap_or_else(|p| p.into_inner());
            state.active = None;
            state.row.arrived = state
                .finished
                .plus(expected.map_or(Amount::default(), Manifest::amount));
        }
        if staging.is_some() {
            // Publishing is a same-directory rename; --no-clobber handles races on
            // the final name. A zero status alone does not mean the rename happened.
            let status = Command::new("mv")
                .args(["--no-clobber", "-T", "--"])
                .arg(&partial)
                .arg(dest)
                .status()?;
            if !status.success()
                || fs::symlink_metadata(&partial).is_ok()
                || fs::symlink_metadata(dest).is_err()
            {
                return Ok(false);
            }
        }
        if let Some(direct_copy) = &mut direct_copy {
            direct_copy.keep = true;
        }
        if moving {
            // Stop is disabled before publishing and throughout source removal.
            remove_originals(
                src,
                expected.ok_or_else(|| io::Error::other("source inventory unavailable"))?,
            )?;
        }
        self.finished_item(index);
        Ok(true)
    }
    pub fn finished_item(&self, index: usize) {
        let mut state = self.handle.state.lock().unwrap_or_else(|p| p.into_inner());
        let amount = self
            .manifests
            .get(index)
            .and_then(Option::as_ref)
            .map_or(Amount::default(), Manifest::amount);
        state.finished = state.finished.plus(amount);
        state.row.arrived = state.finished;
        state.active = None;
        state.row.phase = Phase::Copying;
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        let mut state = self.handle.state.lock().unwrap_or_else(|p| p.into_inner());
        state.active = None;
        state.row.phase = Phase::Done;
    }
}

fn run_child(cmd: &mut Command, control: &Handle) -> io::Result<bool> {
    let mut child = cmd.spawn()?;
    loop {
        if control.stopped() {
            // Reap before cleanup: the child must never write into a removed tree.
            let _ = child.kill();
            child.wait()?;
            return Ok(false);
        }
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status.success()),
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}

struct DirectCopy {
    path: PathBuf,
    keep: bool,
}

impl DirectCopy {
    fn new(dest: &Path) -> io::Result<Self> {
        match fs::symlink_metadata(dest) {
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "destination exists",
                ));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        // The destination was checked free before copying, so the partial item
        // created here is ours. On cancellation clean only this path, never its
        // parent, existing siblings or the source. MTP needs no staging rename.
        Ok(Self {
            path: dest.to_path_buf(),
            keep: false,
        })
    }
}

impl Drop for DirectCopy {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        let result = match fs::symlink_metadata(&self.path) {
            Ok(meta) if meta.is_dir() => fs::remove_dir_all(&self.path),
            Ok(_) => fs::remove_file(&self.path),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return,
            Err(e) => Err(e),
        };
        if let Err(e) = result {
            eprintln!("Не удалить неполную копию {}: {e}", self.path.display());
        }
    }
}

fn staging_pid(name: &str) -> Option<u32> {
    let (pid, number) = name.strip_prefix(".mimshel-transfer-")?.split_once('-')?;
    let pid: u32 = canonical_number(pid)?.try_into().ok()?;
    canonical_number(number)?;
    (pid > 0).then_some(pid)
}

fn canonical_number(number: &str) -> Option<u64> {
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let value: u64 = number.parse().ok()?;
    (value > 0 && value.to_string() == number).then_some(value)
}

fn cleanup_stale(parent: &Path) {
    // Read the effective user's proc directory ownership without unsafe or a
    // fallback to uid 0. If identity cannot be established, leave leftovers alone.
    if let Ok(meta) = fs::metadata("/proc/self") {
        cleanup_stale_owned(parent, meta.uid());
    }
}

fn cleanup_stale_owned(parent: &Path, uid: u32) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(staging_pid) else {
            continue;
        };
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !meta.is_dir() || meta.uid() != uid {
            continue;
        }
        // Permission errors or PID reuse are not evidence that the process died.
        if !fs::metadata(format!("/proc/{pid}")).is_err_and(|e| e.kind() == io::ErrorKind::NotFound)
        {
            continue;
        }
        let stale = Staging {
            path,
            identity: (meta.dev(), meta.ino()),
        };
        drop(stale);
    }
}

struct Staging {
    path: PathBuf,
    identity: (u64, u64),
}

impl Staging {
    fn new(dest: &Path) -> io::Result<Self> {
        let parent = dest
            .parent()
            .ok_or_else(|| io::Error::other("no destination parent"))?;
        cleanup_stale(parent);
        loop {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(".mimshel-transfer-{}-{id}", std::process::id()));
            match fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => {
                    let meta = fs::symlink_metadata(&path)?;
                    return Ok(Self {
                        path,
                        identity: (meta.dev(), meta.ino()),
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        // Never clean a replacement at the staging path. The parent is private,
        // created atomically with mode 0700, and contains only this item's copy.
        if fs::symlink_metadata(&self.path)
            .is_ok_and(|m| (m.dev(), m.ino()) == self.identity && m.is_dir())
            && let Err(e) = cleanup_private(&self.path)
        {
            eprintln!("Не удалить временную копию {}: {e}", self.path.display());
        }
    }
}

fn cleanup_private(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() {
        return fs::remove_file(path);
    }
    // cp -a may have copied read-only directories. Only change permissions
    // inside our private destination tree, never on source files or symlinks.
    fs::set_permissions(
        path,
        fs::Permissions::from_mode(meta.permissions().mode() | 0o700),
    )?;
    for entry in fs::read_dir(path)? {
        cleanup_private(&entry?.path())?;
    }
    fs::remove_dir(path)
}

fn remove_originals(root: &Path, expected: &Manifest) -> io::Result<()> {
    // Children sort after their parent. Remove only inventoried entries and use
    // rmdir for directories, so a newly added file is never recursively deleted.
    for (relative, entry) in expected.0.iter().rev() {
        let path = if relative.as_os_str().is_empty() {
            root.to_path_buf()
        } else {
            root.join(relative)
        };
        let meta = fs::symlink_metadata(&path)?;
        let same = (meta.dev(), meta.ino()) == entry.identity;
        if !same
            || (entry.kind != 0
                && (meta.modified().ok() != entry.modified
                    || (entry.kind == 1 && meta.len() != entry.size)))
        {
            return Err(io::Error::other("source changed before removal"));
        }
        if entry.kind == 0 {
            fs::remove_dir(path)?;
        } else {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    identity: (u64, u64),
    kind: u8,
    size: u64,
    modified: Option<SystemTime>,
    link: Option<PathBuf>,
}

#[derive(Debug, PartialEq, Eq)]
struct Manifest(BTreeMap<PathBuf, Entry>);

impl Manifest {
    fn amount(&self) -> Amount {
        self.0.values().fold(Amount::default(), |sum, entry| {
            sum.plus(Amount {
                bytes: if entry.kind == 1 { entry.size } else { 0 },
                files: u64::from(entry.kind != 0),
            })
        })
    }
}

fn cancelled(stop: &impl Fn() -> bool) -> io::Result<()> {
    if stop() {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "transfer stopped",
        ))
    } else {
        Ok(())
    }
}

fn manifest(root: &Path, stop: &impl Fn() -> bool) -> io::Result<Manifest> {
    let mut entries = BTreeMap::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        cancelled(stop)?;
        let path = if relative.as_os_str().is_empty() {
            root.to_path_buf()
        } else {
            root.join(&relative)
        };
        let meta = fs::symlink_metadata(&path)?;
        let kind = if meta.is_dir() {
            0
        } else if meta.is_file() {
            1
        } else if meta.is_symlink() {
            2
        } else {
            3
        };
        if kind == 0 {
            for entry in fs::read_dir(&path)? {
                cancelled(stop)?;
                pending.push(relative.join(entry?.file_name()));
            }
        }
        let link = if kind == 2 {
            Some(fs::read_link(&path)?)
        } else {
            None
        };
        entries.insert(
            relative,
            Entry {
                identity: (meta.dev(), meta.ino()),
                kind,
                size: if kind == 1 { meta.len() } else { 0 },
                modified: meta.modified().ok(),
                link,
            },
        );
    }
    Ok(Manifest(entries))
}

fn verify(
    src: &Path,
    dest: &Path,
    expected: &Manifest,
    stop: &impl Fn() -> bool,
) -> io::Result<()> {
    // cp's successful status plus an exact inventory of types, lengths and link
    // targets verifies the whole item. Source changes prevent source deletion.
    if manifest(src, stop)? != *expected {
        return Err(io::Error::other("source changed during transfer"));
    }
    let actual = manifest(dest, stop)?;
    if actual.0.len() != expected.0.len()
        || !expected.0.iter().all(|(path, entry)| {
            actual.0.get(path).is_some_and(|a| {
                a.kind == entry.kind && a.size == entry.size && a.link == entry.link
            })
        })
    {
        return Err(io::Error::other("destination inventory differs"));
    }
    Ok(())
}

fn measure(root: &Path, stop: &impl Fn() -> bool) -> Amount {
    let mut total = Amount::default();
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        if stop() {
            break;
        }
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.flatten() {
                    if stop() {
                        return total;
                    }
                    pending.push(entry.path());
                }
            }
        } else {
            total = total.plus(Amount {
                bytes: if meta.is_file() { meta.len() } else { 0 },
                files: 1,
            });
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let root =
                std::env::temp_dir().join(format!("mimshel-progress-{}-{id}", std::process::id()));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn join(&self, path: &str) -> PathBuf {
            self.0.join(path)
        }
        fn staging(&self) -> Vec<PathBuf> {
            fs::read_dir(&self.0)
                .unwrap()
                .filter_map(|e| {
                    let path = e.unwrap().path();
                    path.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with(".mimshel-transfer-")
                        .then_some(path)
                })
                .collect()
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            cleanup_private(&self.0).unwrap();
        }
    }

    fn copy() -> Command {
        let mut cmd = Command::new("cp");
        cmd.args(["--no-clobber", "-T", "-a"]);
        cmd
    }

    fn stopped_transfer(moving: bool, direct: bool) {
        let temp = Temp::new();
        let src = temp.join("DCIM");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("one"), "original one").unwrap();
        fs::write(src.join("sub/two"), "original two").unwrap();
        let before = manifest(&src, &|| false).unwrap();
        let dest = temp.join("destination");
        let existing = temp.join("existing");
        fs::write(&existing, "do not touch").unwrap();
        let finished = temp.join("finished");
        let first_src = temp.join("first");
        fs::write(&first_src, "finished item").unwrap();
        let pairs = vec![
            (first_src.clone(), finished.clone()),
            (src.clone(), dest.clone()),
        ];
        let mut job = Job::new(&pairs, moving);
        job.count(&pairs);
        assert!(
            job.copy_item(&mut copy(), &first_src, &finished, 0, moving)
                .unwrap()
        );
        let handle = job.handle.clone();
        let worker = std::thread::spawn(move || {
            let mut slow = Command::new("sh");
            // Shell builtins keep the fake child alive without descendants.
            slow.args([
                "-c",
                "mkdir -- \"$2\"; printf partial > \"$2/incomplete\"; while :; do :; done",
            ]);
            let result = job
                .copy_item_mode(&mut slow, &src, &dest, 1, moving, direct)
                .unwrap();
            assert!(job.stopped());
            result
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if temp.join("destination/incomplete").exists()
                || temp
                    .staging()
                    .iter()
                    .any(|p| p.join("item/incomplete").exists())
            {
                break;
            }
            assert!(Instant::now() < deadline, "fake copy did not start");
            std::thread::sleep(Duration::from_millis(10));
        }
        if direct {
            assert!(temp.staging().is_empty());
            assert_eq!(
                handle.state.lock().unwrap().active.as_ref(),
                Some(&temp.join("destination"))
            );
            while handle.snapshot().arrived.files < 2 {
                assert!(Instant::now() < deadline, "direct copy was not measured");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        handle.stop();
        assert!(!worker.join().unwrap());
        assert_eq!(manifest(&temp.join("DCIM"), &|| false).unwrap(), before);
        assert_eq!(fs::read_to_string(existing).unwrap(), "do not touch");
        assert_eq!(fs::read_to_string(finished).unwrap(), "finished item");
        assert!(!temp.join("destination").exists());
        assert!(temp.staging().is_empty());
        assert_eq!(first_src.exists(), !moving);
    }

    #[test]
    fn stop_removes_only_current_private_copy() {
        stopped_transfer(false, false);
    }

    #[test]
    fn stopped_two_phase_move_preserves_all_current_sources() {
        // Force the cross-device path directly; both paths may be on one device.
        stopped_transfer(true, false);
    }

    #[test]
    fn successful_two_phase_move_verifies_and_removes_sources() {
        let temp = Temp::new();
        let src = temp.join("source");
        fs::create_dir_all(src.join("empty")).unwrap();
        fs::write(src.join("file"), "contents").unwrap();
        std::os::unix::fs::symlink("file", src.join("link")).unwrap();
        let dest = temp.join("destination");
        let pairs = vec![(src.clone(), dest.clone())];
        let mut job = Job::new(&pairs, true);
        job.count(&pairs);
        assert!(job.copy_item(&mut copy(), &src, &dest, 0, true).unwrap());
        assert!(!src.exists());
        assert_eq!(fs::read_to_string(dest.join("file")).unwrap(), "contents");
        assert_eq!(
            fs::read_link(dest.join("link")).unwrap(),
            PathBuf::from("file")
        );
        assert!(dest.join("empty").is_dir());
        assert!(temp.staging().is_empty());
    }

    #[test]
    fn failed_copy_keeps_source_and_cleans_partial() {
        let temp = Temp::new();
        let src = temp.join("source");
        let dest = temp.join("destination");
        fs::write(&src, "original").unwrap();
        let pairs = vec![(src.clone(), dest.clone())];
        let mut job = Job::new(&pairs, true);
        job.count(&pairs);
        let mut failure = Command::new("sh");
        failure.args(["-c", "printf partial > \"$2\"; exit 1"]);
        assert!(!job.copy_item(&mut failure, &src, &dest, 0, true).unwrap());
        assert_eq!(fs::read_to_string(src).unwrap(), "original");
        assert!(!dest.exists());
        assert!(temp.staging().is_empty());
    }

    #[test]
    fn verification_failure_never_removes_source() {
        let temp = Temp::new();
        let src = temp.join("source");
        let dest = temp.join("destination");
        fs::write(&src, "original").unwrap();
        let pairs = vec![(src.clone(), dest.clone())];
        let mut job = Job::new(&pairs, true);
        job.count(&pairs);
        let mut short = Command::new("sh");
        short.args(["-c", "printf x > \"$2\""]);
        assert!(job.copy_item(&mut short, &src, &dest, 0, true).is_err());
        assert_eq!(fs::read_to_string(src).unwrap(), "original");
        assert!(!dest.exists());
        assert!(temp.staging().is_empty());
    }

    #[test]
    fn concurrent_destination_is_preserved() {
        let temp = Temp::new();
        let src = temp.join("source");
        let dest = temp.join("destination");
        fs::write(&src, "original").unwrap();
        let pairs = vec![(src.clone(), dest.clone())];
        let mut job = Job::new(&pairs, true);
        job.count(&pairs);
        let mut racing = Command::new("sh");
        racing
            .args([
                "-c",
                "cp -a -- \"$1\" \"$2\"; printf rival > \"$RACE_DEST\"",
            ])
            .env("RACE_DEST", &dest);
        assert!(!job.copy_item(&mut racing, &src, &dest, 0, true).unwrap());
        assert_eq!(fs::read_to_string(src).unwrap(), "original");
        assert_eq!(fs::read_to_string(dest).unwrap(), "rival");
        assert!(temp.staging().is_empty());
    }

    #[test]
    fn removal_and_stop_are_serialized() {
        let temp = Temp::new();
        let pairs = vec![(temp.join("source"), temp.join("dest"))];
        let job = Job::new(&pairs, true);
        assert!(job.handle.commit(true));
        job.handle.stop();
        assert!(!job.stopped());
        assert_eq!(job.handle.snapshot().phase, Phase::Removing);
        let other = Job::new(&pairs, true);
        other.handle.stop();
        assert!(!other.handle.commit(true));
        assert_eq!(other.handle.snapshot().phase, Phase::Stopping);
    }

    #[test]
    fn counting_stops_and_symlinks_are_not_followed() {
        let temp = Temp::new();
        let src = temp.join("source");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("file"), "abc").unwrap();
        fs::create_dir(src.join("empty")).unwrap();
        std::os::unix::fs::symlink(".", src.join("loop")).unwrap();
        let inventory = manifest(&src, &|| false).unwrap();
        assert_eq!(inventory.amount(), Amount { bytes: 3, files: 2 });
        assert_eq!(
            manifest(&src, &|| true).unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
        let pairs = vec![(src.clone(), temp.join("dest"))];
        let mut job = Job::new(&pairs, false);
        job.handle.stop();
        job.count(&pairs);
        assert!(job.stopped());
        assert_eq!(job.handle.snapshot().phase, Phase::Stopping);
        assert!(temp.staging().is_empty());
    }
    #[test]
    fn cleanup_handles_readonly_directories_without_following_symlinks() {
        use std::os::unix::fs::PermissionsExt;
        let temp = Temp::new();
        let existing = temp.join("existing");
        fs::create_dir(&existing).unwrap();
        fs::write(existing.join("keep"), "untouched").unwrap();
        let mode = fs::metadata(&existing).unwrap().permissions().mode();
        let staging = Staging::new(&temp.join("destination")).unwrap();
        fs::create_dir(staging.path.join("item")).unwrap();
        fs::write(staging.path.join("item/partial"), "partial").unwrap();
        std::os::unix::fs::symlink(&existing, staging.path.join("item/external")).unwrap();
        fs::set_permissions(staging.path.join("item"), fs::Permissions::from_mode(0o500)).unwrap();
        drop(staging);
        assert!(temp.staging().is_empty());
        assert_eq!(
            fs::read_to_string(existing.join("keep")).unwrap(),
            "untouched"
        );
        assert_eq!(fs::metadata(existing).unwrap().permissions().mode(), mode);
    }

    #[test]
    fn source_changes_prevent_source_removal() {
        let temp = Temp::new();
        let src = temp.join("source");
        let dest = temp.join("destination");
        fs::write(&src, "original").unwrap();
        let pairs = vec![(src.clone(), dest.clone())];
        let mut job = Job::new(&pairs, true);
        job.count(&pairs);
        let mut changing = Command::new("sh");
        changing.args([
            "-c",
            "cp -a -- \"$1\" \"$2\"; printf changed-and-longer > \"$1\"",
        ]);
        assert!(job.copy_item(&mut changing, &src, &dest, 0, true).is_err());
        assert_eq!(fs::read_to_string(src).unwrap(), "changed-and-longer");
        assert!(!dest.exists());
        assert!(temp.staging().is_empty());
    }
    #[test]
    fn removal_keeps_files_added_after_verification() {
        let temp = Temp::new();
        let src = temp.join("source");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("copied"), "copied").unwrap();
        let inventory = manifest(&src, &|| false).unwrap();
        fs::write(src.join("new"), "not copied").unwrap();
        assert!(remove_originals(&src, &inventory).is_err());
        assert_eq!(fs::read_to_string(src.join("new")).unwrap(), "not copied");
        assert!(!src.join("copied").exists());
    }
    #[test]
    fn direct_copy_stop_removes_only_current_final_path() {
        stopped_transfer(false, true);
    }

    #[test]
    fn direct_move_stop_keeps_sources() {
        stopped_transfer(true, true);
    }

    #[test]
    fn direct_copy_and_move_need_no_staging_or_rename() {
        for moving in [false, true] {
            let temp = Temp::new();
            let src = temp.join("source");
            fs::create_dir(&src).unwrap();
            fs::write(src.join("photo"), "image data").unwrap();
            let dest = temp.join("destination");
            let pairs = vec![(src.clone(), dest.clone())];
            let mut job = Job::new(&pairs, moving);
            job.count(&pairs);
            let mut phone_copy = Command::new("sh");
            // Reject a staging path before executing the same cp -r command.
            phone_copy.args(["-c", "test \"$2\" = \"$EXPECTED_DEST\" || exit 1; exec cp --no-clobber -T -r -- \"$1\" \"$2\""])
                .env("EXPECTED_DEST", &dest);
            assert!(
                job.copy_item_mode(&mut phone_copy, &src, &dest, 0, moving, true)
                    .unwrap()
            );
            assert_eq!(
                fs::read_to_string(dest.join("photo")).unwrap(),
                "image data"
            );
            assert_eq!(src.exists(), !moving);
            assert!(temp.staging().is_empty());
        }
    }

    #[test]
    fn direct_copy_failure_and_failed_verification_keep_sources() {
        for succeeds in [false, true] {
            let temp = Temp::new();
            let src = temp.join("source");
            let dest = temp.join("destination");
            fs::write(&src, "original").unwrap();
            let pairs = vec![(src.clone(), dest.clone())];
            let mut job = Job::new(&pairs, true);
            job.count(&pairs);
            let mut command = Command::new("sh");
            command.args([
                "-c",
                if succeeds {
                    "printf x > \"$2\""
                } else {
                    "printf x > \"$2\"; exit 1"
                },
            ]);
            let result = job.copy_item_mode(&mut command, &src, &dest, 0, true, true);
            if succeeds {
                assert!(result.is_err());
            } else {
                assert!(!result.unwrap());
            }
            assert_eq!(fs::read_to_string(src).unwrap(), "original");
            assert!(!dest.exists());
            assert!(temp.staging().is_empty());
        }
    }

    #[test]
    fn direct_copy_rejects_existing_destination() {
        let temp = Temp::new();
        let src = temp.join("source");
        let dest = temp.join("destination");
        fs::write(&src, "source").unwrap();
        fs::write(&dest, "existing").unwrap();
        let pairs = vec![(src.clone(), dest.clone())];
        let mut job = Job::new(&pairs, true);
        job.count(&pairs);
        assert_eq!(
            job.copy_item_mode(&mut copy(), &src, &dest, 0, true, true)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read_to_string(src).unwrap(), "source");
        assert_eq!(fs::read_to_string(dest).unwrap(), "existing");
    }

    #[test]
    fn poisoned_state_still_supports_snapshot_counting_and_stop() {
        let temp = Temp::new();
        let src = temp.join("source");
        fs::write(&src, "data").unwrap();
        let dest = temp.join("destination");
        let pairs = vec![(src.clone(), dest.clone())];
        let mut job = Job::new(&pairs, false);
        let handle = job.handle.clone();
        assert!(
            mimshel_core::crash::catch(|| {
                let _guard = handle.state.lock().unwrap();
                panic!("deliberately poison test state");
            })
            .is_err()
        );
        assert!(handle.state.is_poisoned());
        job.count(&pairs);
        job.item(&src, &dest);
        assert_eq!(handle.snapshot().total, Some(Amount { bytes: 4, files: 1 }));
        handle.stop();
        assert!(job.stopped());
        assert!(!handle.commit(false));
        drop(job);
        assert_eq!(handle.snapshot().phase, Phase::Done);
    }

    #[test]
    fn staging_name_requires_the_exact_generated_pattern() {
        assert_eq!(staging_pid(".mimshel-transfer-123-456"), Some(123));
        for name in [
            "mimshel-transfer-123-456",
            ".mimshel-transfer-123-456-old",
            ".mimshel-transfer-123-456.txt",
            ".mimshel-transfer--456",
            ".mimshel-transfer-123-",
            ".mimshel-transfer-0-1",
            ".mimshel-transfer-123-0",
            ".mimshel-transfer-00123-1",
            ".mimshel-transfer-123-+1",
            ".mimshel-transfer-123-１",
            ".mimshel-transfer-4294967296-1",
        ] {
            assert_eq!(staging_pid(name), None, "{name}");
        }
    }

    #[test]
    fn new_staging_cleans_only_dead_owned_directories_with_exact_names() {
        let temp = Temp::new();
        let dead_pid = u32::MAX;
        assert!(!Path::new(&format!("/proc/{dead_pid}")).exists());
        let stale = temp.join(&format!(".mimshel-transfer-{dead_pid}-1"));
        fs::create_dir(&stale).unwrap();
        fs::write(stale.join("partial"), "old partial").unwrap();
        let live = temp.join(&format!(".mimshel-transfer-{}-123456", std::process::id()));
        fs::create_dir(&live).unwrap();
        fs::write(live.join("active"), "active").unwrap();
        let lookalike = temp.join(&format!(".mimshel-transfer-{dead_pid}-2-backup"));
        fs::create_dir(&lookalike).unwrap();
        let file = temp.join(&format!(".mimshel-transfer-{dead_pid}-3"));
        fs::write(&file, "ordinary file").unwrap();
        let external = temp.join("external");
        fs::create_dir(&external).unwrap();
        fs::write(external.join("keep"), "keep").unwrap();
        let link = temp.join(&format!(".mimshel-transfer-{dead_pid}-4"));
        std::os::unix::fs::symlink(&external, &link).unwrap();
        std::os::unix::fs::symlink(&external, stale.join("external")).unwrap();
        let uid = fs::metadata("/proc/self").unwrap().uid();
        // Force the ownership mismatch without requiring chown privileges.
        cleanup_stale_owned(&temp.0, uid.checked_add(1).unwrap());
        assert!(stale.exists());
        let new = Staging::new(&temp.join("destination")).unwrap();
        assert!(!stale.exists());
        assert!(new.path.is_dir());
        assert_eq!(fs::read_to_string(live.join("active")).unwrap(), "active");
        assert!(lookalike.is_dir());
        assert_eq!(fs::read_to_string(file).unwrap(), "ordinary file");
        assert!(fs::symlink_metadata(link).unwrap().is_symlink());
        assert_eq!(fs::read_to_string(external.join("keep")).unwrap(), "keep");
        drop(new);
        assert!(live.exists());
    }
}
