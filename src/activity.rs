use crate::{aura::AuraApi, config::Config};
use anyhow::Result;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashSet, VecDeque},
    env,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::{Mutex, Notify, mpsc, watch},
    task::JoinHandle,
};
use tokio_stream::{Stream, StreamExt};

pub const ACTIVITY_URI: &str = "aura://user_activity/latest";
const DEFAULT_MAX_BUFFERED: usize = 1024;
const DEFAULT_PING_INTERVAL_MS: u64 = 10_000;
const MAX_READ_LIMIT: usize = 500;
const INITIAL_BACKOFF: Duration = Duration::from_millis(300);
const MAX_BACKOFF: Duration = Duration::from_secs(5);

type ActivityStream = Pin<Box<dyn Stream<Item = std::result::Result<Value, String>> + Send>>;

#[derive(Clone, Debug, Serialize)]
pub struct ActivityItem {
    pub seq: u64,
    pub ts_ms: u64,
    pub event: Value,
}

#[derive(Debug, Serialize)]
pub struct ActivityStartStatus {
    pub running: bool,
    pub already_running: bool,
    pub last_seq: u64,
    pub buffered: usize,
    pub dropped: u64,
}

#[derive(Debug, Serialize)]
pub struct ActivityReadResponse {
    pub running: bool,
    pub events: Vec<ActivityItem>,
    pub last_seq: u64,
    pub buffered: usize,
    pub dropped: u64,
}

#[derive(Debug, Serialize)]
pub struct ActivityStatus {
    pub running: bool,
    pub last_seq: u64,
    pub buffered: usize,
    pub dropped: u64,
    pub last_error: Option<String>,
    pub last_event_ts_ms: Option<u64>,
    pub last_ping_ts_ms: Option<u64>,
    pub subscribed: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadActivityArgs {
    #[serde(default)]
    pub after_seq: u64,
    #[serde(default = "default_read_limit")]
    pub limit: usize,
}

fn default_read_limit() -> usize {
    100
}

#[derive(Default)]
struct ActivityState {
    running: bool,
    shutdown: Option<watch::Sender<bool>>,
    handle: Option<JoinHandle<()>>,
    last_error: Option<String>,
    last_event_ts_ms: Option<u64>,
    last_ping_ts_ms: Option<u64>,
}

struct ActivityInner<F = AuraActivityClientFactory>
where
    F: ActivityClientFactory,
{
    factory: F,
    state: Mutex<ActivityState>,
    next_seq: AtomicU64,
    last_seq: AtomicU64,
    dropped: AtomicU64,
    buffer: Mutex<VecDeque<ActivityItem>>,
    max_buffered: usize,
    event_notify: Notify,
    subscriptions: Mutex<HashSet<String>>,
    notifications: mpsc::UnboundedSender<Value>,
    ping_interval: Duration,
    manager_tasks_started: AtomicU64,
}

pub struct ActivityManager<F = AuraActivityClientFactory>
where
    F: ActivityClientFactory,
{
    inner: Arc<ActivityInner<F>>,
}

impl<F> Clone for ActivityManager<F>
where
    F: ActivityClientFactory,
{
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

#[async_trait]
pub trait ActivityClientFactory: Send + Sync + 'static {
    type Client: ActivityClient;

    async fn connect(&self) -> Result<Self::Client>;
}

#[async_trait]
pub trait ActivityClient: Send {
    async fn open_user_activity(&mut self) -> Result<ActivityStream>;
    async fn user_ping(&mut self) -> Result<()>;
}

pub struct AuraActivityClientFactory {
    config: Config,
}

impl AuraActivityClientFactory {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

pub struct AuraActivityClient {
    api: AuraApi,
}

#[async_trait]
impl ActivityClientFactory for AuraActivityClientFactory {
    type Client = AuraActivityClient;

    async fn connect(&self) -> Result<Self::Client> {
        Ok(AuraActivityClient {
            api: AuraApi::connect(&self.config).await?,
        })
    }
}

#[async_trait]
impl ActivityClient for AuraActivityClient {
    async fn open_user_activity(&mut self) -> Result<ActivityStream> {
        let stream = self.api.open_user_activity().await?;
        Ok(Box::pin(stream.map(|item| {
            item.map_err(|err| err.to_string())
                .and_then(|event| serde_json::to_value(event).map_err(|err| err.to_string()))
        })))
    }

