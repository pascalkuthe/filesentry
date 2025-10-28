use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use filesentry::{EventType, Filter, Watcher};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use log::error;

struct Ignore {
    hidden: bool,
    ignores: Vec<Gitignore>,
}

const HELP: &str = r#"
Watch the files in a directory (recursively) for changes.

Usage: filesentry [OPTIONS] [dir]

Arguments:
  [dir]
      The directory to watch.

Options:
  -H, --hidden
          Include changes from hidden directories and files in (default: hidden files and
          directories are skipped). Files and directories are considered to be hidden if their
          name starts with a `.` sign (dot). Any files or directories that are ignored due to the
          rules described by --no-ignore are still ignored unless otherwise specified.

  -I, --no-ignore
          Show changes from files and directories that would otherwise be ignored by
          '.gitignore', '.ignore', '.fdignore', or the global ignore file, The flag can be
          overridden with --ignore.

  -R, --no-recurse
          Show search results from files and directories that would otherwise be ignored by
          '.gitignore', '.ignore', '.fdignore', or the global ignore file, The flag can be
          overridden with --ignore.
"#;
fn parse_args() -> Result<(PathBuf, Ignore), lexopt::Error> {
    use lexopt::prelude::*;

    let _ = env_logger::builder().try_init();
    let mut no_ignore = false;
    let mut hidden = false;
    // let mut extra_ignores = Vec::new();
    let mut parser = lexopt::Parser::from_env();
    let mut root = None;
    while let Some(arg) = parser.next()? {
        match arg {
            Short('H') | Long("hidden") => {
                hidden = true;
            }
            Short('I') | Long("no-ignore") => {
                no_ignore = true;
            }
            Long("help") => {
                println!("{HELP}");
                std::process::exit(0);
            }
            Value(root_) if root.is_none() => root = Some(PathBuf::from(root_)),
            _ => return Err(arg.unexpected()),
        }
    }
    let root = root
        .map(Ok)
        .unwrap_or_else(std::env::current_dir)
        .map_err(|err| lexopt::Error::Custom(Box::new(err)))?;
    let mut ignores = Vec::new();
    if !no_ignore {
        let (global, errors) = Gitignore::global();
        if let Some(errs) = errors {
            error!("invalid global .gitignore: {errs}");
        }
        ignores.push(global);
        for parent in root.ancestors() {
            let mut builder = None;
            for path in [".ignore", ".gitignore"] {
                let path = parent.join(path);
                if path.exists() {
                    builder
                        .get_or_insert_with(|| GitignoreBuilder::new(parent))
                        .add(path);
                }
            }
            if let Some(builder) = builder.take() {
                match builder.build() {
                    Ok(ig) => ignores.push(ig),
                    Err(err) => error!("invalid ignores at {parent:?} {err}"),
                }
            }
        }
    }
    Ok((root, Ignore { hidden, ignores }))
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|it| it.as_encoded_bytes().first() == Some(&b'.'))
}

impl Filter for Ignore {
    fn ignore_path(&self, path: &Path, is_dir: Option<bool>) -> bool {
        match is_dir {
            Some(is_dir) => {
                for ignore in &self.ignores {
                    match ignore.matched(path, is_dir) {
                        ignore::Match::None => continue,
                        ignore::Match::Ignore(_) => return true,
                        ignore::Match::Whitelist(_) => return false,
                    }
                }
            }
            None => {
                // if we don't know wether this is a directory (on windows)
                // then we are conservative and allow the dirs
                for ignore in &self.ignores {
                    match ignore.matched(path, true) {
                        ignore::Match::None => continue,
                        ignore::Match::Ignore(glob) => {
                            if glob.is_only_dir() {
                                match ignore.matched(path, false) {
                                    ignore::Match::None => (),
                                    ignore::Match::Ignore(_) => return true,
                                    ignore::Match::Whitelist(_) => return false,
                                }
                            } else {
                                return true;
                            }
                        }
                        ignore::Match::Whitelist(_) => return false,
                    }
                }
            }
        }
        !self.hidden && is_hidden(path)
    }
}

pub fn main() -> Result<(), lexopt::Error> {
    let (root, ignore) = parse_args()?;
    let _ = env_logger::builder().try_init();
    let watcher = Watcher::new().unwrap();
    watcher
        .add_root(&root, true, |_| ())
        .map_err(|err| lexopt::Error::Custom(Box::new(err)))?;

    watcher.set_filter(Arc::new(ignore), false);
    watcher.add_handler(|events| {
        for event in &*events {
            match event.ty {
                EventType::Create => println!("{:?} create", event.path),
                EventType::Delete => println!("{:?} delete", event.path),
                EventType::Modified => println!("{:?} modify", event.path),
                EventType::Tempfile => println!("{:?} tempfile", event.path),
            }
        }
        true
    });
    watcher.start();
    std::thread::sleep(Duration::from_secs(60 * 60));
    Ok(())
}
