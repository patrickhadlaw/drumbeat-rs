use super::dispatcher::{
  replicate, DispatchTarget, Dispatcher, DispatcherType, Invoker,
  SubscriptionDispatcher,
};
use super::scheduler::{make_scheduler, Scheduler, SchedulerType};
use super::subscription::Subscription;
use crate::sync::threadpool::Task;
use log::warn;

use std::fmt::Debug;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock, Weak};

pub trait ObservableType: Send + Sync + Clone + Debug + 'static {}

impl<T> ObservableType for T where T: Send + Sync + Clone + Debug + 'static {}

pub(super) trait Owner: Send + Sync {
  fn id(&self) -> usize;
  fn finish(&self);
  fn finished(&self) -> bool {
    false
  }
  fn scheduler(&self) -> Arc<dyn Scheduler>;
  fn add_finalize(&self, task: Task);
  fn handle(&self, signal: Signal);
  fn owner(&self) -> Option<Arc<dyn Owner>> {
    None
  }
  fn initialize(&self) {}
}

pub(super) struct DummyOwner;

impl Owner for DummyOwner {
  fn id(&self) -> usize {
    unimplemented!()
  }
  fn finish(&self) {
    unimplemented!()
  }
  fn scheduler(&self) -> Arc<dyn Scheduler> {
    unimplemented!()
  }
  fn add_finalize(&self, _task: Task) {
    unimplemented!()
  }
  fn handle(&self, _signal: Signal) {
    unimplemented!()
  }
  fn owner(&self) -> Option<Arc<dyn Owner>> {
    unimplemented!()
  }
  fn initialize(&self) {
    unimplemented!()
  }
}

type ResolverFn = dyn Fn() + Send + Sync;

#[derive(Clone)]
struct PipeResolver {
  func: Arc<ResolverFn>,
}

impl PipeResolver {
  fn new(func: Arc<ResolverFn>) -> Self {
    PipeResolver { func }
  }

  fn invoke(&self) {
    (self.func)()
  }
}

type Pipeable<T> = Arc<RwLock<Box<dyn Dispatcher<T>>>>;

/// A consumable event pipe constructed off of a root observable which can
/// be used to construct an event chain using various operations
///
/// - A pipe can be either forwarded or consumed.
///   - All forwarding operations will link a new observable in the chain, mark
///   the pipe as dead and return a brand new pipe. This allows you to continue
///   chaining operations without initiating the handling of events (this is to
///   prevent missed events).
///   - All consuming operations will link a new observable in the chain,
///   attach the chain to the root and do not return a pipe.
///
/// # Example
/// ```
/// # drumbeat::utils::testing::async_context(|| {
/// use drumbeat::event::observable::ObservableBuilder;
/// use drumbeat::event::scheduler::SchedulerType;
/// use drumbeat::event::ops::*;
/// use std::sync::Arc;
/// use std::sync::atomic::{AtomicUsize, Ordering};
///
/// let counter = Arc::new(AtomicUsize::new(0));
/// let capture = counter.clone();
/// ObservableBuilder::of(vec![1, 2, 3, 4, 5, 6])
///   .scheduler(SchedulerType::Blocking)
///   .build()
///   .pipe() // create pipe
///   .skip(2) // forwarding operations...
///   .take(2)
///   .tap(move |x| {
///     capture.fetch_add(x, Ordering::Relaxed);
///   })
///   .assert(|x| x > 2 && x < 5)
///   .assert_count(2)
///   .subscribe(|_| {}); // consuming operation
/// assert_eq!(counter.load(Ordering::Relaxed), 7);
/// });
/// ```
pub struct Pipe<T>
where
  T: ObservableType,
{
  destination: Option<Pipeable<T>>,
  pub(super) next: Pipeable<T>,
  resolver: Option<PipeResolver>,
  dead: bool,
}

