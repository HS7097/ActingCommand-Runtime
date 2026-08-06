// SPDX-License-Identifier: AGPL-3.0-only

//! Shared support for source-only ONNXRuntime JSON providers.
//!
//! This crate owns provider-side lifecycle helpers only: idempotent ORT
//! initialization, cancelable inference watchdogs, and session caches. It does
//! not define game logic, OCR semantics, or model-specific behavior.

use ort::session::{RunOptions, Session};
use std::collections::HashMap;
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Condvar, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};
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
            .map_err(|_| "ONNXRuntime initializer mutex is poisoned".to_string())?;
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

const DEFAULT_MAX_SESSION_CACHE_ENTRIES: usize = 64;

pub struct SessionCache<T, K = PathBuf> {
    sessions: Mutex<HashMap<K, Arc<Mutex<T>>>>,
    max_entries: usize,
}

impl<T, K> SessionCache<T, K>
where
    K: Clone + Eq + Hash,
{
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            max_entries: DEFAULT_MAX_SESSION_CACHE_ENTRIES,
        }
    }

    pub fn with_max_entries(max_entries: usize) -> Result<Self, String> {
        if max_entries == 0 {
            return Err("ONNXRuntime session cache bound must be non-zero".to_string());
        }
        Ok(Self {
            sessions: Mutex::new(HashMap::new()),
            max_entries,
        })
    }

    pub fn get_or_load<F>(&self, key: &K, load: F) -> Result<Arc<Mutex<T>>, String>
    where
        F: FnOnce(&K) -> Result<T, String>,
    {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| "ONNXRuntime session cache mutex is poisoned".to_string())?;
        if let Some(session) = sessions.get(key) {
            return Ok(Arc::clone(session));
        }
        if sessions.len() >= self.max_entries {
            return Err(format!(
                "ONNXRuntime session cache reached its bounded capacity of {} entries",
                self.max_entries
            ));
        }
        let session = Arc::new(Mutex::new(load(key)?));
        sessions.insert(key.clone(), Arc::clone(&session));
        Ok(session)
    }
}

impl<T, K> Default for SessionCache<T, K>
where
    K: Clone + Eq + Hash,
{
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
    timed_out: Arc<AtomicBool>,
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
        let timed_out = Arc::new(AtomicBool::new(false));
        let thread_timed_out = Arc::clone(&timed_out);
        let handle = thread::spawn(move || {
            let (lock, condvar) = &*thread_state;
            let cancelled = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let (cancelled, _timeout) = condvar
                .wait_timeout_while(cancelled, timeout, |cancelled| !*cancelled)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if *cancelled {
                on_cancel();
            } else {
                thread_timed_out.store(true, Ordering::Release);
                target.terminate_inference();
            }
        });
        Self {
            state,
            timed_out,
            handle: Some(handle),
        }
    }

    pub fn timed_out(&self) -> bool {
        self.timed_out.load(Ordering::Acquire)
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
        let cache: SessionCache<u32> = SessionCache::new();
        let loads = AtomicUsize::new(0);
        let path = PathBuf::from("model.onnx");

        let first = cache.get_or_load(&path, |_| {
            loads.fetch_add(1, Ordering::SeqCst);
            Ok(7_u32)
        });
        let second = cache.get_or_load(&path, |_| {
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
        let cache: SessionCache<u32> = SessionCache::new();
        let loads = AtomicUsize::new(0);
        let path_a = PathBuf::from("a.onnx");
        let path_b = PathBuf::from("b.onnx");

        cache
            .get_or_load(&path_a, |_| {
                loads.fetch_add(1, Ordering::SeqCst);
                Ok(1_u32)
            })
            .expect("a");
        cache
            .get_or_load(&path_b, |_| {
                loads.fetch_add(1, Ordering::SeqCst);
                Ok(2_u32)
            })
            .expect("b");

        assert_eq!(loads.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn session_cache_poison_fails_closed() {
        let cache = Arc::new(SessionCache::<u32>::new());
        let poison_target = Arc::clone(&cache);
        let _ = thread::spawn(move || {
            let _guard = poison_target.sessions.lock().expect("lock");
            panic!("poison session cache");
        })
        .join();

        let path = PathBuf::from("model.onnx");
        let err = cache
            .get_or_load(&path, |_| Ok(1))
            .expect_err("poisoned cache rejected");

        assert!(err.contains("poisoned"));
    }

    #[test]
    fn session_cache_fails_closed_at_the_configured_bound() {
        let cache: SessionCache<u32, String> =
            SessionCache::with_max_entries(1).expect("bounded cache");
        let first = "session-a".to_string();
        let second = "session-b".to_string();

        cache.get_or_load(&first, |_| Ok(1)).expect("first session");
        let err = cache
            .get_or_load(&second, |_| Ok(2))
            .expect_err("second distinct session must exceed the bound");

        assert!(err.contains("bounded capacity of 1"));
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
        assert!(!watchdog.timed_out());
        watchdog.cancel();

        rx.recv_timeout(Duration::from_secs(1))
            .expect("early cancel");
        assert_eq!(target.0.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn watchdog_terminates_after_timeout_without_cancel() {
        struct FakeTerminator(AtomicUsize);

        impl InferenceTerminator for FakeTerminator {
            fn terminate_inference(&self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        let target = Arc::new(FakeTerminator(AtomicUsize::new(0)));
        let watchdog = InferenceWatchdog::start(Arc::clone(&target), Duration::from_millis(5));

        for _ in 0..20 {
            if target.0.load(Ordering::SeqCst) > 0 {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(target.0.load(Ordering::SeqCst), 1);
        assert!(watchdog.timed_out());
        watchdog.cancel();
    }
}
