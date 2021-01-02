use super::dispatcher::{DispatchTarget, Invoker};
use super::observable::{Observable, ObservableType, Owner, Pipe, Signal};

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

pub trait Map<A, B>
where
  A: ObservableType,
  B: ObservableType,
{
  /// Attaches a mapping operator to the end of the chain and forwards the pipe
  ///
  /// `map` is used for mapping one observable type into another.
  ///
  /// # Example
  /// ```
  /// use drumbeat::event::observable::Observable;
  /// use drumbeat::event::ops::*;
  ///
  /// let rx = Observable::of(vec![1, 2, 3])
  ///   .pipe()
  ///   .map(|x| format!("value_{}", x))
  ///   .collect();
  ///
  /// assert_eq!(rx.recv().unwrap(), ["value_1", "value_2", "value_3"]);
  /// ```
  fn map<F>(&mut self, map: F) -> Pipe<B>
  where
    F: Fn(A) -> B + Send + Sync + 'static;
}

impl<A, B> Map<A, B> for Pipe<A>
where
  A: ObservableType,
  B: ObservableType,
{
  fn map<F>(&mut self, map: F) -> Pipe<B>
  where
    F: Fn(A) -> B + Send + Sync + 'static,
  {
    let observable = self.make_observer::<B>();
    let cloned = observable.clone();
    self.attach::<B>(
      observable.clone(),
      DispatchTarget::new(
        observable,
        Invoker::new(Arc::new(move |x| {
          cloned.next(map(x));
          Signal::None
        })),
      ),
    )
  }
}

pub trait Take<T>
where
  T: ObservableType,
{
  /// Attaches a take lifecycle operator to the end of the chain and forwards
  /// the pipe
  ///
  /// `take` is used for keeping an observable chain alive for only `n` events.
  ///
  /// # Example
  /// ```
  /// use drumbeat::event::observable::Observable;
  /// use drumbeat::event::ops::*;
  ///
  /// let rx = Observable::of(vec![1, 2, 3, 4, 5, 6])
  ///   .pipe()
  ///   .take(3)
  ///   .collect();
  ///
  /// assert_eq!(rx.recv().unwrap(), [1, 2, 3]);
  /// ```
  fn take(&mut self, n: usize) -> Pipe<T>;
}

impl<T> Take<T> for Pipe<T>
where
  T: ObservableType,
{
  fn take(&mut self, n: usize) -> Pipe<T> {
    let observable = self.make_observer();
    let count = Arc::new(AtomicUsize::new(0));
    let cloned = observable.clone();
    if n > 0 {
      self.attach(
        observable.clone(),
        DispatchTarget::new(
          observable,
          Invoker::new(Arc::new(move |x| {
            let consumed = count.fetch_add(1, Ordering::Relaxed);
            if consumed < n {
              cloned.next(x);
            }
            if consumed >= n - 1 {
              return Signal::Recycle(cloned.id());
            }
            Signal::None
          })),
        ),
      )
    } else {
      self.forward()
    }
  }
}

pub trait TakeUntil<A, B>
where
  A: ObservableType,
  B: ObservableType,
{
  /// Attaches a take until lifecycle operator to the end of the chain and
  /// forwards the pipe
  ///
  /// `take_until` is used for keeping an observable chain alive until the
  /// passed in observer `notify` emits a value.
  ///
  /// # Example
  /// ```
  /// use drumbeat::event::observable::ObservableBuilder;
  /// use drumbeat::event::subject::{Subject, BasicSubjectBuilder};
  /// use drumbeat::event::scheduler::SchedulerType;
  /// use drumbeat::event::ops::*;
  ///
  /// let done = BasicSubjectBuilder::new()
  ///   .scheduler(SchedulerType::Blocking)
  ///   .build();
  ///
  /// let subject = BasicSubjectBuilder::new()
  ///   .scheduler(SchedulerType::Blocking)
  ///   .build();
  ///
  /// let rx = subject
  ///   .observe()
  ///   .pipe()
  ///   .take_until(done.observe())
  ///   .collect();
  ///
  /// subject.next(1);
  /// subject.next(2);
  /// done.next(());
  /// subject.next(3);
  /// subject.next(4);
  ///
  /// assert_eq!(rx.recv().unwrap(), [1, 2]);
  /// ```
  fn take_until(&mut self, notify: Arc<Observable<B>>) -> Pipe<A>;
}

