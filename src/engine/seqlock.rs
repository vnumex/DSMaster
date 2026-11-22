use std::cell::UnsafeCell;
use std::hint::spin_loop;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering, fence};

/// Wait-free single-writer, multi-reader SeqLock for `Copy` telemetry snapshots.
pub struct SeqLock<T: Copy> {
    seq: AtomicU64,
    data: UnsafeCell<T>,
}

unsafe impl<T: Copy + Send> Send for SeqLock<T> {}
unsafe impl<T: Copy + Send> Sync for SeqLock<T> {}

impl<T: Copy> SeqLock<T> {
    pub const fn new(initial: T) -> Self {
        Self {
            seq: AtomicU64::new(0),
            data: UnsafeCell::new(initial),
        }
    }

    #[inline]
    pub fn write(&self, value: T) {
        let s = self.seq.load(Ordering::Relaxed);
        self.seq.store(s.wrapping_add(1), Ordering::Release);
        fence(Ordering::Release);
        unsafe {
            ptr::write_volatile(self.data.get(), value);
        }
        fence(Ordering::Release);
        self.seq.store(s.wrapping_add(2), Ordering::Release);
    }

    #[inline]
    pub fn read(&self) -> T {
        loop {
            let s1 = self.seq.load(Ordering::Acquire);
            if (s1 & 1) != 0 {
                spin_loop();
                continue;
            }
            fence(Ordering::Acquire);
            let val = unsafe { ptr::read_volatile(self.data.get()) };
            fence(Ordering::Acquire);
            let s2 = self.seq.load(Ordering::Acquire);
            if s1 == s2 {
                return val;
            }
            spin_loop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    struct Pair {
        a: u64,
        b: u64,
    }

    #[test]
    fn seqlock_consistent_under_contention() {
        let lock = Arc::new(SeqLock::new(Pair {
            a: 0,
            b: 0xDEAD_BEEF,
        }));
        let writer_lock = Arc::clone(&lock);
        let reader_lock = Arc::clone(&lock);

        let writer = thread::spawn(move || {
            for i in 1..=200_000u64 {
                writer_lock.write(Pair {
                    a: i,
                    b: i ^ 0xDEAD_BEEF,
                });
            }
        });

        let reader = thread::spawn(move || {
            for _ in 0..200_000 {
                let p = reader_lock.read();
                assert_eq!(p.b, p.a ^ 0xDEAD_BEEF);
            }
        });

        writer.join().unwrap();
        reader.join().unwrap();
    }
}
