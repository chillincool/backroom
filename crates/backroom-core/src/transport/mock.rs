//! Scripted in-memory transport for tests.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::{Connector, Transport, TransportError};

/// Replays server lines and records what the client sent.
///
/// Each script step is either a server line to deliver, or a wait for the
/// client to send a line starting with a given prefix. Waiting keeps the
/// conversation in lock-step, like a real server.
#[derive(Clone)]
pub(crate) struct MockConnector {
    script: Arc<Mutex<VecDeque<Step>>>,
    pub sent: Arc<Mutex<Vec<String>>>,
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
        }))
    }
}

struct MockTransport {
    script: Arc<Mutex<VecDeque<Step>>>,
    sent: Arc<Mutex<Vec<String>>>,
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
        }
        Ok(())
    }

    async fn recv(&mut self) -> Result<Option<String>, TransportError> {
        let mut script = self.script.lock().unwrap();
        match script.front() {
            Some(Step::Server(_)) => match script.pop_front() {
                Some(Step::Server(line)) => Ok(Some(line)),
                _ => unreachable!(),
            },
            Some(Step::Expect(prefix)) => panic!(
                "client is waiting for input but script expects it to send {prefix:?}; sent so far: {:#?}",
                self.sent.lock().unwrap()
            ),
            None => Ok(None),
        }
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        Ok(())
    }
}
