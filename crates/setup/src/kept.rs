//! The last build of the program that is known to start: a copy kept in
//! `~/.local/state/mimshel/kept-build/`. The service guard keeps the copy
//! once a service has run for a while, and starts it when a newly installed
//! build keeps falling right after its start — instead of leaving the user
//! without a menu bar and a dock until somebody fixes the build.
//!
//! "Works" here means "starts and stays up for a minute", nothing more.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The folder of the kept build inside the state folder.
const DIR: &str = "kept-build";
/// The note that names the installed build which fell at its start.
const BAD: &str = "bad";

/// Which build a file is: its size and the time it was written. `install`
/// writes a new file for every deploy, so a new build has a new time; the
/// kept copy carries the time of the file it was copied from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    len: u64,
    modified: Duration,
}

impl Identity {
    /// `None` — no such file (or it cannot be read).
    pub fn of(path: &Path) -> Option<Identity> {
        let meta = std::fs::metadata(path).ok()?;
        Some(Identity {
            len: meta.len(),
            modified: meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?,
        })
    }

    fn parse(text: &str) -> Option<Identity> {
        let mut words = text.split_whitespace();
        let len = words.next()?.parse().ok()?;
        let secs = words.next()?.parse().ok()?;
        let nanos = words.next()?.parse().ok()?;
        if nanos >= 1_000_000_000 || words.next().is_some() {
            return None;
        }
        Some(Identity {
            len,
            modified: Duration::new(secs, nanos),
        })
    }

    fn text(self) -> String {
        format!(
            "{} {} {}\n",
            self.len,
            self.modified.as_secs(),
            self.modified.subsec_nanos()
        )
    }
}

/// Where the kept build lives. One per user: `dir` is the program's state
/// folder (`~/.local/state/mimshel`).
#[derive(Debug, Clone)]
pub struct Kept {
    dir: PathBuf,
}

impl Kept {
    /// In the real state folder. `None` — no home folder known.
    pub fn new() -> Option<Kept> {
        mimshel_core::crash::state_dir().map(|state| Kept::in_dir(&state))
    }

    pub fn in_dir(state: &Path) -> Kept {
        Kept {
            dir: state.join(DIR),
        }
    }

    /// The kept program. It keeps the program's own file name, so everything
    /// that finds the service by its command line still finds it.
    pub fn exe(&self) -> PathBuf {
        self.dir.join(mimshel_core::names::APP)
    }

    fn bad(&self) -> PathBuf {
        self.dir.join(BAD)
    }

