# FileSentry

<p align="center">
  <img src="filesentry.png" width="300px">
</p>

FileSentry is a rust library  that enables reliably watching files (potentially recursively) for changes. It goes to great lengths to ensure no events are missed/dropped. FileSentry is intentionally limited to watching whether **files** are: modified, created or deleted. More detailed events are intentionally not recorded as they tend to be misleading. For example a write from applications like Vim that first write to a temporary file can show up as a move, with file sentry it shows up as a `Modify` event.

## Details 


To achieve reliability FileSentry will buildup an in-memory view of the filesystem by running a recursive crawl when it starts watching a directory. When a file watcher event occurs FileSentry will examine (stat) the respective path and compare it to the in-memory view. This will serve as the source of truth for whether the entry was created/deleted/modified (or turned into a directory). If a new directory is created (or its inode number changed) the directory is also crawled again.

When an event is record it is first aggregated and merged in an internal collection. The events are merged until the file-watcher has settled (no events for N milliseconds). This ensures that duplicate events are removed and for example means that replacing file A with file B turns into a `Delete` event for `file A` and a `Modify` event for `file B`.

On all platforms the native file-watching APIs involve one or multiple queues (often in the kernel but sometimes in user space as well). If the system is heavily loaded these queues can overflow which in most file watchers leads to missed events and often de-synchronized states. FileSentry solves this by triggering a recursive re-crawls in this case. As we keep a copy of the file-tree in memory we can always compare the stat information read during a crawl with the in-memory representation and generate events based on the differences.


This approach is heavily inspired by `watchman` which is the most sophisticated/reliable file watcher I am aware of.

## Backends

Currently, only Linux with `inotify` is supported but support for macOS with `fsevent` and Windows is planned.
