//! Background task that merges Crossterm input with a fixed-rate tick into a
//! stream of `AppEvent`s.

use std::io;
use std::time::Duration;

use crossterm::event::{Event, EventStream};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;
use tokio_stream::{Stream, StreamExt};

use super::AppEvent;

const TICK_RATE: Duration = Duration::from_millis(16);

/// Unbounded: input/tick events are low-rate and drained every loop
/// iteration, unlike the bounded fs-scan channel in `src/fs`.
#[derive(Debug)]
pub struct EventHandler {
    receiver: UnboundedReceiver<AppEvent>,
    task: JoinHandle<()>,
}

impl EventHandler {
    pub fn new() -> Self {
        Self::spawn(EventStream::new(), TICK_RATE)
    }

    /// Generic over the input stream so tests can inject a fake one instead
    /// of a real TTY.
    fn spawn<S>(input: S, tick_rate: Duration) -> Self
    where
        S: Stream<Item = io::Result<Event>> + Unpin + Send + 'static,
    {
        let (sender, receiver) = mpsc::unbounded_channel();
        let task = tokio::spawn(Self::run(sender, input, tick_rate));
        Self { receiver, task }
    }

    async fn run<S>(sender: UnboundedSender<AppEvent>, mut input: S, tick_rate: Duration)
    where
        S: Stream<Item = io::Result<Event>> + Unpin + Send + 'static,
    {
        let mut tick = tokio::time::interval(tick_rate);

        loop {
            let tick_delay = tick.tick();
            let next_input = input.next();

            tokio::select! {
                _ = tick_delay => {
                    if sender.send(AppEvent::Tick).is_err() {
                        break;
                    }
                }
                maybe_event = next_input => {
                    let outcome = match maybe_event {
                        Some(Ok(event)) => sender.send(AppEvent::Input(event)),
                        Some(Err(err)) => sender.send(AppEvent::Error(err.to_string())),
                        None => {
                            let _ = sender.send(AppEvent::Error(
                                "terminal input stream closed".to_string(),
                            ));
                            break;
                        }
                    };
                    if outcome.is_err() {
                        break;
                    }
                }
            }
        }
    }

    pub async fn next(&mut self) -> anyhow::Result<AppEvent> {
        self.receiver
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("event channel closed: background task ended"))
    }
}

impl Drop for EventHandler {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tokio::time::timeout;

    use super::*;

    /// Never resolves; isolates tick tests from real input.
    struct PendingInput;

    impl Stream for PendingInput {
        type Item = io::Result<Event>;

        fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            Poll::Pending
        }
    }

    fn key_event(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    const ASSERT_TIMEOUT: Duration = Duration::from_millis(500);

    async fn next_or_timeout(handler: &mut EventHandler) -> anyhow::Result<AppEvent> {
        timeout(ASSERT_TIMEOUT, handler.next())
            .await
            .expect("timed out waiting for an event")
    }

    #[tokio::test]
    async fn emits_ticks_on_a_fixed_interval() {
        let mut handler = EventHandler::spawn(PendingInput, Duration::from_millis(5));

        for _ in 0..3 {
            let event = next_or_timeout(&mut handler)
                .await
                .expect("event channel unexpectedly closed");
            assert!(matches!(event, AppEvent::Tick));
        }
    }

    #[tokio::test]
    async fn forwards_input_events_in_order() {
        let items: Vec<io::Result<Event>> = vec![
            Ok(key_event(KeyCode::Char('a'))),
            Ok(key_event(KeyCode::Char('b'))),
            Err(io::Error::other("boom")),
        ];
        // Slow tick keeps `Tick`s from interleaving with this fixed sequence.
        let mut handler = EventHandler::spawn(tokio_stream::iter(items), Duration::from_secs(10));

        let first = next_or_timeout(&mut handler).await.unwrap();
        assert!(matches!(
            first,
            AppEvent::Input(Event::Key(k)) if k.code == KeyCode::Char('a')
        ));

        let second = next_or_timeout(&mut handler).await.unwrap();
        assert!(matches!(
            second,
            AppEvent::Input(Event::Key(k)) if k.code == KeyCode::Char('b')
        ));

        let third = next_or_timeout(&mut handler).await.unwrap();
        assert!(matches!(third, AppEvent::Error(ref msg) if msg.contains("boom")));
    }

    #[tokio::test]
    async fn reports_closed_input_stream_then_ends_task() {
        let empty: Vec<io::Result<Event>> = Vec::new();
        let mut handler = EventHandler::spawn(tokio_stream::iter(empty), Duration::from_secs(10));

        let event = next_or_timeout(&mut handler).await.unwrap();
        assert!(matches!(event, AppEvent::Error(ref msg) if msg.contains("closed")));

        let closed = next_or_timeout(&mut handler).await;
        assert!(closed.is_err());
    }

    #[tokio::test]
    async fn drop_aborts_background_task_promptly() {
        let handler = EventHandler::spawn(PendingInput, Duration::from_secs(10));
        let abort_handle = handler.task.abort_handle();
        assert!(!abort_handle.is_finished());

        drop(handler);

        for _ in 0..50 {
            if abort_handle.is_finished() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("background task was not aborted promptly after EventHandler was dropped");
    }
}
