//! Scripted in-memory transport for tests.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::Notify;

use super::{Connector, Transport, TransportError};

/// Replays server lines and records what the client sent.
///
/// Each script step is either a server line to deliver, or a wait for the
/// client to send a line starting with a given prefix. Waiting keeps the
/// conversation in lock-step, like a real server.
///
/// By default, reading while the script expects a send panics: with a
/// single task, that's a deadlock. [`MockConnector::concurrent`] instead
/// waits, for clients that read and send from different tasks.
#[derive(Clone)]
pub(crate) struct MockConnector {
    script: Arc<Mutex<VecDeque<Step>>>,
    pub sent: Arc<Mutex<Vec<String>>>,
    progress: Arc<Notify>,
    wait_for_sends: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum Step {
    Server(String),
    Expect(String),
}

impl MockConnector {
    pub fn new(script: impl IntoIterator<Item = Step>) -> Self {
        MockConnector {
            script: Arc::new(Mutex::new(script.into_iter().collect())),
            sent: Arc::default(),
            progress: Arc::default(),
            wait_for_sends: false,
        }
    }

    pub fn concurrent(script: impl IntoIterator<Item = Step>) -> Self {
        MockConnector {
            wait_for_sends: true,
            ..Self::new(script)
        }
    }

    pub fn sent(&self) -> Vec<String> {
        self.sent.lock().unwrap().clone()
    }
}

pub(crate) fn server(line: &str) -> Step {
    Step::Server(line.to_owned())
}

pub(crate) fn expect(prefix: &str) -> Step {
    Step::Expect(prefix.to_owned())
}

#[async_trait]
impl Connector for MockConnector {
    async fn connect(&self) -> Result<Box<dyn Transport>, TransportError> {
        Ok(Box::new(MockTransport {
            script: self.script.clone(),
            sent: self.sent.clone(),
            progress: self.progress.clone(),
            wait_for_sends: self.wait_for_sends,
        }))
    }
}

struct MockTransport {
    script: Arc<Mutex<VecDeque<Step>>>,
    sent: Arc<Mutex<Vec<String>>>,
    progress: Arc<Notify>,
    wait_for_sends: bool,
}

#[async_trait]
impl Transport for MockTransport {
    async fn send(&mut self, line: &str) -> Result<(), TransportError> {
        assert!(!line.contains(['\r', '\n']));
        self.sent.lock().unwrap().push(line.to_owned());
        let mut script = self.script.lock().unwrap();
        if let Some(Step::Expect(prefix)) = script.front()
            && line.starts_with(prefix.as_str())
        {
            script.pop_front();
            self.progress.notify_waiters();
        }
        Ok(())
    }

    async fn recv(&mut self) -> Result<Option<String>, TransportError> {
        loop {
            // Registered before checking, so a send in between isn't missed.
            let progressed = self.progress.notified();
            {
                let mut script = self.script.lock().unwrap();
                match script.front() {
                    Some(Step::Server(_)) => match script.pop_front() {
                        Some(Step::Server(line)) => return Ok(Some(line)),
                        _ => unreachable!(),
                    },
                    Some(Step::Expect(prefix)) if !self.wait_for_sends => panic!(
                        "client is waiting for input but script expects it to send {prefix:?}; sent so far: {:#?}",
                        self.sent.lock().unwrap()
                    ),
                    Some(Step::Expect(_)) => {}
                    None => return Ok(None),
                }
            }
            progressed.await;
        }
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        Ok(())
    }
}
