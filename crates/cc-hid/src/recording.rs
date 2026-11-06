//! Transport wrapper that records every exchange.
//!
//! [`RecordingTransport`] forwards to an inner transport and logs each call as a hex line via
//! `tracing::debug!` (target `cc_hid::record`) and into an in-memory list of [`Record`]s.
//! The list is shared through a [`RecordLog`] handle, so the caller can drain it while the
//! wrapper itself is owned by a driver. [`Record::to_exchange`] turns a transcript from a real
//! device into a [`FakeTransport`](crate::FakeTransport) script.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use crate::fake::Exchange;
use crate::transport::{HidError, HidTransport, hex};

/// Which call a [`Record`] describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecordKind {
    /// `write`; data is the outgoing buffer.
    Write,
    /// `read_timeout`; data is the bytes received.
    Read,
    /// `send_feature_report`; data is the outgoing buffer.
    FeatureSend,
    /// `get_feature_report`; data is the returned report including the id byte.
    FeatureGet,
}

impl RecordKind {
    fn label(self) -> &'static str {
        match self {
            Self::Write => "write",
            Self::Read => "read",
            Self::FeatureSend => "feature-send",
            Self::FeatureGet => "feature-get",
        }
    }
}

/// One recorded exchange.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// The call.
    pub kind: RecordKind,
    /// Bytes sent, or bytes received on success. For a failed get/read this is the request
    /// (`[report_id]` for `FeatureGet`, empty for `Read`).
    pub data: Vec<u8>,
    /// The error the call returned, if any.
    pub error: Option<HidError>,
}

impl Record {
    /// The fake-script step that replays this exchange, if it has one.
    ///
    /// Failed writes and failed feature gets have no equivalent step and return `None`;
    /// a read that timed out becomes [`Exchange::ReadTimeout`].
    pub fn to_exchange(&self) -> Option<Exchange> {
        match (self.kind, &self.error) {
            (RecordKind::Write, None) => Some(Exchange::ExpectWrite(self.data.clone())),
            (RecordKind::FeatureSend, None) => Some(Exchange::ExpectFeature(self.data.clone())),
            (RecordKind::Read, None) => Some(Exchange::Read(self.data.clone())),
            (RecordKind::Read, Some(HidError::Timeout)) => Some(Exchange::ReadTimeout),
            (RecordKind::FeatureGet, None) => {
                let (&report_id, rest) = self.data.split_first()?;
                Some(Exchange::FeatureReply {
                    report_id,
                    data: rest.to_vec(),
                })
            }
            _ => None,
        }
    }
}

impl fmt::Display for Record {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:<12} [{}]", self.kind.label(), hex(&self.data))?;
        if let Some(e) = &self.error {
            write!(f, " error: {e}")?;
        }
        Ok(())
    }
}

/// Shared handle to a recording's records.
#[derive(Debug, Clone, Default)]
pub struct RecordLog {
    inner: Arc<Mutex<Vec<Record>>>,
}

impl RecordLog {
    fn lock(&self) -> MutexGuard<'_, Vec<Record>> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn push(&self, record: Record) {
        tracing::debug!(target: "cc_hid::record", "{record}");
        self.lock().push(record);
    }

    /// Take all records collected so far, leaving the log empty.
    pub fn drain(&self) -> Vec<Record> {
        std::mem::take(&mut *self.lock())
    }

    /// Copy of all records collected so far.
    pub fn snapshot(&self) -> Vec<Record> {
        self.lock().clone()
    }

    /// Number of records collected so far.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether no records have been collected.
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }
}

/// Wraps a transport and records every exchange. See the module docs.
#[derive(Debug)]
pub struct RecordingTransport<T: HidTransport> {
    inner: T,
    log: RecordLog,
}

