use crate::sync::threadpool::{Task, ThreadPool, ThreadPoolBuilder};
use crate::sync::worker::Worker;

use std::sync::Arc;

#[derive(Debug, PartialEq)]
pub enum SchedulerType {
  Worker,
  Pool,
  Runtime,
  Blocking,
}

pub trait Scheduler: Send + Sync {
  fn execute(&self, task: Task);
  fn scheduler_type(&self) -> SchedulerType;
}

impl Scheduler for Worker {
  fn execute(&self, task: Task) {
    self.submit_raw(task);
  }

  fn scheduler_type(&self) -> SchedulerType {
    SchedulerType::Worker
  }
}

impl Scheduler for ThreadPool {
  fn execute(&self, task: Task) {
    self.submit_raw(task);
  }

  fn scheduler_type(&self) -> SchedulerType {
    SchedulerType::Pool
  }
}

pub struct Runtime;

impl Scheduler for Runtime {
  fn execute(&self, task: Task) {
    crate::sync::runtime::Runtime::submit_raw(task);
  }

  fn scheduler_type(&self) -> SchedulerType {
    SchedulerType::Runtime
  }
}

pub struct Blocking;

impl Scheduler for Blocking {
  fn execute(&self, task: Task) {
    task.invoke();
  }

  fn scheduler_type(&self) -> SchedulerType {
    SchedulerType::Blocking
  }
}

pub(super) fn make_scheduler(
  name: String,
  id: usize,
  strategy: SchedulerType,
) -> Arc<dyn Scheduler> {
  match strategy {
    SchedulerType::Worker => Arc::new(Worker::new()),
    SchedulerType::Pool => {
      Arc::new(ThreadPoolBuilder::named(format!("{}{}", name, id)).build())
    }
    SchedulerType::Runtime => Arc::new(Runtime {}),
    SchedulerType::Blocking => Arc::new(Blocking {}),
  }
}
