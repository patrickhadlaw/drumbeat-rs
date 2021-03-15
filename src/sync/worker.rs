use super::task::{TaskType, Task, TaskError};
use super::executor::Executor;

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::future::Future;

pub type WorkerResult<T> = Result<T, TaskError>; 

// panic if submit on non healthy worker

pub struct HealthUnwinder<'a, T>
where
  T: TaskType,
{
  flag: Arc<AtomicBool>,
  holding: Option<Arc<Task<T>>>,
  receiver: &'a Receiver<WorkerSignal<T>>,
}

impl <'a, T> Drop for HealthUnwinder<'a, T>
where
  T: TaskType,
{
  fn drop(&mut self) {
    if std::thread::panicking() {
      self.flag.store(false, Ordering::Relaxed);
      for signal in self.receiver.try_iter() {
        if let WorkerSignal::Run(task) = &signal {
          if let Ok(guard) = &mut task.inner.lock() {
            guard.ready = true;
            guard.result = Some(Err(TaskError::Unhealthy));
          }
        }
      }
    }
  }
}

enum WorkerSignal<T>
where
  T: TaskType,
{
  Run(Arc<Task<T>>),
  Close,
}

pub struct WorkerInner<T>
where
  T: TaskType,
{
  sender: Mutex<Sender<WorkerSignal<T>>>,
  queued: Arc<AtomicIsize>,
  healthy: Arc<AtomicBool>,
}

#[derive(Clone)]
pub struct Worker<T = ()>
where
  T: TaskType,
{
  inner: Arc<WorkerInner<T>>,
}

impl <T> Default for Worker<T>
where
  T: TaskType,
{
  fn default() -> Self {
    Self::new_with_handle().0
  }
}

impl <T> Worker<T>
where
  T: TaskType,
{
  fn new_with_handle() -> (Self, JoinHandle<()>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let queued = Arc::new(AtomicIsize::new(0));
    let healthy = Arc::new(AtomicBool::new(true));
    (
      Worker {
        inner: Arc::new(WorkerInner {
          sender: Mutex::new(tx),
          queued: queued.clone(),
          healthy: healthy.clone(),
        }),
      },
      Self::run(rx, queued, healthy),
    )
  }

  pub fn new() -> Self {
    Self::default()
  }

  fn run(
    receiver: Receiver<WorkerSignal<T>>,
    queued: Arc<AtomicIsize>,
    healthy: Arc<AtomicBool>
  ) -> JoinHandle<()> {
    static mut ID: AtomicUsize = AtomicUsize::new(0);
    let id = unsafe { ID.fetch_add(1, Ordering::Relaxed) };
    std::thread::Builder::new()
      .name(format!("worker{}", id))
      .spawn(move || {
        let mut unwinder = HealthUnwinder {
          flag: healthy,
          holding: None,
          receiver: &receiver,
        };
        while let WorkerSignal::Run(task) = receiver.recv().unwrap() {
          unwinder.holding = Some(task.clone());
          if Task::progress(task) {
            queued.fetch_add(-1, Ordering::Relaxed);
          }
          unwinder.holding.take();
        }
      })
      .unwrap()
  }

  /// Submits a given task to be run asynchronously
  ///
  /// `submit` sends the task to the worker thread using a channel
  ///
  /// - Tasks are guaranteed to be run as long as the worker remains healthy, when the `Worker` instance is
  /// dropped the dropping thread signals the worker to close. Once the worker
  /// receives this signal it quits its thread loop.
  /// 
  /// # Panics
  /// 
  /// Panics if the worker is unhealthy.
  ///
  /// # Example
  /// ```
  /// use drumbeat::sync::worker::Worker;
  /// use std::sync::{Arc, atomic::{AtomicU16, Ordering}};
  ///
  /// let atomic = Arc::new(AtomicU16::new(0));
  /// {
  ///   let worker = Worker::new();
  ///   let (copy1, copy2) = (atomic.clone(), atomic.clone());
  ///   worker.submit(move || { copy1.fetch_add(10, Ordering::Relaxed); });
  ///   worker.submit(move || { copy2.fetch_add(5, Ordering::Relaxed); });
  /// }
  /// assert_eq!(atomic.load(Ordering::Relaxed), 15);
  /// ```
  pub fn submit<F>(&self, job: impl FnOnce() -> F) -> Arc<Task<T>>
  where
    F: Future<Output = T> + Send + 'static,
  {
    self.execute(job())
  }

  pub fn idle(&self) -> bool {
    self.inner.queued.load(Ordering::Relaxed) == 0
  }

  pub fn healthy(&self) -> bool {
    self.inner.healthy.load(Ordering::Relaxed)
  }
}

impl <T> WorkerInner<T>
where
  T: TaskType,
{
  pub fn enqueue(&self, task: Arc<Task<T>>, resuming: bool) {
    let guard = self.sender.lock().unwrap();
    if self.healthy.load(Ordering::Relaxed) {
      if !resuming {
        self.queued.fetch_add(1, Ordering::Relaxed);
      }
      guard.send(WorkerSignal::Run(task)).unwrap();
    } else {
      let mut guard = task.inner.lock().unwrap();
      guard.result = Some(Err(TaskError::Unhealthy));
      guard.ready = true;
    }
  }
}

impl <T> Executor for Worker<T>
where
  T: TaskType,
{
  type Output = T;
  fn execute(&self, future: impl Future<Output = Self::Output> + Send + 'static) -> Arc<Task<Self::Output>> {
    let cloned = self.inner.clone();
    let task = Arc::new(Task::new(move |task| { cloned.enqueue(task, true) }, Box::pin(future)));
    self.inner.enqueue(task.clone(), false);
    task
  }
}

impl <T> Drop for Worker<T>
where T: TaskType {
  fn drop(&mut self) {
    let _ = self
      .inner
      .sender
      .lock()
      .unwrap()
      .send(WorkerSignal::Close);
  }
}

// pub struct WorkerHandle {
//   worker: Arc<Worker>,
//   pool: Weak<Pool>,
// }

// pub struct Pool {
//   workers: RwLock<Vec<Arc<Worker>>>,
//   available: RwLock<Vec<usize>>,
// }

// impl Pool {
//   pub fn new() -> Self {
//     Pool {
//       workers: RwLock::new(Vec::new()),
//       available: RwLock::new(Vec::new()),
//     }
//   }
// }

#[cfg(test)]
mod test {
  use super::*;
  use crate::utils::testing::async_context;

  #[test]
  fn worker_new_test() {
    async_context(|| {
      let worker: Worker = Worker::new();
      assert!(worker.inner.healthy.load(Ordering::Relaxed));
      assert!(worker.healthy());
      assert_eq!(worker.inner.queued.load(Ordering::Relaxed), 0);
      assert!(worker.idle());
      assert!(!worker.inner.sender.is_poisoned());
    });
  }

  #[test]
  fn worker_healthy_task_test() {
    async_context(|| {
      let (worker, handle) = Worker::<()>::new_with_handle();
      let task = worker.submit(async || {});
      worker.inner.sender.lock().unwrap().send(WorkerSignal::Close).unwrap();
      assert!(handle.join().is_ok());
      assert_eq!(task.poll(), Ok(()));
    });
  }

  #[test]
  #[should_panic]
  fn worker_unhealthy_task_test() {
    let (worker, handle) = Worker::<()>::new_with_handle();
    let task = worker.submit(async || { panic!() });
    worker.inner.sender.lock().unwrap().send(WorkerSignal::Close).unwrap();
    assert!(handle.join().is_err());
    assert_eq!(task.poll(), Ok(()));
  }
}
