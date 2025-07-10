use std::{collections::HashMap, hash::Hash, sync::Arc, time::Duration};
use tokio::sync::broadcast;
use tracing::error;

/// Represents an event that transfers data from a sender to multiple receivers.
/// Receivers wait either for the event data or for the event to timeout.
pub struct Event<D> {
    notifier: broadcast::Sender<Option<D>>,
    internal_notifier: Arc<broadcast::Sender<Option<D>>>,
}

impl<D: Clone + Send + 'static> Event<D> {
    /// Create a new `Event` and start the countdown for the timeout.
    fn new(timeout: Duration) -> Self {
        // Use two broadcast channels. External data is sent through the first and
        // then echoed through the second, but the second channel might time out.
        
        // note: None will only be sent to first channel when the event is killed
        let (tx, mut rx) = broadcast::channel::<Option<D>>(1);
        let (internal_tx, _) = broadcast::channel::<Option<D>>(1);

        let internal_tx = Arc::new(internal_tx);
        let internal_tx_2 = internal_tx.clone();
        tokio::spawn(async move {
            let value = tokio::time::timeout(timeout, async {
                match rx.recv().await {
                    Ok(data) => Some(data),
                    Err(broadcast::error::RecvError::Closed) => None,
                    Err(e) => {
                        error!("Failed to receive event data: {}", e);
                        None
                    }
                }
            })
            .await;
            let result = if let Ok(Some(Some(data))) = value {
                internal_tx_2.send(Some(data))
            } else {
                internal_tx_2.send(None)
            };
            if let Err(e) = result {
                error!("Failed to propagate event data: {}", e);
            }
        });

        Self {
            notifier: tx,
            internal_notifier: internal_tx,
        }
    }

    /// Wait for data or timeout. None if timeout or error occurs.
    pub async fn listen(&self) -> Option<D> {
        match self.internal_notifier.subscribe().recv().await {
            Ok(Some(data)) => Some(data),
            Ok(None) => None,
            Err(broadcast::error::RecvError::Closed) => None,
            Err(e) => {
                error!("Failed to receive event data: {}", e);
                None
            }
        }
    }

    /// Send data to all listeners of this event.
    fn notify(&self, data: D) {
        if self.notifier.send(Some(data)).is_err() {
            error!("Failed to send event data");
        }
    }

    /// Make the event time out immediately.
    fn kill(&self) {
        if self.notifier.send(None).is_err() {
            error!("Failed to send kill event");
        }
    }

    /// True if the event hasn't yet timed out.
    fn alive(&self) -> bool {
        self.notifier.receiver_count() != 0
    }
}

/// EventHub is a collection (i.e. map) of events.
pub struct EventHub<E, D> {
    listeners: HashMap<E, Arc<Event<D>>>,
    timeout: Duration,
}

impl<E: Eq + Hash, D: Clone + Send + 'static> EventHub<E, D> {
    /// Create an empty `EventHub` with timeout to use for all created events.
    pub fn new(timeout: Duration) -> Self {
        EventHub {
            listeners: HashMap::new(),
            timeout,
        }
    }

    /// Either get an existing, alive event or create a new one.
    pub fn get_or_create_event(&mut self, event: E) -> Arc<Event<D>> {
        if let Some(existing_event) = self.listeners.get(&event) {
            if existing_event.alive() {
                return existing_event.clone();
            }
        }

        let waitable_event = Arc::new(Event::new(self.timeout));
        let waitable_event_2 = waitable_event.clone();
        self.listeners.insert(event, waitable_event);
        waitable_event_2
    }

    /// True if the event exists and is alive.
    pub fn has_event(&self, event: &E) -> bool {
        if let Some(waitable_event) = self.listeners.get(event) {
            return waitable_event.alive();
        } else {
            return false;
        }
    }

    /// Sends data to all listeners of the event.
    pub fn notify(&mut self, event: &E, data: D) -> Result<(), D> {
        if let Some(waitable_event) = self.listeners.remove(event) {
            if waitable_event.alive() {
                waitable_event.notify(data);
                return Ok(());
            }
        }
        return Err(data);
    }

    /// Removes all events from the hub and immediately times them out.
    pub fn clear(&mut self) {
        for waitable_event in self.listeners.values() {
            waitable_event.kill();
        }
        self.listeners.clear();
    }
}