impl<A, B> TakeUntil<A, B> for Pipe<A>
where
  A: ObservableType,
  B: ObservableType,
{
  fn take_until(&mut self, notify: Arc<Observable<B>>) -> Pipe<A> {
    let closed = Arc::new(AtomicBool::new(false));
    let cloned = closed.clone();
    let observable = self.make_observer();
    let notify_to = observable.clone();
    let next = self.next.clone();
    let capture = notify
      .pipe()
      .first()
      .tap(move |_| {
        if let Ok(last) = closed.compare_exchange(
          false,
          true,
          Ordering::Relaxed,
          Ordering::Relaxed,
        ) {
          if !last {
            use super::scheduler::{Runtime, Scheduler};
            use crate::sync::threadpool::Task;
            let runtime = Runtime {};
            let next = next.clone();
            let notify_to = notify_to.clone();
            runtime.execute(Task::new(move || {
              next.write().unwrap().remove_child(notify_to.id());
            }));
          }
        }
      })
      .observe();
    self.attach(
      observable.clone(),
      DispatchTarget::new(
        observable.clone(),
        Invoker::new(Arc::new(move |x| {
          let _capture = capture.clone();
          if !cloned.load(Ordering::Relaxed) {
            observable.next(x);
          }
          Signal::None
        })),
      ),
    )
  }
}

pub trait TakeWhile<T>
where
  T: ObservableType,
{
  /// Attaches a take while lifecycle operator to the end of the chain and
  /// forwards the pipe
  ///
  /// `take_while` is used for keeping an observable chain alive until the
  /// predicate `predicate` fails.
  ///
  /// # Example
  /// ```
  /// use drumbeat::event::observable::Observable;
  /// use drumbeat::event::scheduler::SchedulerType;
  /// use drumbeat::event::ops::*;
  ///
  /// let rx = Observable::of(vec![1, 2, 3, 4])
  ///   .pipe()
  ///   .take_while(|x| x < 2)
  ///   .collect();
  ///
  /// assert_eq!(rx.recv().unwrap(), [1, 2]);
  /// ```
  fn take_while<F>(&mut self, predicate: F) -> Pipe<T>
  where
    F: Fn(T) -> bool + Send + Sync + 'static;
}

impl<T> TakeWhile<T> for Pipe<T>
where
  T: ObservableType,
{
  fn take_while<F>(&mut self, predicate: F) -> Pipe<T>
  where
    F: Fn(T) -> bool + Send + Sync + 'static,
  {
    let observable = self.make_observer();
    let finished = Arc::new(AtomicBool::new(false));
    let cloned = observable.clone();
    self.attach(
      observable.clone(),
      DispatchTarget::new(
        observable,
        Invoker::new(Arc::new(move |x| {
          let good = predicate(x.clone());
          if let Ok(last) = finished.compare_exchange(
            false,
            !good,
            Ordering::Relaxed,
            Ordering::Relaxed,
          ) {
            if !last {
              cloned.next(x);
              if !good {
                return Signal::Recycle(cloned.id());
              }
            }
          }
          Signal::None
        })),
      ),
    )
  }
}

pub trait First<T>
where
  T: ObservableType,
{
  /// Attaches a first lifecycle operator to the end of the chain and forwards
  /// the pipe
  ///
  /// `first` is used for keeping an observable chain alive for only the first
  /// event.
  ///
  /// # Example
  /// ```
  /// use drumbeat::event::observable::Observable;
  /// use drumbeat::event::ops::*;
  ///
  /// let rx = Observable::of(vec![1, 2, 3])
  ///   .pipe()
  ///   .first()
  ///   .collect();
  ///
  /// assert_eq!(rx.recv().unwrap(), [1]);
  /// ```
  fn first(&mut self) -> Pipe<T>;
}