    async fn user_ping(&mut self) -> Result<()> {
        self.api.user_ping_internal().await
    }
}

impl ActivityManager<AuraActivityClientFactory> {
    pub fn new(config: Config, notifications: mpsc::UnboundedSender<Value>) -> Self {
        let max_buffered = env_usize("AURA_MCP_ACTIVITY_BUFFER", DEFAULT_MAX_BUFFERED).max(1);
        let ping_interval = Duration::from_millis(
            env_u64("AURA_MCP_USER_PING_INTERVAL_MS", DEFAULT_PING_INTERVAL_MS).max(1),
        );
        Self::with_factory(
            AuraActivityClientFactory::new(config),
            notifications,
            max_buffered,
            ping_interval,
        )
    }
}

impl<F> ActivityManager<F>
where
    F: ActivityClientFactory,
{
    pub fn with_factory(
        factory: F,
        notifications: mpsc::UnboundedSender<Value>,
        max_buffered: usize,
        ping_interval: Duration,
    ) -> Self {
        Self {
            inner: Arc::new(ActivityInner {
                factory,
                state: Mutex::new(ActivityState::default()),
                next_seq: AtomicU64::new(0),
                last_seq: AtomicU64::new(0),
                dropped: AtomicU64::new(0),
                buffer: Mutex::new(VecDeque::with_capacity(max_buffered.max(1))),
                max_buffered: max_buffered.max(1),
                event_notify: Notify::new(),
                subscriptions: Mutex::new(HashSet::new()),
                notifications,
                ping_interval,
                manager_tasks_started: AtomicU64::new(0),
            }),
        }
    }

    pub async fn start(&self) -> ActivityStartStatus {
        let mut state = self.inner.state.lock().await;
        if state.running {
            drop(state);
            let status = self.status().await;
            return ActivityStartStatus {
                running: status.running,
                already_running: true,
                last_seq: status.last_seq,
                buffered: status.buffered,
                dropped: status.dropped,
            };
        }

        let (shutdown, shutdown_rx) = watch::channel(false);
        state.running = true;
        state.shutdown = Some(shutdown);
        state.last_error = None;
        let manager = self.clone();
        self.inner
            .manager_tasks_started
            .fetch_add(1, Ordering::Relaxed);
        state.handle = Some(tokio::spawn(async move {
            manager.run_stream_manager(shutdown_rx).await;
        }));
        drop(state);

        let status = self.status().await;
        ActivityStartStatus {
            running: status.running,
            already_running: false,
            last_seq: status.last_seq,
            buffered: status.buffered,
            dropped: status.dropped,
        }
    }

    pub async fn stop(&self) -> Value {
        let (shutdown, handle) = {
            let mut state = self.inner.state.lock().await;
            state.running = false;
            (state.shutdown.take(), state.handle.take())
        };

        if let Some(shutdown) = shutdown {
            let _ = shutdown.send(true);
        }
        if let Some(mut handle) = handle
            && tokio::time::timeout(Duration::from_secs(2), &mut handle)
                .await
                .is_err()
        {
            handle.abort();
        }
        json!({ "running": false })
    }

    pub async fn read(&self, args: ReadActivityArgs) -> ActivityReadResponse {
        if !self.status().await.running {
            self.start().await;
        }

        let limit = args.limit.clamp(1, MAX_READ_LIMIT);
        let running = self.is_running().await;
        let buffer = self.inner.buffer.lock().await;
        let events = buffer
            .iter()
            .filter(|item| item.seq > args.after_seq)
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        ActivityReadResponse {
            running,
            events,
            last_seq: self.inner.last_seq.load(Ordering::Relaxed),
            buffered: buffer.len(),
            dropped: self.inner.dropped.load(Ordering::Relaxed),
        }
    }

    pub async fn status(&self) -> ActivityStatus {
        let (running, last_error, last_event_ts_ms, last_ping_ts_ms) = {
            let state = self.inner.state.lock().await;
            (
                state.running,
                state.last_error.clone(),
                state.last_event_ts_ms,
                state.last_ping_ts_ms,
            )
        };
        let buffered = self.inner.buffer.lock().await.len();
        let subscribed = self.inner.subscriptions.lock().await.contains(ACTIVITY_URI);
        ActivityStatus {
            running,
            last_seq: self.inner.last_seq.load(Ordering::Relaxed),
            buffered,
            dropped: self.inner.dropped.load(Ordering::Relaxed),
            last_error,
            last_event_ts_ms,
            last_ping_ts_ms,
            subscribed,
        }
    }

    pub async fn subscribe(&self, uri: &str) -> Result<()> {
        ensure_activity_uri(uri)?;
        self.inner.subscriptions.lock().await.insert(uri.to_owned());
        Ok(())
    }

    pub async fn unsubscribe(&self, uri: &str) -> Result<()> {
        ensure_activity_uri(uri)?;
        self.inner.subscriptions.lock().await.remove(uri);
        Ok(())
    }

    pub async fn snapshot(&self) -> Value {
        let status = self.status().await;
        let latest_event = self.inner.buffer.lock().await.back().cloned();
        json!({
            "latest_event": latest_event,
            "last_seq": status.last_seq,
            "buffered": status.buffered,
            "dropped": status.dropped,
            "running": status.running,
            "last_error": status.last_error,
            "last_event_ts_ms": status.last_event_ts_ms,
            "last_ping_ts_ms": status.last_ping_ts_ms
        })
    }

    pub async fn push_event_for_test(&self, event: Value) -> ActivityItem {
        self.push_event(event).await
    }

    pub fn manager_tasks_started_for_test(&self) -> u64 {
        self.inner.manager_tasks_started.load(Ordering::Relaxed)
    }

    async fn is_running(&self) -> bool {
        self.inner.state.lock().await.running
    }

    async fn run_stream_manager(&self, mut shutdown: watch::Receiver<bool>) {
        let mut backoff = INITIAL_BACKOFF;

        loop {
            if *shutdown.borrow() {
                break;
            }

            let mut client = match self.inner.factory.connect().await {
                Ok(client) => client,
                Err(err) => {
                    self.record_error(format!("failed to connect activity client: {err}"))
                        .await;
                    if sleep_or_shutdown(backoff, &mut shutdown).await {
                        break;
                    }
                    backoff = next_backoff(backoff);
                    continue;
                }
            };

            let mut stream = match client.open_user_activity().await {
                Ok(stream) => {
                    backoff = INITIAL_BACKOFF;
                    stream
                }
                Err(err) => {
                    self.record_error(format!("failed to open user_activity stream: {err}"))
                        .await;
                    if sleep_or_shutdown(backoff, &mut shutdown).await {
                        break;
                    }
                    backoff = next_backoff(backoff);
                    continue;
                }
            };

            let mut ping = tokio::time::interval(self.inner.ping_interval);
            ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

            loop {
                tokio::select! {
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() {
                            self.mark_stopped().await;
                            return;
                        }
                    }
                    _ = ping.tick() => {
                        match client.user_ping().await {
                            Ok(()) => self.record_ping().await,
                            Err(err) => {
                                self.record_error(format!("user_ping failed: {err}")).await;
                                break;
                            }
                        }
                    }
                    item = stream.next() => {
                        match item {
                            Some(Ok(event)) => {
                                self.push_event(event).await;
                                backoff = INITIAL_BACKOFF;
                            }
                            Some(Err(err)) => {
                                self.record_error(format!("user_activity stream error: {err}")).await;
                                break;
                            }
                            None => {
                                self.record_error("user_activity stream closed".to_owned()).await;
                                break;
                            }
                        }
                    }
                }
            }

            if sleep_or_shutdown(backoff, &mut shutdown).await {
                break;
            }
            backoff = next_backoff(backoff);
        }

        self.mark_stopped().await;
    }

