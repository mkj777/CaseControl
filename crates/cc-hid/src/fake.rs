//! Scripted fake transport for driver tests without hardware.
//!
//! A [`FakeTransport`] holds a script of [`Exchange`] steps that are consumed strictly in order.
//! Outgoing bytes (writes and feature reports) are compared byte-exact against the script; reads
//! and feature-report replies are replayed from it. The first mismatch returns
//! [`HidError::Script`] with a hex dump of both full buffers and marks the fake as failed, so
//! [`FakeTransport::finish`] reports it even if the driver swallowed the error.
//!
//! Clones share one script and one state: hand a clone (boxed) to the driver under test and keep
//! the original to call [`finish`](FakeTransport::finish) or [`recorded`](FakeTransport::recorded).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use crate::transport::{HidError, HidTransport, hex};

/// One step of a fake script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exchange {
    /// The driver must call `write` with exactly these bytes (including the report id).
    ExpectWrite(Vec<u8>),
    /// The next `read_timeout` returns these bytes.
    Read(Vec<u8>),
    /// The next `read_timeout` returns [`HidError::Timeout`].
    ReadTimeout,
    /// The driver must call `send_feature_report` with exactly these bytes.
    ExpectFeature(Vec<u8>),
    /// The next `get_feature_report` must ask for `report_id` (`buf[0]`) and receives
    /// `report_id` followed by `data`.
    FeatureReply {
        /// Report id the driver must request.
        report_id: u8,
        /// Payload after the report id byte.
        data: Vec<u8>,
    },
}

impl Exchange {
    fn describe(&self) -> String {
        match self {
            Self::ExpectWrite(d) => format!("write [{}]", hex(d)),
            Self::Read(d) => format!("read -> [{}]", hex(d)),
            Self::ReadTimeout => "read -> timeout".to_owned(),
            Self::ExpectFeature(d) => format!("send_feature_report [{}]", hex(d)),
            Self::FeatureReply { report_id, data } => {
                format!("get_feature_report id {report_id:#04x} -> [{}]", hex(data))
            }
        }
    }
}

/// Chainable builder for fake scripts.
#[derive(Debug, Clone, Default)]
pub struct ScriptBuilder {
    steps: Vec<Exchange>,
}

impl ScriptBuilder {
    /// An empty script.
    pub fn new() -> Self {
        Self::default()
    }

    /// Expect a `write` of exactly `data`.
    pub fn write(mut self, data: impl Into<Vec<u8>>) -> Self {
        self.steps.push(Exchange::ExpectWrite(data.into()));
        self
    }

    /// Replay `data` on the next `read_timeout`.
    pub fn read(mut self, data: impl Into<Vec<u8>>) -> Self {
        self.steps.push(Exchange::Read(data.into()));
        self
    }

    /// Time out on the next `read_timeout`.
    pub fn read_timeout(mut self) -> Self {
        self.steps.push(Exchange::ReadTimeout);
        self
    }

    /// Expect a `send_feature_report` of exactly `data`.
    pub fn feature(mut self, data: impl Into<Vec<u8>>) -> Self {
        self.steps.push(Exchange::ExpectFeature(data.into()));
        self
    }

    /// Answer the next `get_feature_report` for `report_id` with `data` (after the id byte).
    pub fn feature_reply(mut self, report_id: u8, data: impl Into<Vec<u8>>) -> Self {
        self.steps.push(Exchange::FeatureReply {
            report_id,
            data: data.into(),
        });
        self
    }

    /// Append an arbitrary step.
    pub fn step(mut self, step: Exchange) -> Self {
        self.steps.push(step);
        self
    }

    /// Append several steps (e.g. from a recorded transcript).
    pub fn steps(mut self, steps: impl IntoIterator<Item = Exchange>) -> Self {
        self.steps.extend(steps);
        self
    }

    /// The steps built so far.
    pub fn into_steps(self) -> Vec<Exchange> {
        self.steps
    }

    /// Build the fake transport.
    pub fn build(self) -> FakeTransport {
        FakeTransport::new(self.steps)
    }
}

#[derive(Debug, Default)]
struct State {
    script: VecDeque<Exchange>,
    recorded: Vec<Vec<u8>>,
    failure: Option<String>,
    position: usize,
}

/// Scripted [`HidTransport`] for tests. See the module docs.
#[derive(Debug, Clone)]
pub struct FakeTransport {
    state: Arc<Mutex<State>>,
}

