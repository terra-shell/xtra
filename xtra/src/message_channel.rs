//! A message channel is a channel through which you can send only one kind of message, but to
//! any actor that can handle it. It is like [`Address`], but associated with
//! the message type rather than the actor type.

use std::fmt;
use std::hash::{Hash, Hasher};

use crate::address::{ActorJoinHandle, Address};
use crate::chan::RefCounter;
use crate::refcount::{Either, Strong, Weak};
use crate::send_future::{ActorErasedSending, ResolveToHandlerReturn, SendFuture};
use crate::{Handler, WasmSend, WasmSendSync};

trait MessageChannelTraitWasm<M, Rc, R>: MessageChannelTrait<M, Rc, Return = R> + WasmSendSync {}
impl<M, Rc, R, T: MessageChannelTrait<M, Rc, Return = R> + WasmSendSync> MessageChannelTraitWasm<M, Rc, R> for T {}

/// A message channel is a channel through which you can send only one kind of message, but to
/// any actor that can handle it. It is like [`Address`], but associated with the message type rather
/// than the actor type.
///
/// # Example
///
/// ```rust
/// # use xtra::prelude::*;
/// struct WhatsYourName;
///
/// struct Alice;
/// struct Bob;
///
/// impl Actor for Alice {
///     type Stop = ();
///     async fn stopped(self) {
///         println!("Oh no");
///     }
/// }
/// #  impl Actor for Bob {type Stop = (); async fn stopped(self) -> Self::Stop {} }
///
/// impl Handler<WhatsYourName> for Alice {
///     type Return = &'static str;
///
///     async fn handle(&mut self, _: WhatsYourName, _ctx: &mut Context<Self>) -> Self::Return {
///         "Alice"
///     }
/// }
///
/// impl Handler<WhatsYourName> for Bob {
///     type Return = &'static str;
///
///     async fn handle(&mut self, _: WhatsYourName, _ctx: &mut Context<Self>) -> Self::Return {
///         "Bob"
///     }
/// }
///
/// fn main() {
/// # #[cfg(feature = "smol")]
/// smol::block_on(async {
///         let alice = xtra::spawn_smol(Alice, Mailbox::unbounded());
///         let bob = xtra::spawn_smol(Bob, Mailbox::unbounded());
///
///         let channels = [
///             MessageChannel::new(alice),
///             MessageChannel::new(bob)
///         ];
///         let name = ["Alice", "Bob"];
///
///         for (channel, name) in channels.iter().zip(&name) {
///             assert_eq!(*name, channel.send(WhatsYourName).await.unwrap());
///         }
///     })
/// }
/// ```
pub struct MessageChannel<M, R, Rc = Strong> {
  inner: Box<dyn MessageChannelTraitWasm<M, Rc, R> + 'static>,
}

impl<M, R, Rc> MessageChannel<M, R, Rc>
where
  M: WasmSend + 'static,
  R: WasmSend + 'static,
{
  /// Construct a new [`MessageChannel`] from the given [`Address`].
  ///
  /// The actor behind the [`Address`] must implement the [`Handler`] trait for the message type.
  pub fn new<A>(address: Address<A, Rc>) -> Self
  where
    A: Handler<M, Return = R>,
    Rc: RefCounter + Into<Either>,
  {
    Self {
      inner: Box::new(address),
    }
  }

  /// Returns whether the actor referred to by this message channel is running and accepting messages.
  pub fn is_connected(&self) -> bool {
    self.inner.is_connected()
  }

  /// Returns the number of messages in the actor's mailbox.
  ///
  /// Note that this does **not** differentiate between types of messages; it will return the
  /// count of all messages in the actor's mailbox, not only the messages sent by this
  /// [`MessageChannel`].
  pub fn len(&self) -> usize {
    self.inner.len()
  }

  /// The total capacity of the actor's mailbox.
  ///
  /// Note that this does **not** differentiate between types of messages; it will return the
  /// total capacity of actor's mailbox, not only the messages sent by this [`MessageChannel`].
  pub fn capacity(&self) -> Option<usize> {
    self.inner.capacity()
  }

  /// Returns whether the actor's mailbox is empty.
  pub fn is_empty(&self) -> bool {
    self.len() == 0
  }

  /// Send a message to the actor.
  ///
  /// This function returns a [`Future`](SendFuture) that resolves to the [`Return`](crate::Handler::Return) value of the handler.
  /// The [`SendFuture`] will resolve to [`Err(Disconnected)`] in case the actor is stopped and not accepting messages.
  pub fn send(&self, message: M) -> SendFuture<ActorErasedSending, ResolveToHandlerReturn<R>> {
    self.inner.send(message)
  }

  /// Waits until this [`MessageChannel`] becomes disconnected.
  pub fn join(&self) -> ActorJoinHandle {
    self.inner.join()
  }

  /// Determines whether this and the other [`MessageChannel`] address the same actor mailbox.
  pub fn same_actor<Rc2>(&self, other: &MessageChannel<M, R, Rc2>) -> bool
  where
    Rc2: WasmSend + 'static,
  {
    self.inner.to_inner_ptr() == other.inner.to_inner_ptr()
  }
}