impl<T> First<T> for Pipe<T>
where
  T: ObservableType,
{
  fn first(&mut self) -> Pipe<T> {
    let observable = self.make_observer();
    let consumed = Arc::new(AtomicBool::new(false));
    let cloned = observable.clone();
    self.attach(
      observable.clone(),
      DispatchTarget::new(
        observable,
        Invoker::new(Arc::new(move |x| {
          if !consumed.swap(true, Ordering::Relaxed) {
            cloned.next(x);
          }
          Signal::Recycle(cloned.id())
        })),
      ),
    )
  }
}

pub trait Skip<T>
where
  T: ObservableType,
{
  /// Attaches a skip operator to the end of the chain and forwards the pipe
  ///
  /// `skip` is used to skip the first `n` events observed.
  ///
  /// # Example
  /// ```
  /// use drumbeat::event::observable::Observable;
  /// use drumbeat::event::ops::*;
  ///
  /// let rx = Observable::of(vec![1, 2, 3])
  ///   .pipe()
  ///   .skip(2)
  ///   .collect();
  ///
  /// assert_eq!(rx.recv().unwrap(), [3]);
  /// ```
  fn skip(&mut self, n: usize) -> Pipe<T>;
}

impl<T> Skip<T> for Pipe<T>
where
  T: ObservableType,
{
  fn skip(&mut self, n: usize) -> Pipe<T> {
    let observable = self.make_observer();
    let count = Arc::new(AtomicUsize::new(0));
    let cloned = observable.clone();
    self.attach(
      observable.clone(),
      DispatchTarget::new(
        observable,
        Invoker::new(Arc::new(move |x| {
          let consumed = count.fetch_add(1, Ordering::Relaxed);
          if consumed >= n {
            cloned.next(x);
          }
          Signal::None
        })),
      ),
    )
  }
}

pub trait Filter<T>
where
  T: ObservableType,
{
  /// Attaches a filter operator to the end of the chain and forwards the pipe
  ///
  /// `filter` is used to filter events based on the predicate `predicate`.
  ///
  /// # Example
  /// ```
  /// use drumbeat::event::observable::Observable;
  /// use drumbeat::event::ops::*;
  ///
  /// let rx = Observable::of(vec![1, 2, 3, 4, 5, 6])
  ///   .pipe()
  ///   .filter(|x| x % 2 == 0)
  ///   .collect();
  ///
  /// assert_eq!(rx.recv().unwrap(), [2, 4, 6]);
  /// ```
  fn filter<F>(&mut self, predicate: F) -> Pipe<T>
  where
    F: Fn(&T) -> bool + Send + Sync + 'static;
}

impl<T> Filter<T> for Pipe<T>
where
  T: ObservableType,
{
  fn filter<F>(&mut self, predicate: F) -> Pipe<T>
  where
    F: Fn(&T) -> bool + Send + Sync + 'static,
  {
    let observable = self.make_observer();
    self.attach(
      observable.clone(),
      DispatchTarget::new(
        observable.clone(),
        Invoker::new(Arc::new(move |x| {
          if predicate(&x) {
            observable.next(x);
          }
          Signal::None
        })),
      ),
    )
  }
}

pub trait Collect<T>
where
  T: ObservableType,
{
  /// Creates a collect operator at the end of the chain and consumes the pipe
  ///
  /// `collect` is used for getting a channel which waits for the chain to close
  /// and sends over all of the tracked events.
  ///
  /// # Example
  /// ```
  /// use drumbeat::event::observable::Observable;
  /// use drumbeat::event::ops::*;
  ///
  /// let rx = Observable::of(vec![1, 2, 3])
  ///   .pipe()
  ///   .collect();
  ///
  /// assert_eq!(rx.recv().unwrap(), [1, 2, 3]);
  /// ```
  fn collect(&mut self) -> Receiver<Vec<T>>;
}

impl<T> Collect<T> for Pipe<T>
where
  T: ObservableType,
{
  fn collect(&mut self) -> Receiver<Vec<T>> {
    let observable = self.make_observer::<T>();
    let result = Arc::new(Mutex::new(Vec::new()));
    let cloned = result.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = Mutex::new(tx);
    let mut pipe = self.attach(
      observable.clone(),
      DispatchTarget::new(
        observable,
        Invoker::new(Arc::new(move |x| {
          result.lock().unwrap().push(x);
          Signal::None
        })),
      ),
    );
    let pipe = pipe.finalize(move || {
      tx.lock()
        .unwrap()
        .send(cloned.lock().unwrap().clone())
        .unwrap()
    });
    pipe.instantiate();
    rx
  }
}

