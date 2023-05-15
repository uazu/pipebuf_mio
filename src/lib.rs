//! [`PipeBuf`] connectors for [`mio`] types
//!
//! This permits I/O between pipe-buffers and streaming-based
//! interfaces.
//!
//! [`PipeBuf`]: https://crates.io/crates/pipebuf
//! [`mio`]: https://crates.io/crates/mio

macro_rules! retry {
    ($call:expr) => {{
        loop {
            match $call {
                Err(ref e) if e.kind() == ErrorKind::Interrupted => continue,
                rv => break rv,
            }
        }
    }};
}

mod tcpstream;
pub use tcpstream::TcpLink;

mod unixstream;
pub use unixstream::UnixStreamLink;
