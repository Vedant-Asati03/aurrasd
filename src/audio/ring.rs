use std::cell::UnsafeCell;
use std::sync::atomic::AtomicBool;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

pub struct RingBuffer {
    buf: Vec<UnsafeCell<f32>>,
    capacity: usize,
    write: AtomicUsize,
    read: AtomicUsize,
    finished: AtomicBool,
}

// SAFETY:
// - Single producer (decode thread)
// - Single consumer (audio thread)
// - No simultaneous write/write or read/write on same index
unsafe impl Sync for RingBuffer {}

impl RingBuffer {
    pub fn new(capacity: usize) -> Arc<Self> {
        let buf = (0..capacity).map(|_| UnsafeCell::new(0.0)).collect();

        Arc::new(Self {
            buf,
            capacity,
            write: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
            finished: AtomicBool::new(false),
        })
    }
}

impl RingBuffer {
    #[inline]
    pub fn write(&self, input: &[f32]) -> usize {
        let mut written = 0;

        let mut w = self.write.load(Ordering::Relaxed);

        for &sample in input {
            // wait if buffer is full
            loop {
                let r = self.read.load(Ordering::Acquire);

                if (w + 1) % self.capacity != r % self.capacity {
                    break;
                }

                std::hint::spin_loop();
            }

            unsafe {
                *self.buf[w % self.capacity].get() = sample;
            }

            w += 1;
            written += 1;
        }

        self.write.store(w, Ordering::Release);
        written
    }

    #[inline]
    pub fn read(&self, output: &mut [f32]) -> usize {
        let mut read_count = 0;

        let mut r = self.read.load(Ordering::Relaxed);
        let w = self.write.load(Ordering::Acquire);

        for sample in output {
            if r == w {
                break; // buffer empty
            }

            unsafe {
                *sample = *self.buf[r % self.capacity].get();
            }

            r += 1;
            read_count += 1;
        }

        self.read.store(r, Ordering::Release);
        read_count
    }

    pub fn mark_finished(&self) {
        self.finished.store(true, Ordering::Release);
    }

    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
}