pub trait Tap<T>
where
  T: ObservableType,
{
  /// Attaches a tap to the end of the chain and forwards the pipe
  ///
  /// `tap` is used to tap into the event chain and run code without affecting
  /// the downstream observers.
  ///
  /// # Example
  /// ```
  /// use drumbeat::event::observable::Observable;
  /// use drumbeat::event::ops::*;
  ///
  /// let rx = Observable::of(vec![1, 2, 3, 4, 5, 6])
  ///   .pipe()
  ///   .filter(|x| x % 2 == 0)
  ///   .collect();
  ///
  /// assert_eq!(rx.recv().unwrap(), [2, 4, 6]);
  /// ```
  fn tap<F>(&mut self, tap: F) -> Pipe<T>
  where
    F: Fn(T) + Send + Sync + 'static;
}

impl<T> Tap<T> for Pipe<T>
where
  T: ObservableType,
{
  fn tap<F>(&mut self, tap: F) -> Pipe<T>
  where
    F: Fn(T) + Send + Sync + 'static,
  {
    let observable = self.make_observer();
    self.attach(
      observable.clone(),
      DispatchTarget::new(
        observable.clone(),
        Invoker::new(Arc::new(move |x| {
          tap(x.clone());
          observable.next(x);
          Signal::None
        })),
      ),
    )
  }
}

pub trait Dangling<T>
where
  T: ObservableType,
{
  /// Consumes the pipe and leaves the end of the chain dangling
  ///
  /// `dangling` is used when you want the upstream observers to have complete
  /// control over the lifecycle of the end of the chain
  ///
  /// # Example
  /// ```
  /// use drumbeat::event::observable::ObservableBuilder;
  /// use drumbeat::event::scheduler::SchedulerType;
  /// use drumbeat::event::ops::*;
  ///
  /// let rx = ObservableBuilder::of(vec![1, 2, 3])
  ///   .scheduler(SchedulerType::Blocking)
  ///   .build()
  ///   .pipe()
  ///   .map(|x| x * 10)
  ///   .assert(|x| x >= 10 && x <= 30)
  ///   .assert_count(3)
  ///   .dangling();
  /// ```
  fn dangling(&mut self);
}

impl<T> Dangling<T> for Pipe<T>
where
  T: ObservableType,
{
  fn dangling(&mut self) {
    self.instantiate();
  }
}

pub trait Debounce<T>
where
  T: ObservableType,
{
  /// Attaches a 100ms debounce operator to the end of the chain and forwards
  /// the pipe, see [this method](Debounce::debounce_for) for details
  fn debounce(&mut self) -> Pipe<T>;
  /// Attaches a debounce operator to the end of the chain and forwards
  /// the pipe
  ///
  /// `debounce_for` is used for tapping into a debounced event stream. This
  /// means events will only be emitted after the stream is idle for `duration`
  /// amount of time.
  ///
  /// # Example
  /// ```
  /// use drumbeat::event::subject::{BasicSubject, Subject};
  /// use drumbeat::event::ops::*;
  /// use std::time::Duration;
  ///
  /// let subject = BasicSubject::new();
  /// let rx = subject.observe()
  ///   .pipe()
  ///   .take(100)
  ///   .assert_count(100)
  ///   .debounce_for(Duration::from_millis(10))
  ///   .collect();
  /// for i in 0..100 {
  ///   subject.next(i);
  ///   if i == 50 {
  ///     std::thread::sleep(Duration::from_millis(150));
  ///   }
  /// }
  /// assert_eq!(rx.recv().unwrap(), [50]);
  /// ```
  fn debounce_for(&mut self, duration: Duration) -> Pipe<T>;
}

