// use pretty_assertions::assert_eq;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, LazyLock, Mutex};
use std::time::Duration;

use tempfile::TempDir;

use crate::events::EventType;
use crate::Watcher;

static TIMEOUT: LazyLock<Duration> =
    LazyLock::new(|| match std::env::var("FILESENTRY_TEST_TIMEOUT") {
        Ok(res) if !res.trim().is_empty() => Duration::from_secs(
            res.parse()
                .expect("invalid value for FILESENTRY_TEST_TIMEOUT expected an integer"),
        ),
        _ => Duration::from_secs(20),
    });

pub static READ_DELAY: LazyLock<Duration> =
    LazyLock::new(|| match std::env::var("FILESENTRY_READ_DELAY") {
        Ok(res) if !res.trim().is_empty() => Duration::from_millis(
            res.parse()
                .expect("invalid value for FILESENTRY_TEST_TIMEOUT expected an integer"),
        ),
        _ => Duration::from_millis(300),
    });

struct Assertion {
    done: mpsc::Receiver<()>,
    state: Arc<Mutex<Vec<(PathBuf, EventType)>>>,
    expected: Vec<(PathBuf, EventType)>,
}

impl Assertion {
    pub fn new<'a>(
        watcher: &Watcher,
        dir: &Path,
        expected: impl IntoIterator<Item = (&'a str, EventType)>,
    ) -> Assertion {
        let mut expected: Vec<_> = expected
            .into_iter()
            .map(|(path, event)| (dir.join(path), event))
            .collect();
        expected.sort_unstable();
        let state: Arc<Mutex<_>> = Arc::default();
        let (tx, rx) = mpsc::sync_channel(1);

        let len = expected.len();
        let assertion = Assertion {
            done: rx,
            state: state.clone(),
            expected,
        };

        watcher.add_handler(move |events| {
            if Arc::strong_count(&state) == 1 {
                return false;
            }
            let mut state = state.lock().unwrap();
            state.extend(
                events
                    .iter()
                    .map(|event| (event.path.as_std_path().to_owned(), event.ty)),
            );
            if state.len() >= len {
                let _ = tx.send(());
                false
            } else {
                true
            }
        });
        assertion
    }

    #[track_caller]
    pub fn check(self) {
        let timeout = self.done.recv_timeout(*TIMEOUT).is_err();
        self.state.clear_poison();
        let mut state = self.state.lock().unwrap();
        state.sort_unstable();
        if timeout {
            panic!("watcher didn't observer all changes within the timeout")
        } else {
            assert_eq!(&*state, &*self.expected)
        }
    }
}

fn rm_dir(dst: &Path, path: &str) {
    fs::remove_dir(dst.join(path)).unwrap();
}

fn rm_file(dst: &Path, path: &str) {
    fs::remove_file(dst.join(path)).unwrap();
}

fn write(dst: &Path, path: &str, content: &str) {
    fs::write(dst.join(path), content).unwrap();
}

fn mk_write(dst: &Path, path: &str, content: &str) {
    let path = dst.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn init_watcher() -> (TempDir, Watcher) {
    init_watcher_imp(false)
}

fn init_watcher_slow() -> (TempDir, Watcher) {
    init_watcher_imp(true)
}

fn init_watcher_imp(slow: bool) -> (TempDir, Watcher) {
    let _ = env_logger::builder().try_init();
    let dir = TempDir::new().unwrap();
    let watcher = Watcher::new_impl(slow).unwrap();
    let (tx, rx) = mpsc::sync_channel(1);
    watcher
        .add_root(dir.path(), true, move |success| {
            let _ = tx.send(success);
        })
        .unwrap();
    watcher.start();
    rx.recv_timeout(*TIMEOUT).expect("failed to start watcher");
    (dir, watcher)
}
fn with_watcher(f: impl FnOnce(&Path, &Watcher)) {
    let (dir, watcher) = init_watcher();
    let shutdown_guard = watcher.shutdown_guard();
    f(dir.path(), &watcher);
    drop(shutdown_guard)
}

fn with_watcher_slow(f: impl FnOnce(&Path, &Watcher)) {
    let (dir, watcher) = init_watcher_slow();
    let shutdown_guard = watcher.shutdown_guard();
    f(dir.path(), &watcher);
    drop(shutdown_guard)
}

#[test]
fn create() {
    with_watcher(|dir, watcher| {
        let assertion = Assertion::new(
            watcher,
            dir,
            [
                ("foo/baz", EventType::Create),
                ("foo/bar/baz", EventType::Create),
                ("baz", EventType::Create),
            ],
        );
        mk_write(dir, "baz", "foo");
        mk_write(dir, "foo/baz", "foo");
        mk_write(dir, "foo/bar/baz", "foo");
        assertion.check();
    });
}

#[test]
fn delete() {
    with_watcher(|dir, watcher| {
        let assertion = Assertion::new(
            watcher,
            dir,
            [
                ("foo/baz", EventType::Create),
                ("foo/bar/baz", EventType::Create),
                ("baz", EventType::Create),
            ],
        );
        mk_write(dir, "baz", "foo");
        mk_write(dir, "foo/baz", "foo");
        mk_write(dir, "foo/bar/baz", "foo");
        assertion.check();
        let assertion = Assertion::new(watcher, dir, [("foo/bar/baz", EventType::Delete)]);
        rm_file(dir, "foo/bar/baz");
        rm_dir(dir, "foo/bar");
        assertion.check();
        let assertion = Assertion::new(watcher, dir, [("baz", EventType::Delete)]);
        rm_file(dir, "baz");
        assertion.check();
    });
}

#[test]
fn modify() {
    with_watcher(|dir, watcher| {
        let assertion = Assertion::new(
            watcher,
            dir,
            [
                ("foo/baz", EventType::Create),
                ("foo/bar/baz", EventType::Create),
                ("baz", EventType::Create),
            ],
        );
        mk_write(dir, "baz", "content1");
        mk_write(dir, "foo/baz", "content1");
        mk_write(dir, "foo/bar/baz", "content1");
        assertion.check();
        let assertion = Assertion::new(watcher, dir, [("foo/bar/baz", EventType::Modified)]);
        write(dir, "foo/bar/baz", "content2");
        assertion.check();
        let assertion = Assertion::new(watcher, dir, [("foo/baz", EventType::Modified)]);
        write(dir, "foo/baz", "content2");
        assertion.check();
        let assertion = Assertion::new(watcher, dir, [("baz", EventType::Modified)]);
        rm_file(dir, "baz");
        write(dir, "baz", "content3");
        assertion.check();
    });
}

#[test]
fn queue_overflow() {
    with_watcher_slow(|dir, watcher| {
        let files: Vec<_> = (0..20_0000)
            .map(|i| format!("foo{}/bar{i}", i % 200))
            .collect();
        let assertion = Assertion::new(
            watcher,
            dir,
            files.iter().map(|file| (&**file, EventType::Create)),
        );
        for file in &files {
            mk_write(dir, file, "content1");
        }
        assertion.check();
        let assertion = Assertion::new(
            watcher,
            dir,
            files.iter().map(|file| (&**file, EventType::Modified)),
        );
        for file in &files {
            write(dir, file, "content2");
        }
        assertion.check();
        let recrawls = watcher.recrawls();
        assert!(
            recrawls >= 2,
            "expected atleast 2 recrawls but found {recrawls}"
        )
    });
}
