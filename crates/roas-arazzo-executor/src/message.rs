//! What an AsyncAPI step sends and receives, and the traits a caller
//! implements to carry messages.
//!
//! As with HTTP, the engine performs no IO: it says which channel to
//! publish to or listen on, and a [`MessageClient`] (or
//! [`AsyncMessageClient`]) speaks the protocol — Kafka, MQTT, AMQP,
//! WebSocket or whatever the AsyncAPI document's servers name.
//!
//! A receive step does not hold the workflow up. The engine asks the
//! client to [subscribe](MessageClient::subscribe) when the step is
//! reached and carries on; the client keeps what arrives on that
//! subscription until the engine [asks for it](MessageClient::receive) —
//! when a later step depends on the receive, or the workflow ends.

use crate::http::ClientError;
use serde_json::Value;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

/// Names one subscription for the rest of a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SubscriptionId(pub u64);

impl std::fmt::Display for SubscriptionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// The channel a step sends to or listens on, as its AsyncAPI document
/// describes it.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct Channel {
    /// The source description that holds the channel.
    pub source: String,
    /// The AsyncAPI operation the step named, when it named one rather
    /// than a `channelPath`.
    pub operation: Option<String>,
    /// The channel's address with its `{parameters}` filled in — a
    /// topic, queue or path, as the protocol reads it.
    pub address: String,
    /// The servers the channel is available on, by name, as the AsyncAPI
    /// document writes them (`host`, `protocol`, `pathname`, bindings…).
    pub servers: BTreeMap<String, Value>,
    /// The channel's protocol bindings, if the document gives any.
    pub bindings: Option<Value>,
}

/// A message a `send` step wants published.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct OutgoingMessage {
    /// Where it goes.
    pub channel: Channel,
    /// Headers, from the step's `in: header` parameters, in order.
    pub headers: Vec<(String, String)>,
    /// The encoded payload, from the step's `requestBody`.
    pub payload: Option<Vec<u8>>,
    /// The payload's media type, when there is a payload.
    pub content_type: Option<String>,
}

/// What a `receive` step listens on.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Subscription {
    /// The name later calls use for it.
    pub id: SubscriptionId,
    /// The channel to listen on.
    pub channel: Channel,
    /// Headers, from the step's `in: header` parameters — protocols that
    /// subscribe with a request (a WebSocket handshake, say) may use them.
    pub headers: Vec<(String, String)>,
}

/// A message that arrived on a subscription.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct IncomingMessage {
    /// Its headers, as the protocol carries them.
    pub headers: Vec<(String, String)>,
    /// Its payload, as received.
    pub payload: Vec<u8>,
}

impl IncomingMessage {
    /// A message with these headers and payload.
    #[must_use]
    pub fn new(headers: Vec<(String, String)>, payload: Vec<u8>) -> Self {
        Self { headers, payload }
    }

    /// A message whose payload is `payload` as JSON.
    #[must_use]
    pub fn json(payload: &Value) -> Self {
        Self {
            headers: Vec::new(),
            payload: serde_json::to_vec(payload).unwrap_or_default(),
        }
    }

    /// Add a header.
    #[must_use]
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// The payload as a value: JSON when it parses (a leading byte-order
    /// mark is ignored), its UTF-8 text otherwise, and nothing when it is
    /// empty or not UTF-8 — as a response body is read.
    pub(crate) fn payload_value(&self) -> Option<Value> {
        crate::http::bytes_value(&self.payload)
    }
}

/// Publishes messages and listens on channels.
pub trait MessageClient {
    /// Publish `message`. A send step is done once this returns: Arazzo
    /// does not model broker acknowledgement.
    ///
    /// # Errors
    ///
    /// Whatever kept the message from being sent.
    fn publish(&mut self, message: &OutgoingMessage) -> Result<(), ClientError>;

    /// Start listening. Messages that arrive before the engine asks for
    /// them must be kept, in order.
    ///
    /// # Errors
    ///
    /// Whatever kept the subscription from being made.
    fn subscribe(&mut self, subscription: &Subscription) -> Result<(), ClientError>;

    /// The next message on `subscription`, waiting at most `timeout`
    /// (`None`: as long as it takes). `Ok(None)` when nothing came in
    /// time. A zero timeout asks only for what has already arrived.
    ///
    /// # Errors
    ///
    /// Whatever broke the subscription.
    fn receive(
        &mut self,
        subscription: SubscriptionId,
        timeout: Option<Duration>,
    ) -> Result<Option<IncomingMessage>, ClientError>;

    /// Stop listening: the step is done with the subscription. When a run
    /// stops early this may name a subscription whose `subscribe` failed
    /// or was never asked for; ignore those.
    fn unsubscribe(&mut self, subscription: SubscriptionId);
}

/// The future an [`AsyncMessageClient`] returns from `publish` and
/// `subscribe`.
pub type DoneFuture<'a> = Pin<Box<dyn Future<Output = Result<(), ClientError>> + Send + 'a>>;

/// The future an [`AsyncMessageClient`] returns from `receive`.
pub type ReceiveFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<IncomingMessage>, ClientError>> + Send + 'a>>;

/// A [`MessageClient`] that does not block the thread.
pub trait AsyncMessageClient {
    /// As [`MessageClient::publish`].
    fn publish<'a>(&'a mut self, message: &'a OutgoingMessage) -> DoneFuture<'a>;

    /// As [`MessageClient::subscribe`].
    fn subscribe<'a>(&'a mut self, subscription: &'a Subscription) -> DoneFuture<'a>;

    /// As [`MessageClient::receive`].
    fn receive(
        &mut self,
        subscription: SubscriptionId,
        timeout: Option<Duration>,
    ) -> ReceiveFuture<'_>;

    /// As [`MessageClient::unsubscribe`].
    fn unsubscribe(&mut self, subscription: SubscriptionId);
}
