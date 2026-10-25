//! Короткие команды через Unix-сокет. Молчащий клиент не задерживает
//! остальных: чтение неблокирующее, время и размер запроса ограничены.

use std::io::{self, Read};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use crate::ui::{Msg, Panel};

/// Стабильный адрес для всех процессов пользователя. Запасной каталог
/// принадлежит пользователю и закрыт для остальных, как XDG_RUNTIME_DIR.
pub fn socket_path() -> io::Result<std::path::PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let uid = std::fs::metadata("/proc/self")?.uid();
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from);
    runtime_socket(
        runtime.as_deref(),
        &std::path::PathBuf::from(format!("/run/user/{uid}")),
        &std::path::PathBuf::from(format!("/tmp/mimshel-runtime-{uid}")),
        uid,
    )
}

fn runtime_socket(
    runtime: Option<&std::path::Path>,
    standard: &std::path::Path,
    fallback: &std::path::Path,
    uid: u32,
) -> io::Result<std::path::PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    let dir = if let Some(dir) = runtime.filter(|p| p.is_absolute()) {
        dir
    } else if standard.try_exists()? {
        standard
    } else {
        match std::fs::DirBuilder::new().mode(0o700).create(fallback) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        fallback
    };
    let meta = std::fs::symlink_metadata(dir)?;
    if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "каталог сокета {} должен принадлежать пользователю и иметь права 0700",
                dir.display()
            ),
        ));
    }
    Ok(dir.join(format!("{}.sock", mimshel_core::names::APP)))
}

const MAX_CLIENTS: usize = 32;
/// Команда с путём к папке (`folder ПУТЬ`) бывает длинной.
const MAX_COMMAND: usize = 4096;
const DEADLINE: Duration = Duration::from_millis(250);

struct Client {
    stream: UnixStream,
    bytes: Vec<u8>,
    started: Instant,
}

impl Client {
    fn new(stream: UnixStream) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream,
            bytes: Vec::new(),
            started: Instant::now(),
        })
    }

    /// Вернуть true, если ждём оставшуюся часть команды.
    fn poll(&mut self, tx: &Sender<Msg>) -> bool {
        if self.started.elapsed() >= DEADLINE {
            return false;
        }
        let mut buf = [0; MAX_COMMAND];
        match self.stream.read(&mut buf) {
            Ok(0) => false,
            Ok(n) => {
                let end = buf[..n].iter().position(|&b| b == b'\n');
                let count = end.unwrap_or(n);
                if self.bytes.len() + count > MAX_COMMAND {
                    return false;
                }
                self.bytes.extend_from_slice(&buf[..count]);
                if end.is_some() {
                    if let Some(msg) = std::str::from_utf8(&self.bytes).ok().and_then(command) {
                        let _ = tx.send(msg);
                    }
                    return false;
                }
                self.bytes.len() < MAX_COMMAND
            }
            Err(e) => matches!(
                e.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ),
        }
    }
}

/// Команда службе: окно (`search`, `apps`) или `folder ПУТЬ`.
fn command(text: &str) -> Option<Msg> {
    if let Some(path) = text.strip_prefix("folder ") {
        let path = std::path::PathBuf::from(path);
        return path.is_absolute().then_some(Msg::OpenFolder(path));
    }
    Panel::parse(text.trim()).map(Msg::Toggle)
}

