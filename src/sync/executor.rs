use super::task::{Task, TaskType};

use std::sync::Arc;
use std::future::Future;

pub trait Executor {
  type Output: TaskType;
  
  fn execute(&self, future: impl Future<Output=Self::Output> + Send + 'static) -> Arc<Task<Self::Output>>;
}
