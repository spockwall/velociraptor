use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;
use tracing::{info, warn};

pub struct ThreadMonitor {
    active_threads: Arc<AtomicUsize>,
    max_threads: usize,
}

impl ThreadMonitor {
    pub fn new(max_threads: usize) -> Self {
        Self {
            active_threads: Arc::new(AtomicUsize::new(0)),
            max_threads,
        }
    }

    pub fn spawn_monitored<F, T>(&self, name: &str, f: F) -> thread::JoinHandle<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let active_threads = self.active_threads.clone();
        let name = name.to_string();

        active_threads.fetch_add(1, Ordering::SeqCst);

        let handle = thread::spawn(move || {
            info!("Thread '{}' started", name);
            let result = f();
            active_threads.fetch_sub(1, Ordering::SeqCst);
            info!("Thread '{}' finished", name);
            result
        });

        self.check_thread_limit();
        handle
    }

    pub fn get_active_count(&self) -> usize {
        self.active_threads.load(Ordering::SeqCst)
    }

    pub fn check_thread_limit(&self) {
        let active = self.get_active_count();
        if active > self.max_threads {
            warn!(
                "High thread count: {} active threads (max: {})",
                active, self.max_threads
            );
        }
    }

    pub fn print_status(&self) {
        let active = self.get_active_count();
        info!(
            "Thread Monitor: {} active threads (max: {})",
            active, self.max_threads
        );
    }

    pub fn wait_for_completion(&self, timeout: Duration) -> bool {
        let start = std::time::Instant::now();
        while self.get_active_count() > 0 {
            if start.elapsed() > timeout {
                warn!("Timeout waiting for threads to complete");
                return false;
            }
            thread::sleep(Duration::from_millis(100));
        }
        true
    }
}

impl Default for ThreadMonitor {
    fn default() -> Self {
        Self::new(50) // Default max 50 threads
    }
}

// Global thread monitor instance
lazy_static::lazy_static! {
    pub static ref GLOBAL_MONITOR: Arc<ThreadMonitor> = Arc::new(ThreadMonitor::default());
}

// Utility functions
pub fn get_active_thread_count() -> usize {
    GLOBAL_MONITOR.get_active_count()
}

pub fn print_thread_status() {
    GLOBAL_MONITOR.print_status();
}

pub fn spawn_monitored_thread<F, T>(name: &str, f: F) -> thread::JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    GLOBAL_MONITOR.spawn_monitored(name, f)
}