    async fn push_event(&self, event: Value) -> ActivityItem {
        let seq = self.inner.next_seq.fetch_add(1, Ordering::Relaxed) + 1;
        let item = ActivityItem {
            seq,
            ts_ms: now_ms(),
            event,
        };

        {
            let mut buffer = self.inner.buffer.lock().await;
            if buffer.len() >= self.inner.max_buffered {
                buffer.pop_front();
                self.inner.dropped.fetch_add(1, Ordering::Relaxed);
            }
            buffer.push_back(item.clone());
        }
        self.inner.last_seq.store(seq, Ordering::Relaxed);
        {
            let mut state = self.inner.state.lock().await;
            state.last_event_ts_ms = Some(item.ts_ms);
        }
        self.inner.event_notify.notify_waiters();
        self.notify_resource_updated_if_subscribed().await;
        item
    }

    async fn notify_resource_updated_if_subscribed(&self) {
        let subscribed = self.inner.subscriptions.lock().await.contains(ACTIVITY_URI);
        if subscribed {
            let _ = self.inner.notifications.send(json!({
                "jsonrpc": "2.0",
                "method": "notifications/resources/updated",
                "params": { "uri": ACTIVITY_URI }
            }));
        }
    }

    async fn record_ping(&self) {
        let mut state = self.inner.state.lock().await;
        state.last_ping_ts_ms = Some(now_ms());
    }