#[cfg(feature = "sink")]
impl<M, Rc> MessageChannel<M, (), Rc>
where
  M: WasmSend + 'static,
{
  /// Construct a [`Sink`] from this [`MessageChannel`].
  ///
  /// Sending an item into a [`Sink`]s does not return a value. Consequently, this function is
  /// only available on [`MessageChannel`]s with a return value of `()`.
  ///
  /// To create such a [`MessageChannel`] use an [`Address`] that points to an actor where the
  /// [`Handler`] of a given message has [`Return`](Handler::Return) set to `()`.
  ///
  /// The provided [`Sink`] will process one message at a time completely and thus enforces
  /// back-pressure according to the bounds of the actor's mailbox.
  ///
  /// [`Sink`]: futures_sink::Sink
  pub fn into_sink(self) -> impl futures_sink::Sink<M, Error = crate::Error> {
    futures_util::sink::unfold((), move |(), message| self.send(message))
  }
}

impl<A, M, R, Rc> From<Address<A, Rc>> for MessageChannel<M, R, Rc>
where
  A: Handler<M, Return = R>,
  R: WasmSend + 'static,
  M: WasmSend + 'static,
  Rc: RefCounter + Into<Either>,
{
  fn from(address: Address<A, Rc>) -> Self {
    MessageChannel::new(address)
  }
}

impl<M, R, Rc> fmt::Debug for MessageChannel<M, R, Rc>
where
  R: WasmSend + 'static,
{
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let actor_type = self.inner.actor_type();
    let message_type = &std::any::type_name::<M>();
    let return_type = &std::any::type_name::<R>();
    let rc_type = &std::any::type_name::<Rc>().replace("xtra::chan::ptr::", "").replace("Tx", "");

    f.debug_struct(&format!(
      "MessageChannel<{}, {}, {}, {}>",
      actor_type, message_type, return_type, rc_type
    ))
    .field("addresses", &self.inner.sender_count())
    .field("mailboxes", &self.inner.receiver_count())
    .finish()
  }
}

/// Determines whether this and the other message channel address the same actor mailbox **and**
/// they have reference count type equality. This means that this will only return true if
/// [`MessageChannel::same_actor`] returns true **and** if they both have weak or strong reference
/// counts. [`Either`] will compare as whichever reference count type it wraps.
impl<M, R, Rc> PartialEq for MessageChannel<M, R, Rc>
where
  M: WasmSend + 'static,
  R: WasmSend + 'static,
  Rc: WasmSend + 'static,
{
  fn eq(&self, other: &Self) -> bool {
    self.same_actor(other) && (self.inner.is_strong() == other.inner.is_strong())
  }
}

impl<M, R, Rc: RefCounter> Hash for MessageChannel<M, R, Rc>
where
  M: WasmSend + 'static,
  R: WasmSend + 'static,
  Rc: WasmSend + 'static,
{
  fn hash<H: Hasher>(&self, state: &mut H) {
    self.inner.hash(state)
  }
}

impl<M, R, Rc> Clone for MessageChannel<M, R, Rc>
where
  R: WasmSend + 'static,
{
  fn clone(&self) -> Self {
    Self {
      inner: self.inner.clone_channel(),
    }
  }
}

impl<M, R> MessageChannel<M, R, Strong>
where
  M: WasmSend + 'static,
  R: WasmSend + 'static,
{
  /// Downgrade this [`MessageChannel`] to a [`Weak`] reference count.
  pub fn downgrade(&self) -> MessageChannel<M, R, Weak> {
    MessageChannel {
      inner: self.inner.to_weak(),
    }
  }
}

impl<M, R> MessageChannel<M, R, Either>
where
  M: WasmSend + 'static,
  R: WasmSend + 'static,
{
  /// Downgrade this [`MessageChannel`] to a [`Weak`] reference count.
  pub fn downgrade(&self) -> MessageChannel<M, R, Weak> {
    MessageChannel {
      inner: self.inner.to_weak(),
    }
  }
}

/// Functions which apply to any kind of [`MessageChannel`], be they strong or weak.
impl<M, R, Rc> MessageChannel<M, R, Rc>
where
  M: WasmSend + 'static,
  R: WasmSend + 'static,
{
  /// Convert this [`MessageChannel`] to allow [`Either`] reference counts.
  pub fn as_either(&self) -> MessageChannel<M, R, Either> {
    MessageChannel {
      inner: self.inner.to_either(),
    }
  }
}

trait MessageChannelTrait<M, Rc> {
  type Return: WasmSend + 'static;

  fn is_connected(&self) -> bool;

  fn len(&self) -> usize;

  fn capacity(&self) -> Option<usize>;

  fn send(&self, message: M) -> SendFuture<ActorErasedSending, ResolveToHandlerReturn<Self::Return>>;

  fn clone_channel(&self) -> Box<dyn MessageChannelTraitWasm<M, Rc, Self::Return> + 'static>;

