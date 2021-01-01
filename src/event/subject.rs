use super::dispatcher::{create, DispatcherType};
use super::observable::{
  DummyOwner, Observable, ObservableType, Owner, Signal,
};
use super::scheduler::{make_scheduler, Scheduler, SchedulerType};
use crate::sync::threadpool::Task;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, RwLock, Weak};

pub trait Subject<T>
where
  T: ObservableType,
{
  fn observe(&self) -> Arc<Observable<T>>;
  fn next(&self, value: T);
}

pub struct ObserverManager<T>
where
  T: ObservableType,
{
  subject: Weak<dyn Owner>,
  observers: Vec<Arc<Observable<T>>>,
  scheduler: Arc<dyn Scheduler>,
  finalize: VecDeque<Task>,
  dispatch_type: DispatcherType,
}

impl<T> ObserverManager<T>
where
  T: ObservableType,
{
  fn new(scheduler: Arc<dyn Scheduler>, dispatch_type: DispatcherType) -> Self {
    ObserverManager {
      subject: Weak::<DummyOwner>::new(),
      observers: Vec::new(),
      scheduler,
      finalize: VecDeque::new(),
      dispatch_type,
    }
  }

  fn remove_child(&mut self, id: usize) -> Option<Arc<dyn Owner>> {
    let found = self.observers.iter().find(|child| child.id() != id);
    if let Some(item) = found {
      let result = item.clone();
      self.observers.retain(|child| child.id() != id);
      Some(result)
    } else {
      None
    }
  }

  fn finalize(&mut self) {
    while let Some(task) = self.finalize.pop_back() {
      task.invoke();
    }
  }

  fn add_finalize(&mut self, task: Task) {
    self.finalize.push_front(task)
  }
}

impl<T> Drop for ObserverManager<T>
where
  T: ObservableType,
{
  fn drop(&mut self) {
    self.finalize();
  }
}

pub struct SubjectBase<T>
where
  T: ObservableType,
{
  id: usize,
  manager: Arc<RwLock<ObserverManager<T>>>,
}

impl<T> SubjectBase<T>
where
  T: ObservableType,
{
  pub(super) fn new(
    strategy: SchedulerType,
    dispatch_type: DispatcherType,
  ) -> Self {
    let id = super::observable::id();
    SubjectBase {
      id,
      manager: Arc::new(RwLock::new(ObserverManager::new(
        make_scheduler("subject".to_owned(), id, strategy),
        dispatch_type,
      ))),
    }
  }

  fn set_owner(&self, owner: Arc<dyn Owner>) {
    self.manager.write().unwrap().subject = Arc::downgrade(&owner);
  }
}

impl<T> Owner for SubjectBase<T>
where
  T: ObservableType,
{
  fn id(&self) -> usize {
    self.id
  }

  fn finish(&self) {
    unimplemented!();
  }

  fn scheduler(&self) -> Arc<dyn Scheduler> {
    self.manager.read().unwrap().scheduler.clone()
  }

  fn add_finalize(&self, task: Task) {
    self.manager.write().unwrap().add_finalize(task);
  }

  fn handle(&self, _signal: Signal) {}
}

impl<T> Subject<T> for SubjectBase<T>
where
  T: ObservableType,
{
  fn observe(&self) -> Arc<Observable<T>> {
    let mut guard = self.manager.write().unwrap();
    let observable = Observable::new(
      super::observable::id(),
      Some(guard.subject.upgrade().unwrap()),
      create(guard.dispatch_type),
      guard.scheduler.clone(),
    );
    guard.observers.push(observable.clone());
    observable
  }

  fn next(&self, value: T) {
    let scheduler = self.scheduler();
    let manager = self.manager.clone();
    scheduler.execute(Task::new(Box::new(move || {
      let mut recycle: Option<Vec<usize>> = None;
      {
        let guard = manager.read().unwrap();
        for observable in guard.observers.iter() {
          observable.next(value.clone());
          if observable.finished() {
            if let Some(inner) = &mut recycle {
              inner.push(observable.id());
            } else {
              recycle = Some(Vec::new());
            }
          }
        }
      }
      if let Some(recycle) = &mut recycle {
        let mut guard = manager.write().unwrap();
        for id in recycle.iter() {
          guard.remove_child(*id);
        }
      }
    })));
  }
}

pub struct BasicSubjectBuilder {
  scheduler: SchedulerType,
  dispatcher: DispatcherType,
}

impl Default for BasicSubjectBuilder {
  fn default() -> Self {
    BasicSubjectBuilder {
      scheduler: SchedulerType::Runtime,
      dispatcher: DispatcherType::Basic,
    }
  }
}

impl BasicSubjectBuilder {
  pub fn new() -> Self {
    Self::default()
  }

  pub fn scheduler(mut self, scheduler: SchedulerType) -> Self {
    self.scheduler = scheduler;
    self
  }

  pub fn dispatcher(mut self, dispatcher: DispatcherType) -> Self {
    self.dispatcher = dispatcher;
    self
  }

