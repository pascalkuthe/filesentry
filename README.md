# Filesentry

<p align="center">
  <img src="filesentry.png" width="300px">
</p>

Filesentry is a rust library  that enables reliably watching files (potenitally recrusively) for changes. It goes to great lengths to ensure no events are missed/dropped. Filsentry is intentionally limited to watching whether **files** are: modified, created or deleted. More detailed events are intentionally not recorded as they tend to be missleading. For example a write from applications like vim that first write to a temporary file can show up as a move, with file sentry it shows up as a `Modify` event.

## Details 


To achieve reliability filesentry will buildup an in-memory view of the filesystem by running a recursive crawl when it starts watching a directory. When a filewatcher event occurs filesentry will examine (stat) the respective path and compare it to the in-memory view. This will serve as the source of truth for whether the entry was created/deleted/modified (or turned into a directory). If a new directory is created (or it's inode number changed) the directory is also crawled again.

When an event is record it is first aggregated and merged in an internal collection. There events are merged until the file-watcher has settled (no events for N milliseconds). This ensures that duplicate events are removed and for example means that replacing file A with file B turns into a `Delete` event for `file A` and a `Modify` event for `file B`.

On all platforms the native file-watching APIs involve one or multiple queues (often in the kernel but sometimes in userspace aswell). If the system is heavily loaded these queues can overflow which in most filewatchers leads to missed events and often desynchronized states. Filesentry solves this by triggering a recursive recrawls in tis case. As we keep a copy of the file-tree in memory we can always compare the stat information read during a crawl with the in-memory representation and generate events based on the differences.


This approach is heavily inspired by `watchman` which is the most sophisticated/reliable filewatcher I am aware of.

## Backends

Currently, only linux with `inotify` is supported but support for mac with `fsevent` and windows is planned. 