impl<T> Debounce<T> for Pipe<T>
where
  T: ObservableType,
{
  fn debounce(&mut self) -> Pipe<T> {
    self.debounce_for(Duration::from_millis(100))
  }

  fn debounce_for(&mut self, duration: Duration) -> Pipe<T> {
    enum DebounceCommand<T> {
      Value(T),
      Finish,
    }
    let observable = self.make_observer();
    let captured = observable.clone();
    let connected = Arc::new(AtomicBool::new(true));
    let cloned = connected.clone();
    let worker = Arc::new(crate::sync::worker::Worker::new());
    let (tx, rx) = std::sync::mpsc::channel();
    let finalize = Mutex::new(tx.clone());
    self.finalize(move || {
      finalize
        .lock()
        .unwrap()
        .send(DebounceCommand::Finish)
        .unwrap();
    });
    let rx = Mutex::new(rx);
    worker.submit(move || {
      let mut value: Option<T> = None;
      let mut last_time = SystemTime::now();
      let mut wait_for = Duration::from_secs(u64::MAX);
      loop {
        let command = rx.lock().unwrap().recv_timeout(wait_for);
        if let Err(error) = command {
          match error {
            RecvTimeoutError::Timeout => {
              if let Some(next) = &value {
                if last_time.elapsed().unwrap() > duration {
                  captured.next(next.clone());
                  last_time = SystemTime::now();
                  wait_for = Duration::from_secs(u64::MAX);
                  value = None;
                } else {
                  wait_for = duration - last_time.elapsed().unwrap();
                }
              }
            }
            RecvTimeoutError::Disconnected => break,
          }
        } else {
          match command.unwrap() {
            DebounceCommand::Value(received) => {
              if value.is_none() {
                last_time = SystemTime::now()
              }
              value = Some(received);
              last_time.elapsed().expect("TESTING ERROR");
              let elapsed = last_time.elapsed().unwrap();
              wait_for = if elapsed > duration {
                Duration::from_nanos(0)
              } else {
                duration - elapsed
              }
            }
            DebounceCommand::Finish => break,
          }
        }
      }
      cloned.store(false, Ordering::Relaxed);
    });
    let tx = Mutex::new(tx);
    self.attach(
      observable.clone(),
      DispatchTarget::new(
        observable,
        Invoker::new(Arc::new(move |x| {
          let _worker = worker.clone();
          if connected.load(Ordering::Relaxed) {
            tx.lock().unwrap().send(DebounceCommand::Value(x)).unwrap();
          }
          Signal::None
        })),
      ),
    )
  }
}

pub trait Assert<T>
where
  T: ObservableType,
{
  /// Attaches a assert operator to the end of the chain and forwards the pipe
  ///
  /// `assert` is used to assert a predicate `test` for each event passing
  /// through the current observer.
  ///
  /// # Example
  /// ```
  /// use drumbeat::event::observable::Observable;
  /// use drumbeat::event::ops::*;
  ///
  /// let rx = Observable::of(vec![1, 2, 3, 4, 5, 6])
  ///   .pipe()
  ///   .filter(|x| x % 2 == 0)
  ///   .assert(|x| x % 2 != 1)
  ///   .collect();
  ///
  /// assert_eq!(rx.recv().unwrap(), [2, 4, 6]);
  /// ```
  fn assert<F>(&mut self, test: F) -> Pipe<T>
  where
    F: Fn(T) -> bool + Send + Sync + 'static;
  /// Attaches a assert count operator to the end of the chain and forwards the
  /// pipe
  ///
  /// `assert_count` is used to assert that 'count' events have passed through
  /// the current observer at the end of its lifetime.
  ///
  /// # Example
  /// ```
  /// use drumbeat::event::observable::Observable;
  /// use drumbeat::event::ops::*;
  ///
  /// let rx = Observable::of(vec![1, 2, 3, 4, 5, 6])
  ///   .pipe()
  ///   .take(3)
  ///   .assert_count(3)
  ///   .collect();
  ///
  /// assert_eq!(rx.recv().unwrap(), [1, 2, 3]);
  /// ```
  fn assert_count(&mut self, count: usize) -> Pipe<T>;
}

