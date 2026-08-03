//! Generating a fixture when somebody asks for it.
//!
//! Building the whole catalogue up front means a two minute wait before
//! anything is usable, and most of that work is for fixtures nobody is about to
//! watch. Generating on demand makes the first request slow and every later one
//! instant, which is the right way round.

use crate::fixtures::{self, Fixture};
use crate::progress::Bar;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as AsyncMutex;

/// Everything the request handlers share.
pub struct Demand {
    pub root: PathBuf,
    /// One lock per fixture, so two viewers asking at once wait rather than
    /// generating the same file twice over each other.
    locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
    /// The terminal is a single surface, so the bar is shared rather than one
    /// per request drawing over the others.
    bar: Arc<Mutex<Bar>>,
}

impl Demand {
    /// The bar is handed in rather than created here, since the request log
    /// prints to the same terminal and both must interrupt the same bar.
    pub fn new(root: PathBuf, bar: Arc<Mutex<Bar>>) -> Arc<Self> {
        Arc::new(Self {
            root,
            locks: Mutex::new(HashMap::new()),
            bar,
        })
    }

    /// The lock covering one fixture.
    fn lock_for(&self, id: &str) -> Arc<AsyncMutex<()>> {
        let mut locks = match self.locks.lock() {
            Ok(locks) => locks,
            // A poisoned lock only means some other request panicked, which
            // says nothing about whether this one can proceed.
            Err(poisoned) => poisoned.into_inner(),
        };

        Arc::clone(
            locks
                .entry(id.to_string())
                .or_insert_with(|| Arc::new(AsyncMutex::new(()))),
        )
    }

    /// Make sure a fixture is on disk, generating it if not.
    ///
    /// Generation is CPU bound and takes tens of seconds, so it runs on a
    /// blocking thread rather than tying up the async runtime.
    pub async fn ensure_built(
        self: &Arc<Self>,
        fixture: &Fixture,
        progress: &super::Progress,
    ) -> Result<(), String> {
        let guard = self.lock_for(fixture.id);
        let _held = guard.lock().await;

        let root = self.root.clone();
        let bar = Arc::clone(&self.bar);
        let progress = Arc::clone(progress);
        let subject = fixture.clone();

        tokio::task::spawn_blocking(move || {
            let report = |report: fixtures::Report<'_>| {
                super::record_progress(&progress, &report);
                if let Ok(mut bar) = bar.lock() {
                    bar.handle(report);
                }
            };

            fixtures::build_one(&subject, &root, report).map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| format!("generation thread failed: {error}"))?
    }
}
