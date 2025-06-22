use std::hash::BuildHasher;
use std::mem::{swap, take};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::Duration;

use bitflags::bitflags;
use hashbrown::hash_table::Entry;
use hashbrown::{DefaultHashBuilder, HashTable};

use crate::path::CannonicalPathBuf;

bitflags! {
    #[derive(Clone, Copy, Debug)]
    pub struct Flags: u32 {
        /// for directories: do a recursive crawl
        const NEEDS_RECURSIVE_CRAWL = 1;
        /// for directories: do a non-recursive crawl, some watchers (fsevent)
        /// only report that *something* changed in a dricetory but not what,
        /// it's our job to stat.
        const NEEDS_NON_RECURSIVE_CRAWL = 2;
        /// flag set during recursive watch to indicate that nodes should be marked recursive
        const MARK_RECURSIVE = 4;
        /// change originated from a watcher
        const ORIGIN_WATCHER = 8;
    }
}

#[derive(Debug, Default)]
pub struct PendingChangesLock {
    inner: Mutex<PendingChanges>,
    condvar: Condvar,
}

impl PendingChangesLock {
    pub fn take_timeout(
        &self,
        dst: &mut PendingChanges,
        timeout: Duration,
        exit: impl Fn() -> bool,
    ) -> bool {
        let mut guard = self.inner.lock().unwrap();
        let res;
        (guard, res) = self
            .condvar
            .wait_timeout_while(guard, timeout, |changes| changes.is_empty() && !exit())
            .unwrap();
        if res.timed_out() {
            return true;
        }
        swap(&mut *guard, dst);
        false
    }

    pub fn take(&self, dst: &mut PendingChanges, exit: impl Fn() -> bool) {
        let mut guard = self.inner.lock().unwrap();
        guard = self
            .condvar
            .wait_while(guard, |changes| changes.is_empty() && !exit())
            .unwrap();
        swap(&mut *guard, dst);
    }

    pub fn lock(&self) -> MutexGuard<'_, PendingChanges> {
        self.inner.lock().unwrap()
    }

    pub fn notify(&self) {
        self.condvar.notify_all();
    }
}

#[derive(Clone, Debug)]
pub struct PendingChange {
    pub path: CannonicalPathBuf,
    pub flags: Flags,
}

impl PendingChange {
    fn consolidate(&mut self, mut new: Flags) {
        // TODO: is this really  needed
        new.remove(Flags::ORIGIN_WATCHER);
        self.flags.insert(new);
    }
}

#[derive(Default, Clone)]
pub struct PendingChanges {
    path_set: HashTable<u32>,
    state: DefaultHashBuilder,
    changes: Vec<PendingChange>,
    recrawl: bool,
}

impl std::fmt::Debug for PendingChanges {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingChanges")
            .field("changes", &self.changes)
            .field("recrawl", &self.recrawl)
            .finish()
    }
}

impl PendingChanges {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty() & !self.recrawl
    }

    // pub fn remove(&mut self, path: impl AsRef<OsStr>) -> bool {
    //     let path = path.as_ref();
    //     let hash = self.state.hash_one(path);
    //     let ent = self
    //         .path_set
    //         .find_entry(hash, |&i| self.changes[i as usize].path == path);
    //     match ent {
    //         Ok(ent) => {
    //             ent.remove();
    //             true
    //         }
    //         Err(_) => false,
    //     }
    // }

    pub fn recrawl(&mut self) {
        self.path_set.clear();
        self.changes.clear();
        self.recrawl = true;
    }

    fn add(&mut self, change: PendingChange) {
        if self.recrawl {
            return;
        }
        let hash = self.state.hash_one(&change.path);
        let ent = self.path_set.entry(
            hash,
            |&i| self.changes[i as usize].path == change.path,
            |&i| self.state.hash_one(&self.changes[i as usize].path),
        );
        match ent {
            Entry::Occupied(entry) => {
                self.changes[*entry.get() as usize].consolidate(change.flags);
            }
            Entry::Vacant(entry) => {
                entry.insert(self.changes.len() as u32);
                self.changes.push(change);
            }
        }
    }

    pub fn add_watcher(
        &mut self,
        path: CannonicalPathBuf,
        /* timestamp: SystemTime, */ flags: Flags,
    ) {
        self.add(PendingChange {
            path,
            // timestamp,
            flags: flags | Flags::ORIGIN_WATCHER,
        });
    }

    pub fn take_recrawl(&mut self) -> bool {
        take(&mut self.recrawl)
    }

    pub fn drain(&mut self) -> impl Iterator<Item = PendingChange> + '_ {
        self.path_set.clear();
        self.changes
            .sort_unstable_by(|change1, change2| change1.path.cmp(&change2.path));
        self.changes.drain(..)
    }
}