impl<T> Assert<T> for Pipe<T>
where
  T: ObservableType,
{
  fn assert<F>(&mut self, test: F) -> Pipe<T>
  where
    F: Fn(T) -> bool + Send + Sync + 'static,
  {
    let observable = self.make_observer();
    self.attach(
      observable.clone(),
      DispatchTarget::new(
        observable.clone(),
        Invoker::new(Arc::new(move |x| {
          assert!(test(x.clone()));
          observable.next(x);
          Signal::None
        })),
      ),
    )
  }

  fn assert_count(&mut self, count: usize) -> Pipe<T> {
    let observable = self.make_observer();
    let counter = Arc::new(AtomicUsize::new(0));
    let cloned = counter.clone();
    self
      .attach(
        observable.clone(),
        DispatchTarget::new(
          observable.clone(),
          Invoker::new(Arc::new(move |x| {
            cloned.fetch_add(1, Ordering::Relaxed);
            observable.next(x);
            Signal::None
          })),
        ),
      )
      .finalize(move || {
        let counted = counter.load(Ordering::Relaxed);
        assert_eq!(
          counted, count,
          "assertion failed: `observable.assert_count({})`\ncounted: `{}`",
          count, counted
        );
      })
  }
}

pub trait DebugAssert<T>
where
  T: ObservableType,
{
  /// Equivalent to [Assert](Assert::assert) when in debug mode otherwise has
  /// no effect
  fn debug_assert<F>(&mut self, test: F) -> Pipe<T>
  where
    F: Fn(T) -> bool + Send + Sync + 'static;
}

impl<T> DebugAssert<T> for Pipe<T>
where
  T: ObservableType,
{
  #[cfg(debug_assertions)]
  fn debug_assert<F>(&mut self, test: F) -> Pipe<T>
  where
    F: Fn(T) -> bool + Send + Sync + 'static,
  {
    self.assert(test)
  }

  #[cfg(not(debug_assertions))]
  fn debug_assert<F>(&mut self, test: F) -> Pipe<T>
  where
    F: Fn(T) -> bool + Send + Sync + 'static,
  {
    self.forward()
  }
}

#[cfg(test)]
mod test {
  use super::*;
  use crate::event::dispatcher::DispatcherType;
  use crate::event::observable;
  use crate::event::scheduler::SchedulerType;
  use crate::utils;

  #[test]
  fn assert_test() {
    utils::testing::async_context(|| {
      let observable = observable::testing::mock_observable::<()>(
        SchedulerType::Blocking,
        DispatcherType::Basic,
      );
      observable.pipe().assert(|_| true).dangling();
      observable.next(());
    });
  }

  #[test]
  #[should_panic]
  fn assert_panic_test() {
    utils::testing::async_panic_context(|| {
      let observable = observable::testing::mock_observable::<()>(
        SchedulerType::Blocking,
        DispatcherType::Basic,
      );
      observable.pipe().assert(|_| false).dangling();
      observable.next(());
    });
  }

  #[test]
  fn assert_count_test() {
    utils::testing::async_context(|| {
      let observable = observable::testing::mock_observable::<()>(
        SchedulerType::Blocking,
        DispatcherType::Basic,
      );
      observable.pipe().assert_count(3).dangling();
      observable.next(());
      observable.next(());
      observable.next(());
    });
  }

  #[test]
  #[should_panic]
  fn assert_count_panic_test() {
    utils::testing::async_panic_context(|| {
      let observable = observable::testing::mock_observable::<()>(
        SchedulerType::Blocking,
        DispatcherType::Basic,
      );
      observable.pipe().assert_count(3).dangling();
      observable.next(());
    });
  }

  #[test]
  fn tap_test() {
    utils::testing::async_context(|| {
      let observable = observable::testing::mock_observable(
        SchedulerType::Blocking,
        DispatcherType::Basic,
      );
      let count = Arc::new(AtomicUsize::new(0));
      let cloned = count.clone();
      observable
        .pipe()
        .tap(move |x| {
          cloned.fetch_add(x, Ordering::Relaxed);
        })
        .assert_count(3)
        .dangling();
      observable.next(1);
      observable.next(2);
      observable.next(3);
      assert_eq!(count.load(Ordering::Relaxed), 6);
    });
  }

  #[test]
  fn debounce_test() {
    utils::testing::async_context(|| {
      let observable = observable::testing::mock_observable(
        SchedulerType::Blocking,
        DispatcherType::Basic,
      );
      let rx = observable
        .pipe()
        .take(100)
        .assert_count(100)
        .debounce()
        .collect();
      for i in 0..100 {
        observable.next(i);
        if i == 50 {
          std::thread::sleep(Duration::from_millis(150));
        }
      }
      assert_eq!(rx.recv().unwrap(), [50]);
    });
  }