impl<T> Pipe<T>
where
  T: ObservableType,
{
  fn new(
    destination: Option<Pipeable<T>>,
    next: Pipeable<T>,
    resolver: Option<PipeResolver>,
  ) -> Self {
    Pipe {
      destination,
      next,
      resolver,
      dead: false,
    }
  }

  pub(super) fn attach<A>(
    &mut self,
    observable: Arc<Observable<A>>,
    target: DispatchTarget<T>,
  ) -> Pipe<A>
  where
    A: ObservableType,
  {
    self.dead = true;
    let resolver = if self.resolver.is_some() {
      self.next.write().unwrap().add_child(target);
      if let Some(task) = self.next.read().unwrap().bootstrap(observable.id()) {
        task.invoke();
      }
      self.resolver.clone()
    } else {
      let destination = self.destination.take().unwrap();
      let cloned = observable.clone();
      let scheduler = self.scheduler();
      Some(PipeResolver::new(Arc::new(move || {
        destination.write().unwrap().add_child(target.clone());
        if let Some(task) = destination.read().unwrap().bootstrap(cloned.id()) {
          scheduler.execute(task);
        }
      })))
    };
    Pipe::<A>::new(None, observable.pipeable.clone(), resolver)
  }

  pub(super) fn forward(&mut self) -> Self {
    self.dead = true;
    Pipe::new(
      self.destination.clone(),
      self.next.clone(),
      self.resolver.clone(),
    )
  }

  pub(super) fn instantiate(&self) {
    if self.dead {
      panic!("observable pipe instantiated twice");
    }
    if let Some(resolver) = &self.resolver {
      resolver.invoke();
    } else {
      warn!("attempted to instantiate empty pipe");
    }
  }

  pub(super) fn scheduler(&self) -> Arc<dyn Scheduler> {
    self.next.read().unwrap().owner().unwrap().scheduler()
  }

  pub(super) fn make_observer<A>(&self) -> Arc<Observable<A>>
  where
    A: ObservableType,
  {
    let guard = self.next.read().unwrap();
    Observable::new(
      id(),
      Some(guard.owner().unwrap().clone()),
      replicate(guard.as_replicable()),
      guard.owner().unwrap().scheduler(),
    )
  }

  /// Consumes the pipe and produces an observer of the end of the current event
  /// chain
  ///
  /// `observe` is useful for multiplexing complex event chains without exposing
  /// the parent observer
  ///
  /// # Example
  /// ```
  /// # drumbeat::utils::testing::async_context(|| {
  /// use drumbeat::event::observable::Observable;
  /// use drumbeat::event::ops::*;
  ///
  /// let dimensions = Observable::of(vec![(600, 800)]);
  ///
  /// let width = move || {
  ///   dimensions.pipe().map(|x| x.0).observe()
  /// };
  ///
  /// let result = width()
  ///   .pipe()
  ///   .first()
  ///   .assert(|x| x == 600)
  ///   .collect();
  /// assert_eq!(result.recv().unwrap(), [600])
  /// });
  /// ```
  pub fn observe(&mut self) -> Arc<Observable<T>> {
    let observable = self.make_observer();
    let pipe = self.attach(
      observable.clone(),
      DispatchTarget::new(
        observable.clone(),
        Invoker::identity(observable.clone()),
      ),
    );
    pipe.instantiate();
    observable
  }

  /// Consumes the pipe and creates a subscription off the end of the chain
  ///
  /// `subscribe` is used for creating a subscription to the end of an observer
  /// chain. This subscription kills the leaf observable upon dropping. This
  /// ties the lifetime of the leaf of the observable chain to the subscription
  /// object.
  ///
  /// # Example
  /// ```
  /// # drumbeat::utils::testing::async_context(|| {
  /// use drumbeat::event::observable::Observable;
  /// use std::sync::Arc;
  /// use std::sync::atomic::{AtomicBool, Ordering};
  ///
  /// let finished = Arc::new(AtomicBool::new(false));
  /// let captured = finished.clone();
  ///
  /// {
  ///   let _subscription = Observable::of(vec![1, 2, 3])
  ///     .pipe()
  ///     .subscribe(|_| {})
  ///     .finalize(move || captured.store(true, Ordering::Relaxed));
  ///   assert_eq!(finished.load(Ordering::Relaxed), false);
  /// }
  /// assert_eq!(finished.load(Ordering::Relaxed), true);
  /// # });
  /// ```
  pub fn subscribe<F>(&mut self, consumer: F) -> Subscription
  where
    F: Fn(T) + Send + Sync + 'static,
  {
    let owner = self.next.write().unwrap().owner().unwrap();
    let observable = Observable::<T>::new(
      id(),
      Some(owner.clone()),
      Box::new(SubscriptionDispatcher::new(Invoker::new(Arc::new(
        move |x| {
          consumer(x);
          Signal::None
        },
      )))),
      owner.scheduler(),
    );
    let pipe = self.attach(
      observable.clone(),
      DispatchTarget::new(
        observable.clone(),
        Invoker::identity(observable.clone()),
      ),
    );
    pipe.instantiate();
    Subscription::new(Arc::downgrade(&observable) as Weak<dyn Owner>)
  }

  /// Sets the finalize task on the observable at the end of the chain and
  /// forwards the pipe
  ///
  /// `finalize` is used to perform cleanup tasks on recycling of the observable
  /// it was set on.
  pub fn finalize<F>(&mut self, task: F) -> Pipe<T>
  where
    F: Fn() + Send + Sync + 'static,
  {
    self
      .next
      .read()
      .unwrap()
      .add_finalize(Task::new(Box::new(task)));
    let observable = self.make_observer();
    self.attach(
      observable.clone(),
      DispatchTarget::new(observable.clone(), Invoker::identity(observable)),
    )
  }
}

