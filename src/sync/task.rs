use super::spinlock::SpinLock;

use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

pub trait TaskType: Send + Clone + 'static {}
impl<T> TaskType for T where T: Send + Clone + 'static {}

#[derive(Debug, Clone, PartialEq)]
pub enum TaskError {
  Unhealthy,
}

impl Display for TaskError {
  fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
    match self {
      TaskError::Unhealthy => write!(f, "worker executor panicked"),
    }
  }
}

impl Error for TaskError {}

pub(super) struct TaskInner<T>
where
  T: TaskType,
{
  pub(super) ready: bool,
  pub(super) result: Option<Result<T, TaskError>>,
  pub(super) future: Option<Pin<Box<dyn Future<Output = T> + Send>>>,
}

impl<T> Wake for Task<T>
where
  T: TaskType,
{
  fn wake(self: Arc<Self>) {
    (self.resume)(self.clone());
  }
}

pub struct Task<T>
where
  T: TaskType,
{
  pub(super) inner: SpinLock<TaskInner<T>>,
  pub(super) resume: Box<dyn Fn(Arc<Task<T>>) + Send + Sync + 'static>,
}

#[derive(Debug, PartialEq)]
pub enum PollError {
  NotReady,
  Error(TaskError),
}

impl<T> Task<T>
where
  T: TaskType,
{
  pub fn new<F>(resumer: F, future: Pin<Box<dyn Future<Output = T> + Send>>) -> Self
  where
    F: Fn(Arc<Task<T>>) + Send + Sync + 'static,
  {
    Task {
      inner: SpinLock::new(TaskInner {
        ready: false,
        result: None,
        future: Some(future),
      }),
      resume: Box::new(resumer),
    }
  }

  pub fn poll(&self) -> Result<T, PollError> {
    let guard = self.inner.lock().unwrap();
    if guard.ready {
      guard.result.clone().unwrap().map_err(PollError::Error)
    } else {
      Err(PollError::NotReady)
    }
  }

  pub fn wait(&self) -> Result<T, TaskError> {
    // TODO: Implement non busy wait version
    loop {
      std::thread::yield_now();
      let poll = self.poll();
      if let Err(error) = poll {
        match error {
          PollError::NotReady => (),
          PollError::Error(error) => return Err(error),
        }
      } else {
        return Ok(poll.unwrap());
      }
    }
  }

  pub(super) fn progress(self: Arc<Self>) -> bool {
    if let Some(mut future) = self.inner.lock().unwrap().future.take() {
      let waker = Waker::from(self.clone());
      let context = &mut Context::from_waker(&waker);
      let poll = future.as_mut().poll(context);
      let mut guard = self.inner.lock().unwrap();
      if let Poll::Ready(value) = poll {
        guard.ready = true;
        guard.result = Some(Ok(value));
        true
      } else {
        guard.future = Some(future);
        false
      }
    } else {
      false
    }
  }
}