  fn join(&self) -> ActorJoinHandle;

  fn to_inner_ptr(&self) -> *const ();

  fn is_strong(&self) -> bool;

  fn to_weak(&self) -> Box<dyn MessageChannelTraitWasm<M, Weak, Self::Return> + 'static>;

  fn sender_count(&self) -> usize;

  fn receiver_count(&self) -> usize;

  fn actor_type(&self) -> &str;

  fn to_either(&self) -> Box<dyn MessageChannelTraitWasm<M, Either, Self::Return> + 'static>;

  fn hash(&self, state: &mut dyn Hasher);
}

impl<A, R, M, Rc: RefCounter> MessageChannelTrait<M, Rc> for Address<A, Rc>
where
  A: Handler<M, Return = R>,
  M: WasmSend + 'static,
  R: WasmSend + 'static,
  Rc: Into<Either>,
{
  type Return = R;

  fn is_connected(&self) -> bool {
    self.is_connected()
  }

  fn len(&self) -> usize {
    self.len()
  }

  fn capacity(&self) -> Option<usize> {
    self.capacity()
  }

  fn send(&self, message: M) -> SendFuture<ActorErasedSending, ResolveToHandlerReturn<R>> {
    SendFuture::sending_erased(message, self.0.clone())
  }

  fn clone_channel(&self) -> Box<dyn MessageChannelTraitWasm<M, Rc, Self::Return> + 'static> {
    Box::new(self.clone())
  }

  fn join(&self) -> ActorJoinHandle {
    self.join()
  }

  fn to_inner_ptr(&self) -> *const () {
    self.0.inner_ptr()
  }

  fn is_strong(&self) -> bool {
    self.0.is_strong()
  }

  fn to_weak(&self) -> Box<dyn MessageChannelTraitWasm<M, Weak, Self::Return> + 'static> {
    Box::new(Address(self.0.to_tx_weak()))
  }

  fn sender_count(&self) -> usize {
    self.0.sender_count()
  }

  fn receiver_count(&self) -> usize {
    self.0.receiver_count()
  }

  fn actor_type(&self) -> &str {
    std::any::type_name::<A>()
  }

  fn to_either(&self) -> Box<dyn MessageChannelTraitWasm<M, Either, Self::Return> + 'static>
  where
    Rc: RefCounter + Into<Either>,
  {
    Box::new(Address(self.0.to_tx_either()))
  }

  fn hash(&self, state: &mut dyn Hasher) {
    state.write_usize(self.0.inner_ptr() as *const _ as usize);
    state.write_u8(self.0.is_strong() as u8);
    let _ = state.finish();
  }
}

#[cfg(test)]
mod test {
  use std::hash::{Hash, Hasher};

  use crate::{Actor, Handler, Mailbox};

  type TestMessageChannel = super::MessageChannel<TestMessage, ()>;

  struct TestActor;
  struct TestMessage;

  impl Actor for TestActor {
    type Stop = ();

    async fn stopped(self) -> Self::Stop {}
  }

  impl Handler<TestMessage> for TestActor {
    type Return = ();

    async fn handle(&mut self, _: TestMessage, _: &mut crate::Context<Self>) -> Self::Return {}
  }

  struct RecordingHasher(Vec<u8>);

  impl RecordingHasher {
    fn record_hash<H: Hash>(value: &H) -> Vec<u8> {
      let mut h = Self(Vec::new());
      value.hash(&mut h);
      assert!(!h.0.is_empty(), "the hash data not be empty");
      h.0
    }
  }

  impl Hasher for RecordingHasher {
    fn finish(&self) -> u64 {
      0
    }

    fn write(&mut self, bytes: &[u8]) {
      self.0.extend_from_slice(bytes)
    }
  }

  #[test]
  fn hashcode() {
    let (a1, _) = Mailbox::<TestActor>::unbounded();
    let c1 = TestMessageChannel::new(a1.clone());

    let h1 = RecordingHasher::record_hash(&c1);
    let h2 = RecordingHasher::record_hash(&c1.clone());
    let h3 = RecordingHasher::record_hash(&TestMessageChannel::new(a1));

    assert_eq!(h1, h2, "hashes from cloned channels should match");
    assert_eq!(h1, h3, "hashes channels created against the same address should match");

    let h4 = RecordingHasher::record_hash(&TestMessageChannel::new(Mailbox::<TestActor>::unbounded().0));

    assert_ne!(
      h1, h4,
      "hashes from channels created against different addresses should differ"
    );
  }

  #[test]
  fn partial_eq() {
    let (a1, _) = Mailbox::<TestActor>::unbounded();
    let c1 = TestMessageChannel::new(a1.clone());
    let c2 = c1.clone();
    let c3 = TestMessageChannel::new(a1);

    assert_eq!(c1, c2, "cloned channels should match");
    assert_eq!(c1, c3, "channels created against the same address should match");

    let c4 = TestMessageChannel::new(Mailbox::<TestActor>::unbounded().0);

    assert_ne!(c1, c4, "channels created against different addresses should differ");
  }
}