fn poll(listener: &UnixListener, clients: &mut Vec<Client>, tx: &Sender<Msg>) -> io::Result<()> {
    // Ограничиваем и число соединений, и работу за один проход.
    for _ in clients.len()..MAX_CLIENTS {
        match listener.accept() {
            Ok((stream, _)) => clients.push(Client::new(stream)?),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    clients.retain_mut(|client| client.poll(tx));
    Ok(())
}

fn serve(listener: UnixListener, tx: Sender<Msg>) -> io::Result<()> {
    let mut clients = Vec::new();
    loop {
        // В простое спим на accept, не будим службу сто раз в секунду.
        if clients.is_empty() {
            listener.set_nonblocking(false)?;
            let (stream, _) = match listener.accept() {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                result => result?,
            };
            clients.push(Client::new(stream)?);
            listener.set_nonblocking(true)?;
        }
        poll(&listener, &mut clients, &tx)?;
        if !clients.is_empty() {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

pub fn start(listener: UnixListener, tx: Sender<Msg>) -> io::Result<()> {
    listener.set_nonblocking(true)?;
    std::thread::spawn(move || {
        if let Err(e) = serve(listener, tx) {
            eprintln!("Ошибка сокета команд: {e}");
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::mpsc;

    #[test]
    fn fallback_is_stable_private_and_rejects_symlinks() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
        let uid = std::fs::metadata("/proc/self").unwrap().uid();
        let root = std::env::temp_dir().join(format!("shell-runtime-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let standard = root.join("missing");
        let fallback = root.join("private");
        let first = runtime_socket(None, &standard, &fallback, uid).unwrap();
        assert_eq!(
            first,
            runtime_socket(None, &standard, &fallback, uid).unwrap()
        );
        assert_eq!(first, fallback.join("mimshel.sock"));
        assert_eq!(std::fs::metadata(&fallback).unwrap().mode() & 0o777, 0o700);
        // Путь с XDG и без XDG совпадает, когда есть /run/user/UID.
        assert_eq!(
            first,
            runtime_socket(None, &fallback, &root.join("unused"), uid).unwrap()
        );
        assert_eq!(
            first,
            runtime_socket(Some(&fallback), &standard, &root.join("unused"), uid).unwrap()
        );
        let alias = root.join("alias");
        symlink(&fallback, &alias).unwrap();
        assert!(runtime_socket(None, &standard, &alias, uid).is_err());
        assert!(runtime_socket(None, &standard, &fallback, uid.wrapping_add(1)).is_err());
        std::fs::set_permissions(&fallback, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(runtime_socket(None, &standard, &fallback, uid).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn listener_accepts_commands_while_another_client_is_idle() {
        let name = format!(
            "shell-ipc-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = std::env::temp_dir().join(name);
        let listener = UnixListener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let _idle = UnixStream::connect(&path).unwrap();
        let mut active = UnixStream::connect(&path).unwrap();
        active.write_all(b"apps\n").unwrap();
        let (tx, rx) = mpsc::channel();
        let mut clients = Vec::new();
        poll(&listener, &mut clients, &tx).unwrap();
        assert!(matches!(rx.try_recv(), Ok(Msg::Toggle(Panel::Apps))));
        assert_eq!(clients.len(), 1);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn idle_client_does_not_block_another_and_fragments_work() {
        let (idle, _idle_peer) = UnixStream::pair().unwrap();
        let (active, mut peer) = UnixStream::pair().unwrap();
        let mut clients = [Client::new(idle).unwrap(), Client::new(active).unwrap()];
        let (tx, rx) = mpsc::channel();
        peer.write_all(b"spot").unwrap();
        assert!(clients[0].poll(&tx));
        assert!(clients[1].poll(&tx));
        assert!(rx.try_recv().is_err());
        peer.write_all(b"light\n").unwrap();
        assert!(clients[0].poll(&tx));
        assert!(!clients[1].poll(&tx));
        assert!(matches!(rx.try_recv(), Ok(Msg::Toggle(Panel::Search))));
    }

    #[test]
    fn rejects_oversized_invalid_and_expired_requests() {
        let (tx, rx) = mpsc::channel();
        for bytes in [
            vec![b'x'; MAX_COMMAND + 1],
            vec![255, b'\n'],
            b"unknown\n".to_vec(),
        ] {
            let (stream, mut peer) = UnixStream::pair().unwrap();
            let mut client = Client::new(stream).unwrap();
            peer.write_all(&bytes).unwrap();
            assert!(!client.poll(&tx));
        }
        let (stream, mut peer) = UnixStream::pair().unwrap();
        let mut client = Client::new(stream).unwrap();
        client.started = Instant::now() - DEADLINE;
        peer.write_all(b"apps\n").unwrap();
        assert!(!client.poll(&tx));
        assert!(rx.try_recv().is_err());
    }
}