    /// Keep `running` — the file a service that stayed up was started from
    /// (`/proc/PID/exe`: the very file, even after a newer build replaced it
    /// on disk). `Ok(true)` — a copy was made, `Ok(false)` — this build is
    /// kept already. The note about a bad build goes away: the installed
    /// build works.
    pub fn keep(&self, running: &Path) -> std::io::Result<bool> {
        let id = Identity::of(running).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "the program file is gone")
        })?;
        let _ = std::fs::remove_file(self.bad());
        let exe = self.exe();
        if Identity::of(&exe) == Some(id) {
            return Ok(false);
        }
        std::fs::create_dir_all(&self.dir)?;
        // Through a temporary name: a copy cut short (a full disk, the
        // session ending) must never stand where the kept build is looked for.
        let tmp = exe.with_extension("tmp");
        let copied = (|| {
            std::fs::copy(running, &tmp)?;
            let file = std::fs::OpenOptions::new().write(true).open(&tmp)?;
            file.set_modified(SystemTime::UNIX_EPOCH + id.modified)?;
            file.sync_all()?;
            std::fs::rename(&tmp, &exe)
        })();
        if copied.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        copied.map(|()| true)
    }

    /// The kept build, when it can stand in for `installed`: there is one and
    /// it is another build. The same build would fall the same way.
    pub fn stand_in(&self, installed: &Path) -> Option<PathBuf> {
        let exe = self.exe();
        let kept = Identity::of(&exe)?;
        (Identity::of(installed) != Some(kept)).then_some(exe)
    }

    /// Remember that `installed` fell at its start while the kept build ran
    /// fine: the next login starts the kept build at once.
    pub fn mark_bad(&self, installed: Identity) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        std::fs::write(self.bad(), installed.text())
    }

    /// Is `installed` the build noted as bad? A newer file at the same path
    /// is another build and gets its own try.
    pub fn is_bad(&self, installed: &Path) -> bool {
        let noted = std::fs::read_to_string(self.bad())
            .ok()
            .and_then(|text| Identity::parse(&text));
        noted.is_some() && noted == Identity::of(installed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder with an "installed" program in it.
    fn setup(tag: &str) -> (PathBuf, PathBuf, Kept) {
        let root = std::env::temp_dir().join(format!("mimshel-kept-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let installed = root.join("installed");
        write_build(&installed, b"build one", 1_000);
        let kept = Kept::in_dir(&root.join("state"));
        (root, installed, kept)
    }

    /// Like `install`: a new file with its own time.
    fn write_build(path: &Path, bytes: &[u8], secs: u64) {
        let _ = std::fs::remove_file(path);
        std::fs::write(path, bytes).unwrap();
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(UNIX_EPOCH + Duration::from_secs(secs))
            .unwrap();
    }

    #[test]
    fn keeps_a_build_once() {
        let (root, installed, kept) = setup("once");
        assert_eq!(kept.stand_in(&installed), None, "nothing kept yet");
        assert!(kept.keep(&installed).unwrap());
        assert_eq!(std::fs::read(kept.exe()).unwrap(), b"build one");
        assert_eq!(Identity::of(&kept.exe()), Identity::of(&installed));
        assert!(!kept.keep(&installed).unwrap(), "the same build again");
        assert_eq!(
            kept.stand_in(&installed),
            None,
            "the kept copy of the installed build is no way out"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn stands_in_for_a_newer_build() {
        let (root, installed, kept) = setup("newer");
        kept.keep(&installed).unwrap();
        write_build(&installed, b"build two", 2_000);
        assert_eq!(kept.stand_in(&installed), Some(kept.exe()));
        assert_eq!(std::fs::read(kept.exe()).unwrap(), b"build one");
        // Same size, another time: still another build.
        write_build(&installed, b"build 222", 3_000);
        assert_eq!(kept.stand_in(&installed), Some(kept.exe()));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_bad_note_is_about_one_build() {
        let (root, installed, kept) = setup("bad");
        kept.keep(&installed).unwrap();
        write_build(&installed, b"build two", 2_000);
        assert!(!kept.is_bad(&installed));
        kept.mark_bad(Identity::of(&installed).unwrap()).unwrap();
        assert!(kept.is_bad(&installed));
        // A fixed build installed over it gets its own try.
        write_build(&installed, b"build three", 3_000);
        assert!(!kept.is_bad(&installed));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn keeping_a_working_build_clears_the_note() {
        let (root, installed, kept) = setup("clear");
        kept.keep(&installed).unwrap();
        write_build(&installed, b"build two", 2_000);
        kept.mark_bad(Identity::of(&installed).unwrap()).unwrap();
        assert!(kept.keep(&installed).unwrap(), "it ran after all");
        assert!(!kept.is_bad(&installed));
        assert_eq!(std::fs::read(kept.exe()).unwrap(), b"build two");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_missing_program_is_an_error_and_keeps_the_old_copy() {
        let (root, installed, kept) = setup("missing");
        kept.keep(&installed).unwrap();
        assert!(kept.keep(&root.join("no-such-file")).is_err());
        assert_eq!(std::fs::read(kept.exe()).unwrap(), b"build one");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn identity_text_round_trip() {
        let id = Identity {
            len: 18_471_168,
            modified: Duration::new(1_794_000_000, 123_456_789),
        };
        assert_eq!(Identity::parse(&id.text()), Some(id));
        for broken in ["", "1 2", "1 2 3 4", "x 2 3", "1 2 1000000000"] {
            assert_eq!(Identity::parse(broken), None, "{broken:?}");
        }
    }
}
