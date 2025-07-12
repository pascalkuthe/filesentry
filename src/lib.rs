use std::io;
use std::path::Path;
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{self, AtomicBool};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use crate::config::Config;
use crate::events::EventDebouncer;
pub use crate::events::{EventType, Events};
use crate::inotify::InotifyWatcher;
pub use crate::path::{CannonicalPath, CanonicalPathBuf};
use crate::worker::Worker;
pub use config::Filter;

mod config;
mod events;
mod inotify;
mod metadata;
mod path;
mod pending;
#[cfg(test)]
mod tests;
mod tree;
mod worker;

struct AddRoot {
    path: CanonicalPathBuf,
    recursive: bool,
    notify: Box<dyn FnOnce(bool) + Send>,
}

#[derive(Default)]
struct Notifications {
    /// new roots to be added to the watcher
    roots: Vec<AddRoot>,
}

impl std::fmt::Debug for Notifications {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Notifications").finish_non_exhaustive()
    }
}

#[derive(Debug)]
struct WatcherState {
    config: Mutex<Config>,
    notifications: Mutex<Notifications>,
    has_notifications: AtomicBool,
    #[cfg(test)]
    recrawls: AtomicUsize,
}

pub struct ShutdownOnDrop {
    watcher: Weak<InotifyWatcher>,
}

impl ShutdownOnDrop {
    pub fn cancel(&mut self) {
        self.watcher = Weak::new()
    }
}

impl Drop for ShutdownOnDrop {
    fn drop(&mut self) {
        if let Some(watcher) = self.watcher.upgrade() {
            watcher.shutdown();
        }
    }
}

#[derive(Debug, Clone)]
pub struct Watcher {
    state: Arc<WatcherState>,
    notify: Arc<InotifyWatcher>,
}

impl Watcher {
    #[cfg(test)]
    pub fn recrawls(&self) -> usize {
        self.state.recrawls.load(atomic::Ordering::Relaxed)
    }

    pub fn shutdown(&self) {
        self.notify.shutdown();
    }

    pub fn shutdown_guard(&self) -> ShutdownOnDrop {
        ShutdownOnDrop {
            watcher: Arc::downgrade(&self.notify),
        }
    }

    pub fn add_root(
        &self,
        root: &Path,
        recursive: bool,
        root_crawled: impl FnOnce(bool) + 'static + Send,
    ) -> io::Result<()> {
        let root = root.canonicalize()?;
        if self
            .state
            .config
            .lock()
            .unwrap()
            .filter
            .ignore_path_rec(&root, None)
        {
            log::warn!("ignoring root {root:?} as it matches the ignore pattern");
            return Ok(());
        }
        let root = CanonicalPathBuf::assert_canonicalized(&root);
        self.state
            .notifications
            .lock()
            .unwrap()
            .roots
            .push(AddRoot {
                path: root,
                recursive,
                notify: Box::new(root_crawled),
            });
        self.state
            .has_notifications
            .store(true, atomic::Ordering::Relaxed);
        self.notify.changes.notify();
        Ok(())
    }

    pub fn set_filter(&self, filter: Arc<dyn Filter>, recrawl: bool) {
        self.state.config.lock().unwrap().filter = filter;
        self.notify.refresh_config();
        if recrawl {
            self.notify.changes.lock().recrawl();
            self.notify.changes.notify();
        }
    }

    pub fn set_settle_time(&self, settle_time: Duration) {
        self.state.config.lock().unwrap().settle_time = settle_time;
    }

    pub fn add_handler(&self, handler: impl FnMut(Events) -> bool + Send + 'static) {
        self.state
            .config
            .lock()
            .unwrap()
            .handlers
            .push(Box::new(handler));
    }

    pub fn new() -> io::Result<Self> {
        Self::new_impl(false)
    }

    pub fn new_impl(_slow: bool) -> io::Result<Self> {
        let state = Arc::new(WatcherState {
            config: Mutex::new(Config {
                filter: Arc::new(()),
                settle_time: Duration::from_millis(200),
                handlers: Vec::new(),
            }),
            notifications: Mutex::new(Notifications::default()),
            has_notifications: AtomicBool::new(false),
            #[cfg(test)]
            recrawls: AtomicUsize::new(0),
        });
        #[cfg(test)]
        let watcher = InotifyWatcher::new(_slow, state.clone())?;
        #[cfg(not(test))]
        let watcher = InotifyWatcher::new(state.clone())?;

        Ok(Self {
            state,
            notify: watcher,
        })
    }

    pub fn start(&self) {
        let watcher = self.clone();
        std::thread::spawn(move || {
            let worker = Worker::new(watcher);
            worker.run();
        });
    }
}
