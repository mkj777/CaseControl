//! The OpenRGB SDK port probe.
//!
//! This is the only socket use in the whole project (ADR 0001 #13). It connects to the loopback address only,
//! sends nothing, and runs only when [`ScanOptions::check_openrgb_port`](crate::ScanOptions) is set.

use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

use crate::sources::OpenRgbPortProbe;

/// The OpenRGB SDK server's default port.
pub const OPENRGB_SDK_PORT: u16 = 6742;

/// How long the probe waits for the connection.
pub const PROBE_TIMEOUT: Duration = Duration::from_millis(200);

/// Real probe: a TCP connect to `127.0.0.1:6742` with a 200 ms timeout. The connection is closed immediately.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenRgbProbe;

impl OpenRgbPortProbe for OpenRgbProbe {
    fn openrgb_listening(&self) -> bool {
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, OPENRGB_SDK_PORT));
        TcpStream::connect_timeout(&addr, PROBE_TIMEOUT).is_ok()
    }
}
