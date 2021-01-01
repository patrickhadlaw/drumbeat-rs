use drumbeat::sync::runtime::Runtime;

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

#[test]
fn runtime_test() {
  let counter = Arc::new(AtomicI32::new(0));
  for _ in 0..100 {
    let cloned = counter.clone();
    Runtime::submit(move || {
      cloned.fetch_add(1, Ordering::Relaxed);
    });
  }
  while !Runtime::done() {}
  assert_eq!(counter.load(Ordering::Relaxed), 100);
}