impl FakeTransport {
    /// A fake that will play `script` in order.
    pub fn new(script: impl IntoIterator<Item = Exchange>) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                script: script.into_iter().collect(),
                ..State::default()
            })),
        }
    }

    /// Start a [`ScriptBuilder`].
    pub fn builder() -> ScriptBuilder {
        ScriptBuilder::new()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // A poisoned lock means a test already panicked; keep going with the data.
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Every outgoing buffer so far (writes and sent feature reports, in call order),
    /// including ones that did not match the script.
    pub fn recorded(&self) -> Vec<Vec<u8>> {
        self.lock().recorded.clone()
    }

    /// Number of script steps not consumed yet.
    pub fn remaining(&self) -> usize {
        self.lock().script.len()
    }

    /// Whether a mismatch has occurred.
    pub fn failed(&self) -> bool {
        self.lock().failure.is_some()
    }

    /// Succeeds only if no mismatch occurred and the whole script was consumed.
    pub fn finish(self) -> Result<(), HidError> {
        let st = self.lock();
        if let Some(f) = &st.failure {
            return Err(HidError::Script(f.clone()));
        }
        if !st.script.is_empty() {
            let left: Vec<String> = st.script.iter().map(Exchange::describe).collect();
            return Err(HidError::Script(format!(
                "{} unconsumed step(s) after step {}: {}",
                left.len(),
                st.position,
                left.join("; ")
            )));
        }
        Ok(())
    }

    /// Pop the next step for the call described by `actual`, failing on a finished/failed script.
    fn next(st: &mut State, actual: &str) -> Result<Exchange, HidError> {
        if let Some(f) = &st.failure {
            return Err(HidError::Script(format!("fake already failed: {f}")));
        }
        match st.script.pop_front() {
            Some(step) => {
                st.position += 1;
                Ok(step)
            }
            None => Err(Self::fail(
                st,
                format!("step {}: script exhausted, got {actual}", st.position + 1),
            )),
        }
    }

    fn fail(st: &mut State, msg: String) -> HidError {
        tracing::debug!(target: "cc_hid::fake", "{msg}");
        st.failure = Some(msg.clone());
        HidError::Script(msg)
    }

    fn mismatch(st: &mut State, expected: &Exchange, actual: String) -> HidError {
        let msg = format!(
            "step {}: expected {}, got {actual}",
            st.position,
            expected.describe()
        );
        Self::fail(st, msg)
    }
}

impl HidTransport for FakeTransport {
    fn write(&mut self, data: &[u8]) -> Result<usize, HidError> {
        let mut st = self.lock();
        st.recorded.push(data.to_vec());
        let actual = format!("write [{}]", hex(data));
        match Self::next(&mut st, &actual)? {
            Exchange::ExpectWrite(exp) if exp == data => Ok(data.len()),
            other => Err(Self::mismatch(&mut st, &other, actual)),
        }
    }

    fn read_timeout(&mut self, buf: &mut [u8], _timeout: Duration) -> Result<usize, HidError> {
        let mut st = self.lock();
        let actual = format!("read (buffer {} bytes)", buf.len());
        match Self::next(&mut st, &actual)? {
            Exchange::Read(data) => {
                if data.len() > buf.len() {
                    let msg = format!(
                        "step {}: read buffer of {} bytes too small for scripted [{}]",
                        st.position,
                        buf.len(),
                        hex(&data)
                    );
                    Self::fail(&mut st, msg);
                    return Err(HidError::InvalidReport {
                        expected: data.len(),
                        got: buf.len(),
                    });
                }
                buf[..data.len()].copy_from_slice(&data);
                Ok(data.len())
            }
            Exchange::ReadTimeout => Err(HidError::Timeout),
            other => Err(Self::mismatch(&mut st, &other, actual)),
        }
    }

    fn send_feature_report(&mut self, data: &[u8]) -> Result<(), HidError> {
        let mut st = self.lock();
        st.recorded.push(data.to_vec());
        let actual = format!("send_feature_report [{}]", hex(data));
        match Self::next(&mut st, &actual)? {
            Exchange::ExpectFeature(exp) if exp == data => Ok(()),
            other => Err(Self::mismatch(&mut st, &other, actual)),
        }
    }

