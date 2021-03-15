use std::sync::atomic::{fence, AtomicBool, Ordering};
use std::cell::{Cell, UnsafeCell};
use std::sync::{LockResult, PoisonError};
use std::ops::{Deref, DerefMut};

use rand::distributions::{Distribution, Uniform};

pub struct SpinLock<T> {
  flag: AtomicBool,
  poisoned: Cell<bool>,
  inner: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for SpinLock<T> {}
unsafe impl<T: Send> Sync for SpinLock<T> {}

pub struct SpinLockGuard<'a, T> {
  lock: &'a SpinLock<T>,
}

impl <'a, T> Drop for SpinLockGuard<'a, T> {
  fn drop(&mut self) {
    if std::thread::panicking() {
      *self.lock.poisoned.as_ptr() = true;
    }
    unsafe {
      self.lock.unlock();
    }
  }
}

impl <'a, T> Deref for SpinLockGuard<'a, T> {
  type Target = T;

  fn deref(&self) -> &Self::Target {
    unsafe { &*self.lock.inner.get() }
  }
}

impl <'a, T> DerefMut for SpinLockGuard<'a, T> {
  fn deref_mut(&mut self) -> &mut Self::Target {
    unsafe { &mut *self.lock.inner.get() }
  }
}

impl <T> !Send for SpinLockGuard<'_, T> {}
unsafe impl <T: Sync> Sync for SpinLockGuard<'_, T> {}

impl <T> SpinLock<T> {
  pub fn new(value: T) -> Self {
    SpinLock {
      flag: AtomicBool::new(false),
      poisoned: Cell::new(false),
      inner: UnsafeCell::new(value),
    }
  }

  unsafe fn try_lock(&self) -> bool {
    self
      .flag
      .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
      .unwrap()
  }

  pub fn lock(&self) -> LockResult<SpinLockGuard<'_, T>> {
    if !unsafe { self.try_lock() } {
      let backoff = 1u32;
      let mut rng = rand::thread_rng();
      while unsafe { self.try_lock() } {
        backoff = std::cmp::min(10, backoff + 1);
        let uniform = Uniform::from(0..(2u32.pow(backoff) - 1));
        for i in 0..uniform.sample(&mut rng) {
          std::hint::spin_loop();
        }
      }
    }
    fence(Ordering::Acquire);
    if *self.poisoned.as_ptr() {
      LockResult::Err(PoisonError::new(SpinLockGuard {
        lock: self,
      }))
    } else {
      LockResult::Ok(SpinLockGuard {
        lock: self,
      })
    }
  }

  unsafe fn unlock(&self) {
    self.flag.store(false, Ordering::Release);
  }
}
