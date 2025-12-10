use crate::indexer::{build_full_index, index_one, is_indexed_file, remove_if_tracked};
use crate::store::IndexStore;
use anyhow::{Context, Result};
use log::{debug, info, warn};
use notify::event::{ModifyKind, RenameMode};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

pub fn run(root: &Path, db_path: &Path) -> Result<()> {
    let root = root
        .canonicalize()
        .with_context(|| format!("failed to canonicalize root {}", root.display()))?;
    info!("Opening index at {}", db_path.display());
    let store = IndexStore::open(db_path)?;

    build_full_index(&root, &store)?;

    let (tx, rx) = mpsc::channel();
    let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |res| {
        if tx.send(res).is_err() {
            eprintln!("watcher channel closed");
        }
    })?;
    watcher.watch(&root, RecursiveMode::Recursive)?;

    info!("Watching {} for changes (TypeScript files)", root.display());
    loop {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Ok(event)) => {
                if let Err(err) = handle_event(&root, &store, event) {
                    warn!("failed to handle event: {err:#}");
                }
            }
            Ok(Err(err)) => warn!("watch error: {err}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // continue loop to keep watcher alive
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    Ok(())
}

fn handle_event(root: &Path, store: &IndexStore, event: Event) -> Result<()> {
    let paths: Vec<PathBuf> = event
        .paths
        .into_iter()
        .filter_map(|p| normalize_event_path(root, p))
        .collect();

    match event.kind {
        EventKind::Modify(ModifyKind::Name(RenameMode::From)) | EventKind::Remove(_) => {
            for path in paths {
                remove_if_tracked(&path, store)?;
            }
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::To))
        | EventKind::Create(_)
        | EventKind::Modify(_) => {
            for path in paths {
                if is_indexed_file(&path) && path.is_file() {
                    index_one(&path, store)?;
                }
            }
        }
        _ => debug!("ignoring event {:?}", event.kind),
    }
    Ok(())
}

fn normalize_event_path(root: &Path, path: PathBuf) -> Option<PathBuf> {
    if path.is_absolute() {
        Some(path)
    } else {
        Some(root.join(path))
    }
}
