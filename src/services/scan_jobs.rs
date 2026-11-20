//! One shared budget for disk scans, including rule previews and the analyzer.
use std::sync::{Condvar, Mutex};

struct Limiter {
    active: Mutex<usize>,
    ready: Condvar,
}
impl Limiter {
    const fn new() -> Self {
        Self {
            active: Mutex::new(0),
            ready: Condvar::new(),
        }
    }
    fn acquire(&self) -> Permit<'_> {
        let mut active = self.active.lock().unwrap();
        while *active >= 2 {
            active = self.ready.wait(active).unwrap();
        }
        *active += 1;
        Permit(self)
    }
}
pub struct Permit<'a>(&'a Limiter);
impl Drop for Permit<'_> {
    fn drop(&mut self) {
        *self.0.active.lock().unwrap() -= 1;
        self.0.ready.notify_one();
    }
}
static SCANS: Limiter = Limiter::new();
/// Acquire only on workers, never on GTK. Nested scanner helpers share the permit.
pub fn acquire() -> Permit<'static> {
    SCANS.acquire()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Barrier,
    };
    #[test]
    fn at_most_two_scans_run_and_unwinding_releases_permits() {
        let limiter = Arc::new(Limiter::new());
        let barrier = Arc::new(Barrier::new(8));
        let peak = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let (limiter, barrier, peak, active) = (
                    limiter.clone(),
                    barrier.clone(),
                    peak.clone(),
                    active.clone(),
                );
                std::thread::spawn(move || {
                    barrier.wait();
                    let _permit = limiter.acquire();
                    let n = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(n, Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    active.fetch_sub(1, Ordering::SeqCst);
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), 2);
        let _ = std::panic::catch_unwind(|| {
            let _permit = limiter.acquire();
            panic!("worker failed");
        });
        assert_eq!(*limiter.active.lock().unwrap(), 0);
    }
}