    fn get_feature_report(&mut self, buf: &mut [u8]) -> Result<usize, HidError> {
        let mut st = self.lock();
        let requested = buf.first().copied();
        let actual = match requested {
            Some(id) => format!("get_feature_report id {id:#04x} (buffer {} bytes)", buf.len()),
            None => "get_feature_report with empty buffer".to_owned(),
        };
        match Self::next(&mut st, &actual)? {
            Exchange::FeatureReply { report_id, data } if requested == Some(report_id) => {
                let total = data.len() + 1;
                if total > buf.len() {
                    let msg = format!(
                        "step {}: feature buffer of {} bytes too small for {total}-byte reply",
                        st.position,
                        buf.len()
                    );
                    Self::fail(&mut st, msg);
                    return Err(HidError::InvalidReport {
                        expected: total,
                        got: buf.len(),
                    });
                }
                buf[0] = report_id;
                buf[1..total].copy_from_slice(&data);
                Ok(total)
            }
            other => Err(Self::mismatch(&mut st, &other, actual)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Duration = Duration::from_millis(10);

    #[test]
    fn happy_path() {
        let fake = FakeTransport::builder()
            .write([0x01, 0x02])
            .read([0x01, 0xAA])
            .feature([0x52, 0x00, 0x01])
            .feature_reply(0x52, [0x10, 0x20])
            .read_timeout()
            .build();
        let mut t: Box<dyn HidTransport> = Box::new(fake.clone());

        assert_eq!(t.write(&[0x01, 0x02]), Ok(2));
        let mut buf = [0u8; 8];
        assert_eq!(t.read_timeout(&mut buf, T), Ok(2));
        assert_eq!(&buf[..2], &[0x01, 0xAA]);
        assert_eq!(t.send_feature_report(&[0x52, 0x00, 0x01]), Ok(()));
        let mut fb = [0u8; 4];
        fb[0] = 0x52;
        assert_eq!(t.get_feature_report(&mut fb), Ok(3));
        assert_eq!(fb, [0x52, 0x10, 0x20, 0x00]);
        assert_eq!(t.read_timeout(&mut buf, T), Err(HidError::Timeout));

        assert_eq!(fake.remaining(), 0);
        assert_eq!(fake.recorded(), vec![vec![0x01, 0x02], vec![0x52, 0x00, 0x01]]);
        fake.finish().expect("script fully consumed");
    }

    #[test]
    fn write_mismatch_reports_hex_dump_and_fails() {
        let fake = FakeTransport::builder().write([0x01, 0xFF, 0x00]).build();
        let mut t = fake.clone();
        let err = t.write(&[0x01, 0xFE, 0x00, 0x07]).expect_err("mismatch");
        match &err {
            HidError::Script(m) => {
                assert!(m.contains("01 ff 00"), "expected bytes in dump: {m}");
                assert!(m.contains("01 fe 00 07"), "actual bytes in dump: {m}");
            }
            other => panic!("wrong error {other:?}"),
        }
        assert!(fake.failed());
        // Further calls keep failing.
        assert!(matches!(t.write(&[0x01]), Err(HidError::Script(_))));
        assert!(matches!(fake.finish(), Err(HidError::Script(_))));
    }

    #[test]
    fn wrong_call_kind_is_a_mismatch() {
        let mut fake = FakeTransport::builder().feature([0x01]).build();
        assert!(matches!(fake.write(&[0x01]), Err(HidError::Script(_))));
        assert!(fake.failed());
    }

    #[test]
    fn leftover_steps_fail_finish() {
        let mut fake = FakeTransport::builder().write([1]).write([2]).build();
        fake.write(&[1]).expect("first step matches");
        match fake.finish() {
            Err(HidError::Script(m)) => assert!(m.contains("1 unconsumed"), "{m}"),
            other => panic!("expected leftover failure, got {other:?}"),
        }
    }

    #[test]
    fn read_timeout_yields_timeout() {
        let mut fake = FakeTransport::builder().read_timeout().build();
        let mut buf = [0u8; 4];
        assert_eq!(fake.read_timeout(&mut buf, T), Err(HidError::Timeout));
        fake.finish().expect("consumed");
    }

    #[test]
    fn exhausted_script_fails() {
        let mut fake = FakeTransport::new([]);
        assert!(matches!(fake.write(&[0]), Err(HidError::Script(_))));
        assert!(fake.failed());
    }

    #[test]
    fn feature_reply_checks_report_id_and_size() {
        let mut fake = FakeTransport::builder().feature_reply(0x52, [1, 2, 3]).build();
        let mut buf = [0x53u8, 0, 0, 0];
        assert!(matches!(
            fake.get_feature_report(&mut buf),
            Err(HidError::Script(_))
        ));

        let mut fake = FakeTransport::builder().feature_reply(0x52, [1, 2, 3]).build();
        let mut small = [0x52u8, 0];
        assert_eq!(
            fake.get_feature_report(&mut small),
            Err(HidError::InvalidReport { expected: 4, got: 2 })
        );
        assert!(fake.failed());
    }

    #[test]
    fn short_read_buffer_is_invalid_report() {
        let mut fake = FakeTransport::builder().read([1, 2, 3]).build();
        let mut buf = [0u8; 2];
        assert_eq!(
            fake.read_timeout(&mut buf, T),
            Err(HidError::InvalidReport { expected: 3, got: 2 })
        );
    }
}
