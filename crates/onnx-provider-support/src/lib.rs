// SPDX-License-Identifier: AGPL-3.0-only

//! Shared support for source-only ONNXRuntime JSON providers.
//!
//! This crate owns provider-side lifecycle helpers only: idempotent ORT
//! initialization, cancelable inference watchdogs, and session caches. It does
//! not define game logic, OCR semantics, or model-specific behavior.

use ort::session::{RunOptions, Session};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub struct OrtRuntimeInitializer {
    library: OnceLock<PathBuf>,
    lock: Mutex<()>,
}

impl OrtRuntimeInitializer {
    pub const fn new() -> Self {
        Self {
            library: OnceLock::new(),
            lock: Mutex::new(()),
        }
    }

    pub fn ensure(&self, runtime_library: &Path) -> Result<(), String> {
        self.ensure_with(runtime_library, |path| {
            let committed = ort::init_from(path)
                .map_err(|err| {
                    format!(
                        "failed to load ONNXRuntime library {}: {err}",
                        path.display()
                    )
                })?
                .commit();
            Ok(committed)
        })
    }

    pub fn ensure_with<F>(&self, runtime_library: &Path, init: F) -> Result<(), String>
    where
        F: FnOnce(&Path) -> Result<bool, String>,
    {
        if let Some(existing) = self.library.get() {
            return ensure_same_runtime(existing, runtime_library);
        }

        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(existing) = self.library.get() {
            return ensure_same_runtime(existing, runtime_library);
        }

        let committed = init(runtime_library)?;
        if !committed {
            return Err(
                "ONNXRuntime environment was already configured before this provider initialized"
                    .to_string(),
            );
        }
        self.library
            .set(runtime_library.to_path_buf())
            .map_err(|_| "failed to record ONNXRuntime runtime library path".to_string())
    }
}

impl Default for OrtRuntimeInitializer {
    fn default() -> Self {
        Self::new()
    }
}

fn ensure_same_runtime(existing: &Path, requested: &Path) -> Result<(), String> {
    if existing == requested {
        Ok(())
    } else {
        Err(format!(
            "ONNXRuntime is already initialized from {}; refusing second runtime library {}",
            existing.display(),
            requested.display()
        ))
    }
}

pub struct SessionCache<T> {
    sessions: Mutex<HashMap<PathBuf, Arc<Mutex<T>>>>,
}

impl<T> SessionCache<T> {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn get_or_load<F>(&self, path: &Path, load: F) -> Result<Arc<Mutex<T>>, String>
    where
        F: FnOnce(&Path) -> Result<T, String>,
    {
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(session) = sessions.get(path) {
            return Ok(Arc::clone(session));
        }
        let session = Arc::new(Mutex::new(load(path)?));
        sessions.insert(path.to_path_buf(), Arc::clone(&session));
        Ok(session)
    }
}

impl<T> Default for SessionCache<T> {
    fn default() -> Self {
        Self::new()
    }
}

pub type OrtSessionCache = SessionCache<Session>;

pub trait InferenceTerminator: Send + Sync + 'static {
    fn terminate_inference(&self);
}

impl InferenceTerminator for RunOptions {
    fn terminate_inference(&self) {
        let _ = self.terminate();
    }
}

pub struct InferenceWatchdog {
    state: Arc<(Mutex<bool>, Condvar)>,
    handle: Option<JoinHandle<()>>,
}

impl InferenceWatchdog {
    pub fn start<T>(target: Arc<T>, timeout: Duration) -> Self
    where
        T: InferenceTerminator,
    {
        Self::start_with_cancel_hook(target, timeout, || {})
    }

    pub fn start_with_cancel_hook<T, F>(target: Arc<T>, timeout: Duration, on_cancel: F) -> Self
    where
        T: InferenceTerminator,
        F: FnOnce() + Send + 'static,
    {
        let state = Arc::new((Mutex::new(false), Condvar::new()));
        let thread_state = Arc::clone(&state);
        let handle = thread::spawn(move || {
            let (lock, condvar) = &*thread_state;
            let cancelled = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let (cancelled, _timeout) = condvar
                .wait_timeout_while(cancelled, timeout, |cancelled| !*cancelled)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if *cancelled {
                on_cancel();
            } else {
                target.terminate_inference();
            }
        });
        Self {
            state,
            handle: Some(handle),
        }
    }

    pub fn cancel(mut self) {
        self.cancel_inner();
    }

    fn cancel_inner(&mut self) {
        let (lock, condvar) = &*self.state;
        {
            let mut cancelled = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            *cancelled = true;
            condvar.notify_one();
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for InferenceWatchdog {
    fn drop(&mut self) {
        self.cancel_inner();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    #[test]
    fn ort_runtime_initializer_is_idempotent_under_concurrency() {
        let initializer = Arc::new(OrtRuntimeInitializer::new());
        let init_count = Arc::new(AtomicUsize::new(0));
        let path = PathBuf::from("onnxruntime.dll");
        let handles = (0..2)
            .map(|_| {
                let initializer = Arc::clone(&initializer);
                let init_count = Arc::clone(&init_count);
                let path = path.clone();
                thread::spawn(move || {
                    initializer.ensure_with(&path, |_| {
                        init_count.fetch_add(1, Ordering::SeqCst);
                        Ok(true)
                    })
                })
            })
            .collect::<Vec<_>>();

        for handle in handles {
            handle.join().expect("thread").expect("init");
        }

        assert_eq!(init_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn session_cache_loads_same_path_once() {
        let cache = SessionCache::new();
        let loads = AtomicUsize::new(0);
        let path = Path::new("model.onnx");

        let first = cache.get_or_load(path, |_| {
            loads.fetch_add(1, Ordering::SeqCst);
            Ok(7_u32)
        });
        let second = cache.get_or_load(path, |_| {
            loads.fetch_add(1, Ordering::SeqCst);
            Ok(9_u32)
        });

        assert_eq!(loads.load(Ordering::SeqCst), 1);
        assert!(Arc::ptr_eq(
            &first.expect("first"),
            &second.expect("second")
        ));
    }

    #[test]
    fn session_cache_loads_distinct_paths_separately() {
        let cache = SessionCache::new();
        let loads = AtomicUsize::new(0);

        cache
            .get_or_load(Path::new("a.onnx"), |_| {
                loads.fetch_add(1, Ordering::SeqCst);
                Ok(1_u32)
            })
            .expect("a");
        cache
            .get_or_load(Path::new("b.onnx"), |_| {
                loads.fetch_add(1, Ordering::SeqCst);
                Ok(2_u32)
            })
            .expect("b");

        assert_eq!(loads.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn watchdog_reports_early_cancel_before_timeout() {
        struct FakeTerminator(AtomicUsize);

        impl InferenceTerminator for FakeTerminator {
            fn terminate_inference(&self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        let (tx, rx) = mpsc::channel();
        let target = Arc::new(FakeTerminator(AtomicUsize::new(0)));

        let watchdog = InferenceWatchdog::start_with_cancel_hook(
            Arc::clone(&target),
            Duration::from_secs(60),
            move || tx.send(()).expect("cancel notification"),
        );
        watchdog.cancel();

        rx.recv_timeout(Duration::from_secs(1))
            .expect("early cancel");
        assert_eq!(target.0.load(Ordering::SeqCst), 0);
    }
}
