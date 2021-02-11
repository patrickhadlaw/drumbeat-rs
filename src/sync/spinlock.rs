#![feature(negative_impls)]
use std::sync::atomic::{fence, AtomicBool, Ordering};
use std::cell::{Cell, UnsafeCell};
use std::sync::{LockResult, PoisonError};
use std::ops::{Deref, DerefMut};

pub struct SpinLock<T> {
  flag: AtomicBool,
  poisoned: Cell<bool>,
  inner: UnsafeCell<T>,
}

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
  fn deref_mut(&mut self) -> &mut T {
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

  pub fn lock(&self) -> LockResult<SpinLockGuard<'_, T>> {
    while self
      .flag
      .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
      .unwrap()
    {
      std::hint::spin_loop();
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