  pub fn build<T>(self) -> Arc<BasicSubject<T>>
  where
    T: ObservableType,
  {
    let subject = Arc::new(BasicSubject {
      base: SubjectBase::new(self.scheduler, self.dispatcher),
    });
    subject.base.set_owner(subject.clone());
    subject
  }
}

pub struct BasicSubject<T>
where
  T: ObservableType,
{
  base: SubjectBase<T>,
}

impl<T> Owner for BasicSubject<T>
where
  T: ObservableType,
{
  fn id(&self) -> usize {
    self.base.id()
  }
  fn finish(&self) {
    self.base.finish()
  }
  fn scheduler(&self) -> Arc<dyn Scheduler> {
    self.base.scheduler()
  }
  fn add_finalize(&self, task: Task) {
    self.base.add_finalize(task)
  }
  fn handle(&self, signal: Signal) {
    self.base.handle(signal)
  }
}

impl<T> BasicSubject<T>
where
  T: ObservableType,
{
  pub fn new() -> Arc<Self> {
    BasicSubjectBuilder::new().build()
  }
}

impl<T> Subject<T> for BasicSubject<T>
where
  T: ObservableType,
{
  fn observe(&self) -> Arc<Observable<T>> {
    self.base.observe()
  }
  fn next(&self, value: T) {
    self.base.next(value)
  }
}

// pub struct PiggybackSubject

pub struct StateSubjectBuilder<T>
where
  T: ObservableType,
{
  scheduler: SchedulerType,
  dispatcher: DispatcherType,
  value: T,
}

impl<T> StateSubjectBuilder<T>
where
  T: ObservableType,
{
  pub fn new(value: T) -> Self {
    StateSubjectBuilder {
      scheduler: SchedulerType::Runtime,
      dispatcher: DispatcherType::Replay(1),
      value,
    }
  }

  pub fn scheduler(mut self, scheduler: SchedulerType) -> Self {
    self.scheduler = scheduler;
    self
  }

  pub fn dispatcher(mut self, dispatcher: DispatcherType) -> Self {
    self.dispatcher = dispatcher;
    self
  }

  pub fn build(self) -> Arc<StateSubject<T>> {
    let subject = Arc::new(StateSubject {
      base: SubjectBase::new(self.scheduler, self.dispatcher),
      state: Mutex::new(self.value),
    });
    subject.base.set_owner(subject.clone());
    subject
  }
}

pub struct StateSubject<T>
where
  T: ObservableType,
{
  base: SubjectBase<T>,
  state: Mutex<T>,
}

impl<T> Owner for StateSubject<T>
where
  T: ObservableType,
{
  fn id(&self) -> usize {
    self.base.id()
  }
  fn finish(&self) {
    self.base.finish()
  }
  fn scheduler(&self) -> Arc<dyn Scheduler> {
    self.base.scheduler()
  }
  fn add_finalize(&self, task: Task) {
    self.base.add_finalize(task)
  }
  fn handle(&self, signal: Signal) {
    self.base.handle(signal)
  }
}

impl<T> StateSubject<T>
where
  T: ObservableType,
{
  pub fn new(value: T) -> Arc<Self> {
    StateSubjectBuilder::new(value).build()
  }

  pub fn state(&self) -> T {
    self.state.lock().unwrap().clone()
  }
}

impl<T> Subject<T> for StateSubject<T>
where
  T: ObservableType,
{
  fn observe(&self) -> Arc<Observable<T>> {
    let observer = self.base.observe();
    observer.next(self.state.lock().unwrap().clone());
    observer
  }
  fn next(&self, value: T) {
    let mut guard = self.state.lock().unwrap();
    *guard = value.clone();
    self.base.next(value);
  }
}

pub struct QueuingSubjectBuilder {
  scheduler: SchedulerType,
  dispatcher: DispatcherType,
}

impl Default for QueuingSubjectBuilder {
  fn default() -> Self {
    QueuingSubjectBuilder {
      scheduler: SchedulerType::Runtime,
      dispatcher: DispatcherType::Basic,
    }
  }
}

impl QueuingSubjectBuilder {
  pub fn new() -> Self {
    Self::default()
  }

  pub fn scheduler(mut self, scheduler: SchedulerType) -> Self {
    self.scheduler = scheduler;
    self
  }

  pub fn dispatcher(mut self, dispatcher: DispatcherType) -> Self {
    self.dispatcher = dispatcher;
    self
  }

  pub fn build<T>(self) -> Arc<QueuingSubject<T>>
  where
    T: ObservableType,
  {
    let subject = Arc::new(QueuingSubject {
      base: SubjectBase::new(self.scheduler, self.dispatcher),
      queue: Mutex::new(VecDeque::new()),
    });
    subject.base.set_owner(subject.clone());
    subject
  }
}

pub struct QueuingSubject<T>
where
  T: ObservableType,
{
  base: SubjectBase<T>,
  queue: Mutex<VecDeque<T>>,
}