impl<T: HidTransport> RecordingTransport<T> {
    /// Wrap `inner` with a fresh log.
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            log: RecordLog::default(),
        }
    }

    /// A handle to the log that stays valid after the wrapper is moved into a driver.
    pub fn log(&self) -> RecordLog {
        self.log.clone()
    }

    /// Take all records collected so far.
    pub fn drain(&self) -> Vec<Record> {
        self.log.drain()
    }

    /// Unwrap the inner transport.
    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: HidTransport> HidTransport for RecordingTransport<T> {
    fn write(&mut self, data: &[u8]) -> Result<usize, HidError> {
        let r = self.inner.write(data);
        self.log.push(Record {
            kind: RecordKind::Write,
            data: data.to_vec(),
            error: r.as_ref().err().cloned(),
        });
        r
    }

    fn read_timeout(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, HidError> {
        let r = self.inner.read_timeout(buf, timeout);
        let (data, error) = match &r {
            Ok(n) => (buf[..(*n).min(buf.len())].to_vec(), None),
            Err(e) => (Vec::new(), Some(e.clone())),
        };
        self.log.push(Record {
            kind: RecordKind::Read,
            data,
            error,
        });
        r
    }

    fn send_feature_report(&mut self, data: &[u8]) -> Result<(), HidError> {
        let r = self.inner.send_feature_report(data);
        self.log.push(Record {
            kind: RecordKind::FeatureSend,
            data: data.to_vec(),
            error: r.as_ref().err().cloned(),
        });
        r
    }

    fn get_feature_report(&mut self, buf: &mut [u8]) -> Result<usize, HidError> {
        let requested: Vec<u8> = buf.first().copied().into_iter().collect();
        let r = self.inner.get_feature_report(buf);
        let (data, error) = match &r {
            Ok(n) => (buf[..(*n).min(buf.len())].to_vec(), None),
            Err(e) => (requested, Some(e.clone())),
        };
        self.log.push(Record {
            kind: RecordKind::FeatureGet,
            data,
            error,
        });
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeTransport;

    #[test]
    fn records_writes_and_feature_reports_in_order() {
        let fake = FakeTransport::builder()
            .write([0x01, 0x02])
            .feature([0x52, 0x09])
            .feature_reply(0x52, [0xAB])
            .read([0x05, 0x06])
            .read_timeout()
            .write([0x03])
            .build();
        let mut rec = RecordingTransport::new(fake.clone());
        let log = rec.log();

        rec.write(&[0x01, 0x02]).expect("write");
        rec.send_feature_report(&[0x52, 0x09]).expect("feature");
        let mut fb = [0x52u8, 0, 0];
        assert_eq!(rec.get_feature_report(&mut fb), Ok(2));
        let mut buf = [0u8; 4];
        assert_eq!(rec.read_timeout(&mut buf, Duration::from_millis(1)), Ok(2));
        assert_eq!(
            rec.read_timeout(&mut buf, Duration::from_millis(1)),
            Err(HidError::Timeout)
        );
        rec.write(&[0x03]).expect("write");

        let records = log.drain();
        let kinds: Vec<_> = records.iter().map(|r| r.kind).collect();
        assert_eq!(
            kinds,
            vec![
                RecordKind::Write,
                RecordKind::FeatureSend,
                RecordKind::FeatureGet,
                RecordKind::Read,
                RecordKind::Read,
                RecordKind::Write
            ]
        );
        assert_eq!(records[0].data, vec![0x01, 0x02]);
        assert_eq!(records[1].data, vec![0x52, 0x09]);
        assert_eq!(records[2].data, vec![0x52, 0xAB]);
        assert_eq!(records[3].data, vec![0x05, 0x06]);
        assert_eq!(records[4].error, Some(HidError::Timeout));
        assert_eq!(records[5].data, vec![0x03]);
        assert!(log.is_empty(), "drain empties the log");
        assert_eq!(records[1].to_string(), "feature-send [52 09]");

        // The transcript replays as a fake script.
        let script: Vec<Exchange> = records.iter().filter_map(Record::to_exchange).collect();
        assert_eq!(script.len(), 6);
        let mut replay = RecordingTransport::new(FakeTransport::new(script));
        replay.write(&[0x01, 0x02]).expect("replayed write");
        fake.finish().expect("original script consumed");
    }

    #[test]
    fn records_errors() {
        let mut rec = RecordingTransport::new(FakeTransport::builder().write([1]).build());
        assert!(rec.write(&[2]).is_err());
        let r = rec.drain();
        assert_eq!(r.len(), 1);
        assert!(matches!(r[0].error, Some(HidError::Script(_))));
        assert_eq!(r[0].to_exchange(), None);
    }
}