#[derive(PartialEq, Copy, Clone)]
pub(super) enum Signal {
  None,
  Recycle(usize),
}

impl Signal {
  pub(super) fn is_none(&self) -> bool {
    *self == Signal::None
  }
}

/// An observable is the value observer in the observer pattern. It represents
/// a channel in the propagation of an event
pub struct Observable<T>
where
  T: ObservableType,
{
  id: usize,
  pipeable: Pipeable<T>,
  owner: Weak<dyn Owner>,
  scheduler: Arc<dyn Scheduler>,
  finished: AtomicBool,
}

impl<T> Owner for Observable<T>
where
  T: ObservableType,
{
  fn id(&self) -> usize {
    self.id
  }

  fn finish(&self) {
    if let Ok(last) = self.finished.compare_exchange(
      false,
      true,
      Ordering::Relaxed,
      Ordering::Relaxed,
    ) {
      if !last {
        self.pipeable.read().unwrap().finalize();
      }
    }
  }

  fn finished(&self) -> bool {
    self.finished.load(Ordering::Relaxed)
  }

  fn scheduler(&self) -> Arc<dyn Scheduler> {
    self.scheduler.clone()
  }

  fn add_finalize(&self, task: Task) {
    self.pipeable.read().unwrap().add_finalize(task);
  }

  fn handle(&self, signal: Signal) {
    match signal {
      Signal::None => (),
      Signal::Recycle(id) => {
        {
          let child = self.pipeable.write().unwrap().remove_child(id);
          if let Some(child) = child {
            child.finish();
          }
        }
        if self.num_children() == 0 {
          self.finish();
        }
      }
    }
  }

  fn owner(&self) -> Option<Arc<dyn Owner>> {
    self.owner.upgrade()
  }

  fn initialize(&self) {
    self.pipeable.read().unwrap().initialize();
  }
}

impl<T> Drop for Observable<T>
where
  T: ObservableType,
{
  fn drop(&mut self) {
    self.finish();
  }
}

pub(super) fn id() -> usize {
  static mut ID: AtomicUsize = AtomicUsize::new(0);
  unsafe { ID.fetch_add(1, Ordering::Relaxed) }
}

enum ObservableStrategy<T>
where
  T: ObservableType,
{
  Of(Vec<T>),
  Merge(Vec<Arc<Observable<T>>>),
}

pub struct ObservableBuilder<T>
where
  T: ObservableType,
{
  scheduler: SchedulerType,
  dispatcher: DispatcherType,
  strategy: ObservableStrategy<T>,
}