  #[test]
  fn map_test() {
    utils::testing::async_context(|| {
      let observable = observable::testing::mock_observable(
        SchedulerType::Blocking,
        DispatcherType::Basic,
      );
      observable
        .pipe()
        .map(|x| format!("test_{}", x))
        .assert(|x| x == "test_1234")
        .dangling();
      observable.next(1234);
    });
  }

  #[test]
  fn take_test() {
    utils::testing::async_context(|| {
      let observable = observable::testing::mock_observable(
        SchedulerType::Blocking,
        DispatcherType::Basic,
      );
      observable
        .pipe()
        .take(3)
        .assert(|x| x <= 3)
        .assert_count(3)
        .dangling();
      observable.next(1);
      observable.next(2);
      observable.next(3);
      assert_eq!(observable.num_children(), 0);
      observable.next(4);
    });
  }

  #[test]
  fn take_until_test() {
    utils::testing::async_context(|| {
      let observable = observable::testing::mock_observable(
        SchedulerType::Blocking,
        DispatcherType::Basic,
      );
      let done = observable::testing::mock_observable::<()>(
        SchedulerType::Blocking,
        DispatcherType::Basic,
      );
      observable
        .pipe()
        .take_until(done.clone())
        .assert(|x| x <= 3)
        .assert_count(3)
        .dangling();
      observable.next(1);
      observable.next(2);
      observable.next(3);
      done.next(());
      while !crate::sync::runtime::Runtime::done() {}
      assert_eq!(observable.num_children(), 0);
      observable.next(4);
    });
  }

  #[test]
  fn take_while_test() {
    utils::testing::async_context(|| {
      let observable = observable::testing::mock_observable(
        SchedulerType::Blocking,
        DispatcherType::Basic,
      );
      observable
        .pipe()
        .take_while(|x| x < 3)
        .assert(|x| x <= 3)
        .assert_count(3)
        .dangling();
      observable.next(1);
      observable.next(2);
      observable.next(3);
      assert_eq!(observable.num_children(), 0);
      observable.next(4);
    });
  }

  #[test]
  fn first_test() {
    utils::testing::async_context(|| {
      let observable = observable::testing::mock_observable(
        SchedulerType::Blocking,
        DispatcherType::Basic,
      );
      observable
        .pipe()
        .first()
        .assert(|x| x == 1)
        .assert_count(1)
        .dangling();
      observable.next(1);
      assert_eq!(observable.num_children(), 0);
      observable.next(2);
      observable.next(3);
      observable.next(4);
    });
  }

  #[test]
  fn skip_test() {
    utils::testing::async_context(|| {
      let observable = observable::testing::mock_observable(
        SchedulerType::Blocking,
        DispatcherType::Basic,
      );
      observable
        .pipe()
        .skip(2)
        .assert(|x| x > 2)
        .assert_count(2)
        .dangling();
      observable.next(1);
      observable.next(2);
      observable.next(3);
      observable.next(4);
    });
  }

  #[test]
  fn filter_test() {
    utils::testing::async_context(|| {
      let observable = observable::testing::mock_observable(
        SchedulerType::Blocking,
        DispatcherType::Basic,
      );
      observable
        .pipe()
        .filter(|x| x % 3 == 0)
        .assert(|x| x % 3 == 0)
        .assert_count(2)
        .dangling();
      observable.next(1);
      observable.next(2);
      observable.next(3);
      observable.next(4);
      observable.next(5);
      observable.next(6);
    });
  }

  #[test]
  fn collect_test() {
    utils::testing::async_context(|| {
      let observable = observable::testing::mock_observable(
        SchedulerType::Blocking,
        DispatcherType::Basic,
      );
      let rx = observable.pipe().take(3).assert_count(3).collect();
      observable.next(1);
      observable.next(2);
      observable.next(3);
      assert_eq!(observable.num_children(), 0);
      assert_eq!(rx.recv().unwrap(), vec![1, 2, 3]);
    });
  }
}
