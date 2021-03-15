use super::spinlock::{SpinLock, SpinLockGuard};

struct RingBufferInner<T> {
  front: usize,
  filled: bool,
  raw: Vec<T>,
}

pub struct RingBuffer<T> {
  inner: SpinLock<RingBufferInner<T>>,
  size: usize,
}

impl<T> RingBuffer<T>
where
  T: Clone,
{
  pub fn new(size: usize) -> RingBuffer<T> {
    RingBuffer {
      inner: SpinLock::new(RingBufferInner {
        front: 0,
        filled: false,
        raw: Vec::with_capacity(size),
      }),
      size,
    }
  }

  pub fn size(&self) -> usize {
    self.size
  }

  pub fn push(&self, value: T) {
    let mut guard = self.inner.lock().unwrap();
    if guard.raw.len() < self.size {
      guard.raw.push(value);
    } else {
      let idx = guard.front;
      guard.raw[idx] = value;
    }
    guard.front = (guard.front + 1) % self.size;
    if guard.front == 0 {
      guard.filled = true;
    }
  }

  unsafe fn get_critical(
    &self,
    guard: &SpinLockGuard<'_, RingBufferInner<T>>,
    num: usize
  ) -> Vec<T> {
    let mut result = Vec::new();
    result.reserve(num);
    let fetch = (if num < self.size { num } else { self.size }) as isize;
    let front = guard.front as isize;
    if guard.filled {
      let back = if front - fetch < 0 {
        ((self.size as isize) + front - fetch) as usize
      } else {
        (front - fetch) as usize
      };
      if back < guard.front {
        for item in guard.raw.iter().take(guard.front).skip(back) {
          result.push(item.clone());
        }
      } else {
        for i in (back..self.size).chain(0..guard.front) {
          result.push(guard.raw[i].clone());
        }
      }
    } else {
      let back = front - fetch;
      let start = if back < 0 { 0 } else { back as usize };
      for item in guard.raw.iter().take(guard.front).skip(start) {
        result.push(item.clone());
      }
    }
    result
  }

  pub fn get(&self, num: usize) -> Vec<T> {
    if num == 0 {
      return Vec::new();
    }
    let guard = self.inner.lock().unwrap();
    unsafe { self.get_critical(&guard, num) }
  }

  pub fn get_and_do<F>(&self, num: usize, task: F) -> Vec<T>
  where
    F: Fn(),
  {
    if num == 0 {
      return Vec::new();
    }
    let guard = self.inner.lock().unwrap();
    let result = unsafe { self.get_critical(&guard, num) };
    task();
    result
  }

  pub fn get_all(&self) -> Vec<T> {
    self.get(self.size)
  }
}

unsafe impl<T> Sync for RingBuffer<T> {}
unsafe impl<T> Send for RingBuffer<T> {}

#[cfg(test)]
mod test {
  use super::*;
  use crate::utils::testing::async_context;

  #[test]
  fn new_ring_buffer_test() {
    let ring = RingBuffer::<()>::new(5);
    assert_eq!(ring.size, 5);
    assert_eq!(ring.inner.lock().unwrap().filled, false);
    assert_eq!(ring.inner.lock().unwrap().front, 0);
  }

  #[test]
  fn push_test() {
    async_context(|| {
      let ring = RingBuffer::new(5);
      ring.push(1);
      ring.push(2);
      ring.push(3);
      assert_eq!(ring.inner.lock().unwrap().raw.len(), 3);
      assert_eq!(ring.inner.lock().unwrap().raw, [1, 2, 3]);
    });
  }

  #[test]
  fn overflow_test() {
    async_context(|| {
      let ring = RingBuffer::new(3);
      ring.push(1);
      ring.push(2);
      ring.push(3);
      ring.push(4);
      ring.push(5);
      ring.push(6);
      assert_eq!(ring.inner.lock().unwrap().raw.len(), 3);
      assert_eq!(ring.inner.lock().unwrap().raw, [4, 5, 6]);
    });
  }

  #[test]
  fn get_unfilled_test() {
    async_context(|| {
      let ring = RingBuffer::new(5);
      ring.push(1);
      ring.push(2);
      ring.push(3);
      assert_eq!(ring.get(2), [2, 3]);
    });
  }

  #[test]
  fn get_filled_test() {
    async_context(|| {
      let ring = RingBuffer::new(3);
      ring.push(1);
      ring.push(2);
      ring.push(3);
      ring.push(4);
      ring.push(5);
      assert_eq!(ring.inner.lock().unwrap().filled, true);
      assert_eq!(ring.get(2), [4, 5]);
    });
  }

  #[test]
  fn get_wraparound_test() {
    async_context(|| {
      let ring = RingBuffer::new(3);
      ring.push(1);
      ring.push(2);
      ring.push(3);
      ring.push(4);
      ring.push(5);
      assert_eq!(ring.inner.lock().unwrap().filled, true);
      assert_eq!(ring.get(3), [3, 4, 5]);
    });
  }
}