impl<T> ObservableBuilder<T>
where
  T: ObservableType,
{
  /// Builder for an observable of constant values, see
  /// [this method](Observable::of) for details
  pub fn of(list: Vec<T>) -> Self {
    ObservableBuilder {
      scheduler: SchedulerType::Worker,
      dispatcher: DispatcherType::Replay(list.len()),
      strategy: ObservableStrategy::Of(list),
    }
  }

  /// Builder for an observable which funnels a list of other observables, see
  /// [this method](Observable::merge) for details
  pub fn merge(list: Vec<Arc<Observable<T>>>) -> Self {
    ObservableBuilder {
      scheduler: SchedulerType::Worker,
      dispatcher: DispatcherType::Basic,
      strategy: ObservableStrategy::Merge(list),
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

  pub fn build(self) -> Arc<Observable<T>> {
    let id = id();
    let scheduler = make_scheduler("observable".to_owned(), id, self.scheduler);
    let dispatcher = super::dispatcher::create(self.dispatcher);
    match self.strategy {
      ObservableStrategy::Of(list) => {
        let observable =
          Observable::new(id, None, dispatcher, scheduler.clone());
        for x in list.iter() {
          let value = x.clone();
          let cloned = observable.clone();
          scheduler.execute(Task::new(move || {
            cloned.next(value.clone());
          }));
        }
        observable
      }
      ObservableStrategy::Merge(owners) => {
        Funnel::new(owners, id, scheduler, dispatcher).1
      }
    }
  }
}

impl<T> Observable<T>
where
  T: ObservableType,
{
  pub(super) fn new(
    id: usize,
    owner: Option<Arc<dyn Owner>>,
    dispatcher: Box<dyn Dispatcher<T>>,
    scheduler: Arc<dyn Scheduler>,
  ) -> Arc<Self> {
    let observable = Arc::new(Observable {
      id,
      pipeable: Arc::new(RwLock::new(dispatcher)),
      owner: match owner.clone() {
        Some(owner) => Arc::downgrade(&owner),
        None => Weak::<DummyOwner>::new(),
      },
      scheduler,
      finished: AtomicBool::new(false),
    });
    observable
      .pipeable
      .write()
      .unwrap()
      .set_owner(observable.clone() as Arc<dyn Owner>);
    observable
  }

  /// Constructs an observable which receives a merged stream from a given list
  /// of observables
  ///
  /// `merge` creates a funnel operator chaining all the parent observers into
  /// the resulting observable.
  ///
  /// # Example
  /// ```
  /// # drumbeat::utils::testing::async_context(|| {
  /// use drumbeat::event::observable::Observable;
  /// use drumbeat::event::ops::*;
  /// use std::sync::atomic::{AtomicU32, Ordering};
  /// use std::sync::Arc;
  ///
  /// let sum = Arc::new(AtomicU32::new(0));
  ///
  /// let a = Observable::of(vec![1, 2, 3, 4]);
  /// let b = Observable::of(vec![10, 20, 30, 40]);
  /// let c = Observable::of(vec![100, 200, 300, 400]);
  ///
  /// let capture = sum.clone();
  /// let rx = Observable::merge(vec![a, b, c])
  ///   .pipe()
  ///   .assert_count(12)
  ///   .tap(move |x| {
  ///     capture.fetch_add(x, Ordering::Relaxed);
  ///   })
  ///   .collect();
  ///
  /// assert_eq!(rx.recv().unwrap().iter().sum::<u32>(), 1110);
  /// assert_eq!(sum.load(Ordering::Relaxed), 1110);
  /// # });
  /// ```
  pub fn merge(list: Vec<Arc<Observable<T>>>) -> Arc<Self> {
    ObservableBuilder::merge(list).build()
  }

  /// Constructs an observable of a constant list of values
  ///
  /// `of` creates a replaying observable scheduled on a worker. This means that
  /// the ordering of the inputs are preserved and any attached observers will
  /// replay the constant list of values.
  ///
  /// # Example
  /// ```
  /// # drumbeat::utils::testing::async_context(|| {
  /// use drumbeat::event::observable::Observable;
  /// use drumbeat::event::ops::*;
  /// use std::sync::{Arc, Mutex};
  /// use std::sync::atomic::{AtomicUsize, Ordering};
  ///
  /// let counter = Arc::new(AtomicUsize::new(0));
  /// let capture = counter.clone();
  /// let (tx, rx) = std::sync::mpsc::channel();
  /// let tx = Mutex::new(tx);
  /// {
  ///   let observable = Observable::of(vec![1, 2, 3]);
  ///   // do some stuff...
  ///   observable
  ///     .pipe()
  ///     .tap(move |x| {
  ///       capture.fetch_add(x, Ordering::Relaxed);
  ///     })
  ///     .finalize(move || tx.lock().unwrap().send(()).unwrap())
  ///     .dangling();
  /// }
  /// rx.recv().unwrap();
  /// assert_eq!(counter.load(Ordering::Relaxed), 6);
  /// # });
  /// ```
  pub fn of(list: Vec<T>) -> Arc<Self> {
    ObservableBuilder::of(list).build()
  }

  /// Constructs a pipe rooted at this observable instance, this pipe can be
  /// used to route & resolve the propagation of events.
  ///
  /// # Example
  /// ```
  /// # drumbeat::utils::testing::async_context(|| {
  /// use drumbeat::event::observable::Observable;
  /// use drumbeat::event::ops::*;
  /// use std::sync::{Arc, Mutex};
  /// use std::sync::atomic::{AtomicUsize, Ordering};
  ///
  /// let counter = Arc::new(AtomicUsize::new(0));
  /// let capture = counter.clone();
  /// let rx = Observable::of(vec![1, 2, 3])
  ///   .pipe()
  ///   .tap(move |x| {
  ///     capture.fetch_add(x, Ordering::Relaxed);
  ///   })
  ///   .collect();
  /// rx.recv().unwrap();
  /// assert_eq!(counter.load(Ordering::Relaxed), 6);
  /// # });
  /// ```
  pub fn pipe(&self) -> Pipe<T> {
    Pipe::new(Some(self.pipeable.clone()), self.pipeable.clone(), None)
  }

  pub fn num_children(&self) -> usize {
    self.pipeable.read().unwrap().num_children()
  }

  pub fn unsubscribe(&self) {
    self.finish();
    let owner = self.owner();
    let id = self.id();
    crate::sync::runtime::Runtime::submit(move || {
      if let Some(owner) = owner.clone() {
        owner.handle(Signal::Recycle(id));
      }
    });
  }

  pub(super) fn next(&self, value: T) {
    if !self.finished() {
      let signals = self.pipeable.read().unwrap().dispatch(value);
      if let Some(signals) = signals {
        for signal in signals.iter() {
          self.handle(*signal);
        }
      }
    }
  }

  pub(super) fn finished(&self) -> bool {
    self.finished.load(Ordering::Relaxed)
  }
}

pub struct Funnel<T>
where
  T: ObservableType,
{
  id: usize,
  owners: Vec<Weak<dyn Owner>>,
  target: RwLock<Option<Arc<Observable<T>>>>,
  finished: AtomicBool,
}

impl<T> Owner for Funnel<T>
where
  T: ObservableType,
{
  fn id(&self) -> usize {
    self.id
  }

  fn finish(&self) {
    for owner in self.owners.iter() {
      if let Some(owner) = owner.upgrade() {
        owner.handle(Signal::Recycle(self.id()));
      }
    }
  }

  fn finished(&self) -> bool {
    self.finished.load(Ordering::Relaxed)
  }

  fn scheduler(&self) -> Arc<dyn Scheduler> {
    if let Some(owner) = self.owner() {
      owner.scheduler()
    } else {
      Arc::new(super::scheduler::Blocking {})
    }
  }

  fn add_finalize(&self, _task: Task) {
    unreachable!();
  }

  fn handle(&self, signal: Signal) {
    if let Signal::Recycle(_) = signal {
      self.recycle(None);
    }
  }

  fn owner(&self) -> Option<Arc<dyn Owner>> {
    self
      .owners
      .iter()
      .map(|x| x.upgrade())
      .find(|x| x.is_some())?
  }
}

impl<T> Funnel<T>
where
  T: ObservableType,
{
  pub(super) fn new(
    owners: Vec<Arc<Observable<T>>>,
    child_id: usize,
    scheduler: Arc<dyn Scheduler>,
    dispatcher: Box<dyn Dispatcher<T>>,
  ) -> (Arc<Self>, Arc<Observable<T>>) {
    let downgrade = owners
      .iter()
      .map(|x| Arc::downgrade(&x) as Weak<dyn Owner>)
      .collect();
    let funnel = Arc::new(Funnel {
      id: id(),
      owners: downgrade,
      target: RwLock::new(None),
      finished: AtomicBool::new(false),
    });
    let observable =
      Observable::new(child_id, Some(funnel.clone()), dispatcher, scheduler);
    *funnel.target.write().unwrap() = Some(observable.clone());
    for owner in owners.iter() {
      let capture = funnel.clone();
      let owner_weak = Arc::downgrade(owner);
      owner
        .pipeable
        .write()
        .unwrap()
        .add_child(DispatchTarget::new(
          funnel.clone(),
          Invoker::new(Arc::new(move |x| {
            capture.next(x, owner_weak.clone());
            Signal::None
          })),
        ));
    }
    (funnel, observable)
  }

  pub(super) fn next(&self, value: T, caller: Weak<dyn Owner>) {
    if !self.finished() {
      // NOTE: signal won't be produced here since target observable dispatcher
      // is inaccessible
      self.target.read().unwrap().as_ref().unwrap().next(value);
      if self.target.read().unwrap().as_ref().unwrap().finished() {
        self.recycle(Some(caller));
      }
    }
  }

  pub(super) fn recycle(&self, caller: Option<Weak<dyn Owner>>) {
    self.finish();
    let id = caller
      .map(|caller| caller.upgrade())
      .unwrap_or(None)
      .map(|caller| caller.id());
    for owner in self.owners.iter() {
      if let Some(owner) = owner.upgrade() {
        let not_same = id.map(|id| id != owner.id());
        if not_same.is_none() || not_same.unwrap() {
          owner.handle(Signal::Recycle(self.id()));
        }
      }
    }
  }
}

#[cfg(test)]
pub mod testing {
  use super::*;
  use crate::event::dispatcher::{create, DispatcherType};
  use crate::event::scheduler::{make_scheduler, SchedulerType};

  pub fn mock_observable<T>(
    strategy: SchedulerType,
    dispatcher: DispatcherType,
  ) -> Arc<Observable<T>>
  where
    T: ObservableType,
  {
    let id = id();
    let scheduler =
      make_scheduler("observable".to_owned(), id.clone(), strategy);
    Observable::new(id, None, create(dispatcher), scheduler)
  }
}

#[cfg(test)]
mod test {
  use super::*;
  use crate::event::dispatcher::DispatcherType;

  #[test]
  fn id_unique_test() {
    for _ in 0..100 {
      assert_ne!(id(), id());
    }
  }

  #[test]
  fn observable_of_test() {
    let observable = Observable::of(vec![1, 2, 3]);
    assert_eq!(
      observable.pipeable.read().unwrap().get_type(),
      DispatcherType::Replay(3)
    );
  }

  #[test]
  fn funnel_new_test() {
    crate::utils::testing::async_context(|| {
      let (funnel, _into) = {
        let observables: Vec<Arc<Observable<()>>> = vec![
          Observable::of(vec![]),
          Observable::of(vec![]),
          Observable::of(vec![]),
        ];
        let (funnel, into) = {
          let (funnel, observable) = Funnel::new(
            observables.clone(),
            id(),
            Arc::new(crate::event::scheduler::Blocking {}),
            crate::event::dispatcher::create(DispatcherType::Basic),
          );
          assert_eq!(funnel.owners.len(), 3);
          (Arc::downgrade(&funnel), observable)
        };
        for observer in observables.iter() {
          assert_eq!(observer.num_children(), 1);
        }
        assert!(funnel.upgrade().is_some());
        (funnel, into)
      };
      assert!(funnel.upgrade().is_none());
    });
  }

  #[test]
  fn funnel_target_unsubscribe_test() {
    crate::utils::testing::async_context(|| {
      let observables: Vec<Arc<Observable<()>>> = vec![
        Observable::of(vec![]),
        Observable::of(vec![]),
        Observable::of(vec![]),
      ];
      let (funnel, into) = {
        let (funnel, observable) = Funnel::new(
          observables.clone(),
          id(),
          Arc::new(crate::event::scheduler::Blocking {}),
          crate::event::dispatcher::create(DispatcherType::Basic),
        );
        assert_eq!(funnel.owners.len(), 3);
        (Arc::downgrade(&funnel), observable)
      };
      for observer in observables.iter() {
        assert_eq!(observer.num_children(), 1);
      }
      assert!(funnel.upgrade().is_some());
      into.unsubscribe();
      while !crate::sync::runtime::Runtime::done() {}
      assert!(funnel.upgrade().is_none());
      for observer in observables.iter() {
        assert_eq!(observer.num_children(), 0);
      }
    });
  }
}
