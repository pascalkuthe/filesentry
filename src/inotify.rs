use std::sync::atomic::{self, AtomicBool};
use std::sync::Arc;
use std::{io, thread};

mod sys;

use hashbrown::DefaultHashBuilder;
use mio::{Poll, Waker};
use papaya::HashMap;

use crate::inotify::sys::{Event, EventFlags, Inotify, Watch};
use crate::path::CanonicalPathBuf;
use crate::pending::{self, PendingChangesLock};
use crate::{Filter, WatcherState};

pub(crate) struct InotifyWatcher {
    waker: mio::Waker,
    shutdown: AtomicBool,
    notify: Inotify,
    watches: HashMap<Watch, CanonicalPathBuf, DefaultHashBuilder>,
    pub changes: PendingChangesLock,
}

impl std::fmt::Debug for InotifyWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InotifyWatcher")
            .field("waker", &self.waker)
            .field("shutdown", &self.shutdown)
            .field("notify", &self.notify)
            .field("watches", &self.watches)
            .field("changes", &self.changes)
            .finish_non_exhaustive()
    }
}

impl InotifyWatcher {
    pub fn shutdown(&self) {
        self.shutdown.store(true, atomic::Ordering::Relaxed);
        let _ = self.waker.wake();
        self.changes.notify();
    }

    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(atomic::Ordering::Relaxed)
    }

    pub fn new(#[cfg(test)] slow: bool, state: Arc<WatcherState>) -> io::Result<Arc<Self>> {
        let mut poll = Poll::new()?;
        let waker = Waker::new(poll.registry(), sys::MESSAGE)?;
        let watcher = Arc::new(Self {
            waker,
            notify: Inotify::new()?,
            watches: HashMap::with_capacity_and_hasher(1024, DefaultHashBuilder::default()),
            changes: PendingChangesLock::default(),
            shutdown: AtomicBool::new(false),
        });
        let mut filter = state.config.lock().unwrap().filter.clone();

        let watcher_ = watcher.clone();
        thread::spawn(move || {
            watcher_.notify.event_loop(
                &mut poll,
                &mut filter,
                |filter, event /* , timestamp */| {
                    watcher_.handle_event(event, &**filter /* , timestamp */)
                },
                |_| {
                    watcher_.changes.notify();
                },
                |filter| {
                    *filter = state.config.lock().unwrap().filter.clone();
                    watcher_.is_shutdown()
                },
                #[cfg(test)]
                slow,
            )
        });
        Ok(watcher)
    }

    pub fn watch_dir(&self, path: CanonicalPathBuf) -> io::Result<()> {
        let watch = self.notify.add_directory_watch(&*path)?;
        self.watches.pin().insert(watch, path);
        Ok(())
    }

    pub fn refresh_config(&self) {
        let _ = self.waker.wake();
    }

    fn handle_event(&self, event: Event, filter: &dyn Filter) {
        // need to recrawl everything anyway if the queue overflowed
        if event.flags.contains(EventFlags::QUEUE_OVERFLOW) {
            self.changes.lock().recrawl();
            return;
        }
        let watches = self.watches.pin();
        let Some(dir) = watches.get(&event.wd) else {
            if event.wd.is_invalid()
                || event
                    .flags
                    .intersects(EventFlags::MOVE_SELF | EventFlags::IGNORED)
            {
                self.changes.lock().recrawl();
            }
            return;
        };

        let watch_deleted = event.flags.intersects(
            EventFlags::IGNORED
                | EventFlags::MOVE_SELF
                | EventFlags::DELETE_SELF
                | EventFlags::UNMOUNT,
        );
        if event.child.is_empty() || watch_deleted {
            if event.flags.contains(EventFlags::IGNORED) {
                watches.remove(&event.wd);
            }
            let path = dir.clone();
            self.changes.lock().add_watcher(
                path,
                /* timestamp, */ pending::Flags::NEEDS_RECURSIVE_CRAWL,
            );
        } else {
            let path = dir.join(event.child);
            if filter.ignore_path(
                path.as_std_path(),
                Some(event.flags.contains(EventFlags::ISDIR)),
            ) {
                return;
            }
            let mut pending = self.changes.lock();
            if event
                .flags
                .intersects(EventFlags::CREATE | EventFlags::DELETE)
            {
                pending.add_watcher(
                    path,
                    /* timestamp, */ pending::Flags::NEEDS_RECURSIVE_CRAWL,
                );
            } else {
                pending.add_watcher(path, /* timestamp, */ pending::Flags::empty());
            }
        }
    }
}
