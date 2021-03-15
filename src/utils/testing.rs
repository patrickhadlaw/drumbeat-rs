use std::{sync::mpsc, thread, time::Duration};

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

pub fn panic_after<T, F>(d: Duration, f: F) -> T
where
  T: Send + 'static,
  F: FnOnce() -> T + Send + 'static,
{
  let (done_tx, done_rx) = mpsc::channel();
  let handle = thread::Builder::new()
    .name("testing-thread".to_owned())
    .spawn(move || {
      let val = f();
      done_tx.send(()).expect("failed to send complete signal");
      val
    })
    .unwrap();
  match done_rx.recv_timeout(d) {
    Ok(_) => handle.join().expect("thread panicked"),
    Err(error) => match error {
      mpsc::RecvTimeoutError::Timeout => panic!("thread took too long"),
      mpsc::RecvTimeoutError::Disconnected => panic!("thread panicked"),
    },
  }
}

pub fn async_context<T, F>(f: F) -> T
where
  T: Send + 'static,
  F: FnOnce() -> T + Send + 'static,
{
  panic_after(DEFAULT_TIMEOUT, f)
}

pub fn interrupt_after<T, F>(d: Duration, f: F) -> T
where
  T: Send + 'static,
  F: FnOnce() -> T + Send + 'static,
{
  let (done_tx, done_rx) = mpsc::channel();
  let handle = thread::Builder::new()
    .name("testing-thread".to_owned())
    .spawn(move || {
      let val = f();
      let _ = done_tx.send(());
      val
    })
    .unwrap();
  let _ = done_rx.recv_timeout(d);
  handle.join().expect("thread panicked")
}

pub fn async_panic_context<T, F>(f: F) -> T
where
  T: Send + 'static,
  F: FnOnce() -> T + Send + 'static,
{
  interrupt_after(DEFAULT_TIMEOUT, f)
}

#[cfg(test)]
mod test {
  use super::*;

  #[test]
  #[should_panic]
  fn panic_after_timeout_test() {
    panic_after(Duration::from_secs(0), || {
      std::thread::sleep(Duration::from_secs(1));
    });
  }

  #[test]
  fn no_panic_after_timeout_test() {
    interrupt_after(Duration::from_secs(0), || {
      std::thread::sleep(Duration::from_secs(1));
    });
  }

  #[test]
  #[should_panic]
  fn interrupt_panic_passthrough_test() {
    interrupt_after(Duration::from_secs(1), || {
      panic!("test");
    });
  }
}
