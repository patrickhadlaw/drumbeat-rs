use super::task::{Task, TaskType};

use std::future::Future;
use std::sync::Arc;

pub trait Executor {
  type Output: TaskType;

  fn execute(&self, future: impl Future<Output = Self::Output> + Send + 'static) -> Arc<Task<Self::Output>>;
}