impl<T> Owner for QueuingSubject<T>
where
  T: ObservableType,
{
  fn id(&self) -> usize {
    self.base.id()
  }
  fn finish(&self) {
    self.base.finish()
  }
  fn scheduler(&self) -> Arc<dyn Scheduler> {
    self.base.scheduler()
  }
  fn add_finalize(&self, task: Task) {
    self.base.add_finalize(task)
  }
  fn handle(&self, signal: Signal) {
    self.base.handle(signal)
  }
}

impl<T> QueuingSubject<T>
where
  T: ObservableType,
{
  pub fn new() -> Arc<Self> {
    QueuingSubjectBuilder::new().build()
  }

  pub fn push(&self, value: T) {
    self.queue.lock().unwrap().push_back(value);
  }

  pub fn flush(&self) {
    for item in self.queue.lock().unwrap().iter() {
      self.next(item.clone());
    }
  }
}

impl<T> Subject<T> for QueuingSubject<T>
where
  T: ObservableType,
{
  fn observe(&self) -> Arc<Observable<T>> {
    self.base.observe()
  }
  fn next(&self, value: T) {
    self.base.next(value)
  }
}

#[cfg(test)]
mod test {
  use super::*;
  use crate::utils::testing::async_context;

  use std::sync::atomic::{AtomicIsize, Ordering};
  use std::sync::Arc;

  #[test]
  fn basic_subject_new_pool_test() {
    let basic = BasicSubjectBuilder::new()
      .scheduler(SchedulerType::Pool)
      .build::<()>();
    let read_guard = basic.base.manager.read().unwrap();
    assert!(read_guard.subject.upgrade().is_some());
    assert_eq!(read_guard.scheduler.scheduler_type(), SchedulerType::Pool);
  }

  #[test]
  fn basic_subject_new_runtime_test() {
    let basic = BasicSubjectBuilder::new()
      .scheduler(SchedulerType::Runtime)
      .build::<()>();
    let read_guard = basic.base.manager.read().unwrap();
    assert!(read_guard.subject.upgrade().is_some());
    assert_eq!(
      read_guard.scheduler.scheduler_type(),
      SchedulerType::Runtime
    );
  }

  #[test]
  fn basic_subject_new_blocking_test() {
    let basic = BasicSubjectBuilder::new()
      .scheduler(SchedulerType::Blocking)
      .build::<()>();
    let read_guard = basic.base.manager.read().unwrap();
    assert!(read_guard.subject.upgrade().is_some());
    assert_eq!(
      read_guard.scheduler.scheduler_type(),
      SchedulerType::Blocking
    );
  }

  #[test]
  fn basic_subject_next_test() {
    async_context(|| {
      let basic = BasicSubject::new();
      let (tx, rx) = std::sync::mpsc::channel();
      let tx = Mutex::new(tx.clone());
      let _subscription = basic.observe().pipe().subscribe(move |x| {
        tx.lock().unwrap().send(x).unwrap();
      });
      basic.next("test".to_owned());
      assert_eq!(rx.recv().unwrap(), "test");
    });
  }

  #[test]
  fn queuing_subject_new_pool_test() {
    let queuing = QueuingSubjectBuilder::new()
      .scheduler(SchedulerType::Pool)
      .build::<()>();
    let read_guard = queuing.base.manager.read().unwrap();
    assert!(read_guard.subject.upgrade().is_some());
    assert_eq!(read_guard.scheduler.scheduler_type(), SchedulerType::Pool);
  }

  #[test]
  fn queuing_subject_new_runtime_test() {
    let queuing = QueuingSubjectBuilder::new()
      .scheduler(SchedulerType::Runtime)
      .build::<()>();
    let read_guard = queuing.base.manager.read().unwrap();
    assert!(read_guard.subject.upgrade().is_some());
    assert_eq!(
      read_guard.scheduler.scheduler_type(),
      SchedulerType::Runtime
    );
  }

  #[test]
  fn queuing_subject_new_blocking_test() {
    let queuing = QueuingSubjectBuilder::new()
      .scheduler(SchedulerType::Blocking)
      .build::<()>();
    let read_guard = queuing.base.manager.read().unwrap();
    assert!(read_guard.subject.upgrade().is_some());
    assert_eq!(
      read_guard.scheduler.scheduler_type(),
      SchedulerType::Blocking
    );
  }

  #[test]
  fn queuing_subject_push_flush_test() {
    async_context(|| {
      let subject = QueuingSubjectBuilder::new()
        .scheduler(SchedulerType::Blocking)
        .build();
      let count = Arc::new(AtomicIsize::new(0));
      let clone = count.clone();
      let _subscription = subject.observe().pipe().subscribe(move |_| {
        clone.fetch_add(1, Ordering::Relaxed);
      });
      subject.push(());
      subject.push(());
      subject.push(());
      assert_eq!(count.load(Ordering::Relaxed), 0);
      subject.flush();
      assert_eq!(count.load(Ordering::Relaxed), 3);
    });
  }
}