    async fn record_error(&self, error: String) {
        tracing::warn!(%error, "Aura user_activity stream error");
        let mut state = self.inner.state.lock().await;
        state.last_error = Some(error);
    }

    async fn mark_stopped(&self) {
        let mut state = self.inner.state.lock().await;
        state.running = false;
        state.shutdown = None;
        state.handle = None;
    }
}

fn ensure_activity_uri(uri: &str) -> Result<()> {
    anyhow::ensure!(uri == ACTIVITY_URI, "unknown subscribable resource URI");
    Ok(())
}

fn env_usize(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_u64(name: &str, default: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn next_backoff(current: Duration) -> Duration {
    (current * 2).min(MAX_BACKOFF)
}

async fn sleep_or_shutdown(duration: Duration, shutdown: &mut watch::Receiver<bool>) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(duration) => *shutdown.borrow(),
        changed = shutdown.changed() => changed.is_err() || *shutdown.borrow(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[derive(Clone)]
    struct MockFactory {
        shared: Arc<MockShared>,
    }

    struct MockShared {
        connect_count: AtomicUsize,
        open_count: AtomicUsize,
        ping_count: AtomicUsize,
        plans: Mutex<VecDeque<MockPlan>>,
    }

    enum MockPlan {
        Pending,
        Items(Vec<std::result::Result<Value, String>>),
    }

    struct MockClient {
        shared: Arc<MockShared>,
    }

    impl MockFactory {
        fn new(plans: Vec<MockPlan>) -> Self {
            Self {
                shared: Arc::new(MockShared {
                    connect_count: AtomicUsize::new(0),
                    open_count: AtomicUsize::new(0),
                    ping_count: AtomicUsize::new(0),
                    plans: Mutex::new(plans.into()),
                }),
            }
        }

        fn open_count(&self) -> usize {
            self.shared.open_count.load(Ordering::Relaxed)
        }

        fn ping_count(&self) -> usize {
            self.shared.ping_count.load(Ordering::Relaxed)
        }
    }

    #[async_trait]
    impl ActivityClientFactory for MockFactory {
        type Client = MockClient;

        async fn connect(&self) -> Result<Self::Client> {
            self.shared.connect_count.fetch_add(1, Ordering::Relaxed);
            Ok(MockClient {
                shared: Arc::clone(&self.shared),
            })
        }
    }

    #[async_trait]
    impl ActivityClient for MockClient {
        async fn open_user_activity(&mut self) -> Result<ActivityStream> {
            self.shared.open_count.fetch_add(1, Ordering::Relaxed);
            let plan = self
                .shared
                .plans
                .lock()
                .await
                .pop_front()
                .unwrap_or(MockPlan::Pending);
            Ok(match plan {
                MockPlan::Pending => Box::pin(tokio_stream::pending()),
                MockPlan::Items(items) => Box::pin(tokio_stream::iter(items)),
            })
        }

        async fn user_ping(&mut self) -> Result<()> {
            self.shared.ping_count.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    fn manager(
        factory: MockFactory,
        max_buffered: usize,
    ) -> (ActivityManager<MockFactory>, mpsc::UnboundedReceiver<Value>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            ActivityManager::with_factory(factory, tx, max_buffered, Duration::from_millis(20)),
            rx,
        )
    }

    async fn wait_until(mut predicate: impl FnMut() -> bool) {
        for _ in 0..100 {
            if predicate() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(predicate());
    }

    #[tokio::test]
    async fn starting_twice_does_not_create_two_streams() {
        let factory = MockFactory::new(vec![MockPlan::Pending]);
        let (manager, _rx) = manager(factory.clone(), 8);
        let first = manager.start().await;
        assert!(first.running);
        wait_until(|| factory.open_count() == 1).await;

        let second = manager.start().await;
        assert!(second.already_running);
        assert_eq!(manager.manager_tasks_started_for_test(), 1);
        assert_eq!(factory.open_count(), 1);
        manager.stop().await;
    }

    #[tokio::test]
    async fn read_lazily_starts_stream() {
        let factory = MockFactory::new(vec![MockPlan::Pending]);
        let (manager, _rx) = manager(factory, 8);
        let response = manager
            .read(ReadActivityArgs {
                after_seq: 0,
                limit: 100,
            })
            .await;
        assert!(response.running);
        assert_eq!(manager.manager_tasks_started_for_test(), 1);
        manager.stop().await;
    }

    #[tokio::test]
    async fn read_returns_events_after_cursor() {
        let factory = MockFactory::new(vec![]);
        let (manager, _rx) = manager(factory, 8);
        manager.push_event_for_test(json!({"n": 1})).await;
        manager.push_event_for_test(json!({"n": 2})).await;
        manager.push_event_for_test(json!({"n": 3})).await;

        let response = manager
            .read(ReadActivityArgs {
                after_seq: 1,
                limit: 100,
            })
            .await;
        assert_eq!(response.events.len(), 2);
        assert_eq!(response.events[0].seq, 2);
        assert_eq!(response.events[1].seq, 3);
        manager.stop().await;
    }

    #[tokio::test]
    async fn buffer_overflow_drops_oldest() {
        let factory = MockFactory::new(vec![]);
        let (manager, _rx) = manager(factory, 2);
        manager.push_event_for_test(json!({"n": 1})).await;
        manager.push_event_for_test(json!({"n": 2})).await;
        manager.push_event_for_test(json!({"n": 3})).await;

        let status = manager.status().await;
        assert_eq!(status.buffered, 2);
        assert_eq!(status.dropped, 1);
        let response = manager
            .read(ReadActivityArgs {
                after_seq: 0,
                limit: 100,
            })
            .await;
        assert_eq!(response.events[0].seq, 2);
        assert_eq!(response.events[1].seq, 3);
        manager.stop().await;
    }

    #[tokio::test]
    async fn stop_marks_stream_not_running() {
        let factory = MockFactory::new(vec![MockPlan::Pending]);
        let (manager, _rx) = manager(factory, 8);
        manager.start().await;
        manager.stop().await;
        assert!(!manager.status().await.running);
    }

    #[tokio::test]
    async fn ping_loop_runs_while_stream_active() {
        let factory = MockFactory::new(vec![MockPlan::Pending]);
        let (manager, _rx) = manager(factory.clone(), 8);
        manager.start().await;
        wait_until(|| factory.ping_count() >= 2).await;
        assert!(manager.status().await.last_ping_ts_ms.is_some());
        manager.stop().await;
    }

    #[tokio::test]
    async fn stream_error_records_error_and_reconnects_in_same_task() {
        let factory = MockFactory::new(vec![
            MockPlan::Items(vec![Err("boom".to_owned())]),
            MockPlan::Pending,
        ]);
        let (manager, _rx) = manager(factory.clone(), 8);
        manager.start().await;
        wait_until(|| factory.open_count() >= 2).await;

        let status = manager.status().await;
        assert!(status.last_error.unwrap_or_default().contains("boom"));
        assert_eq!(manager.manager_tasks_started_for_test(), 1);
        manager.stop().await;
    }

    #[tokio::test]
    async fn subscribe_and_unsubscribe_activity_resource() {
        let factory = MockFactory::new(vec![]);
        let (manager, mut rx) = manager(factory, 8);

        manager.subscribe(ACTIVITY_URI).await.unwrap();
        assert!(manager.status().await.subscribed);
        manager.push_event_for_test(json!({"event": 1})).await;
        let notification = rx.try_recv().unwrap();
        assert_eq!(notification["method"], "notifications/resources/updated");
        assert_eq!(notification["params"]["uri"], ACTIVITY_URI);

        manager.unsubscribe(ACTIVITY_URI).await.unwrap();
        assert!(!manager.status().await.subscribed);
        manager.push_event_for_test(json!({"event": 2})).await;
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn snapshot_contains_latest_activity_event() {
        let factory = MockFactory::new(vec![]);
        let (manager, _rx) = manager(factory, 8);
        manager.push_event_for_test(json!({"latest": true})).await;
        let snapshot = manager.snapshot().await;
        assert_eq!(snapshot["latest_event"]["seq"], 1);
        assert_eq!(snapshot["last_seq"], 1);
        assert_eq!(snapshot["buffered"], 1);
    }
}
