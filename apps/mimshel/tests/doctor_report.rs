//! Exercise the CLI in a separate process without mutating the test environment.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[test]
fn private_report_from_fake_home() {
    let root = std::env::current_dir()
        .unwrap()
        .join(format!(".doctor-report-test-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let home = root.join("reportperson");
    let state = root.join("state/mimshel");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(state.join("crashes")).unwrap();
    let host = fs::read_to_string("/proc/sys/kernel/hostname").unwrap();
    let private = format!(
        "reportperson {} {}\nSSID: private-network\n",
        home.display(),
        host.trim()
    );
    let config = home.join("config.toml");
    fs::write(
        &config,
        format!("# {}\n[look]\nscale = 1.5\n", private.replace('\n', "\n# ")),
    )
    .unwrap();
    fs::write(state.join("daemon.log"), &private).unwrap();
    fs::write(
        state.join("crashes/2026-10-26_14-44-58-service-16965.txt"),
        &private,
    )
    .unwrap();
    let mut broken = private.as_bytes().to_vec();
    broken.extend_from_slice(b"\xff\xfe");
    fs::write(state.join("crashes/invalid UTF-8.txt"), broken).unwrap();
    fs::write(state.join("crashes/crash-1.txt"), "reserved original").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mimshel"))
        .args(["doctor", "--report"])
        .env("HOME", &home)
        .env("USER", "reportperson")
        .env("XDG_STATE_HOME", root.join("state"))
        .env("MIMSHEL_CONFIG", &config)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let archive = fs::read_dir(&home)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "gz"))
        .unwrap();
    assert_eq!(
        fs::metadata(&archive).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let extracted = root.join("extracted");
    fs::create_dir(&extracted).unwrap();
    assert!(
        Command::new("tar")
            .arg("xzf")
            .arg(&archive)
            .arg("-C")
            .arg(&extracted)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        extracted
            .join("crashes/2026-10-26_14-44-58-service-16965.txt")
            .is_file()
    );
    assert_eq!(
        fs::read_to_string(extracted.join("crashes/crash-1.txt")).unwrap(),
        "reserved original"
    );
    assert!(
        fs::read_to_string(extracted.join("crashes/crash-2.txt"))
            .unwrap()
            .contains('\u{fffd}')
    );
    let mut paths = fs::read_dir(&extracted)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect::<Vec<_>>();
    paths.extend(
        fs::read_dir(extracted.join("crashes"))
            .unwrap()
            .flatten()
            .map(|e| e.path()),
    );
    for path in paths.into_iter().filter(|p| p.is_file()) {
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let text = fs::read_to_string(path).unwrap();
        for secret in [
            "reportperson",
            home.to_str().unwrap(),
            host.trim(),
            "private-network",
        ] {
            assert!(!text.contains(secret), "не удалено: {secret}");
        }
    }
    let readme = fs::read_to_string(extracted.join("README.txt")).unwrap();
    assert!(readme.contains("lock.log: missing"));
    assert!(readme.contains("uses the UI language (currently Russian)"));
    let system = fs::read_to_string(extracted.join("system.txt")).unwrap();
    assert!(system.contains("Kernel:"));
    assert!(system.contains("[look] scale (effective): 1.5"));
}
