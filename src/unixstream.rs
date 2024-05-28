use mio::net::UnixStream;
use pipebuf::PBufRdWr;
use std::io::{ErrorKind, Result};

/// Exchange stream data via the `mio` [`UnixStream`] type
///
/// For the incoming Unix stream both "close" and "abort" are detected
/// and passed on.  For the Unix outgoing stream, "close" handling is
/// mapped to a normal shutdown on the outgoing half of the Unix
/// stream.  The incoming half of the Unix stream, if still open,
/// remains open until the other end closes, as expected.
///
/// For outgoing "abort", this code does a normal shutdown on both
/// incoming and outgoing Unix streams, and does an "abort" on the
/// side of the pipe for incoming Unix-stream data.  This should cause
/// rapid shutdown of things locally.
///
/// To start with both reading and writing via the Unix stream are
/// paused.  So call `set_pause_writes(false)` or
/// `set_pause_reads(false)` as soon as the stream indicates "ready"
/// in order to allow data to flow.
pub struct UnixStreamLink {
    // Maximum amount of data to read in one go (in bytes)
    max_read_unit: usize,

    // Set to pause writes (waiting for first "ready" indication)
    pause_writes: bool,

    // Set to pause reads (waiting for first "ready" indication)
    pause_reads: bool,
}

impl UnixStreamLink {
    /// Create the component with default settings:
    ///
    /// - **max_read_unit** of 2048
    ///
    /// - Both reads and writes paused
    #[inline]
    pub fn new() -> Self {
        Self {
            max_read_unit: 2048,
            pause_writes: true,
            pause_reads: true,
        }
    }

    /// Change the maximum number of bytes to read in each `process`
    /// call.  This allows managing how much data you wish to handle
    /// at a time, to allow the possibility of backpressure, and to
    /// control how large the pipe buffers in your processing chain
    /// will grow.  If memory is not an issue, there is no problem
    /// with setting this large, which will likely give higher
    /// efficiency.
    #[inline]
    pub fn set_max_read_unit(&mut self, max_read_unit: usize) {
        self.max_read_unit = max_read_unit;
    }

    /// Pause or unpause writes.  This takes effect on the next
    /// `process` call.
    #[inline]
    pub fn set_pause_writes(&mut self, pause: bool) {
        self.pause_writes = pause;
    }

    /// Pause or unpause reads.  This takes effect on the next
    /// `process` call.
    #[inline]
    pub fn set_pause_reads(&mut self, pause: bool) {
        self.pause_reads = pause;
    }

    /// Read and write as much data as possible to and from the given
    /// Unix stream.  Returns the activity status: `Ok(true)` if
    /// something changed, `Ok(false)` if no progress could be made,
    /// or `Err(_)` if there was a fatal error on the stream.
    ///
    /// Assumes that it is always called with the same `UnixStream`
    /// and pipe-buffer.  Things will behave unpredictably otherwise.
    pub fn process(&mut self, stream: &mut UnixStream, mut pbuf: PBufRdWr) -> Result<bool> {
        let rd_activity = self.process_out(stream, pbuf.reborrow())?;
        let wr_activity = self.process_in(stream, pbuf.reborrow())?;
        Ok(rd_activity || wr_activity)
    }

    /// Write as much data as possible out to the given Unix stream.
    /// Returns the activity status: `Ok(true)` if something changed,
    /// `Ok(false)` if no progress could be made, or `Err(_)` if there
    /// was a fatal error on the stream.
    ///
    /// Assumes that it is always called with the same `UnixStream`
    /// and pipe-buffer.  Things will behave unpredictably otherwise.
    pub fn process_out(&mut self, stream: &mut UnixStream, mut pbuf: PBufRdWr) -> Result<bool> {
        if self.pause_writes {
            return Ok(false);
        }

        let mut prd = pbuf.rd;
        let trip = prd.tripwire();
        match prd.output_to(stream, false) {
            Err(ref e) if e.kind() == ErrorKind::WouldBlock => (),
            Err(e) => return Err(e),
            Ok(_) => {
                if prd.is_empty() && prd.has_pending_eof() {
                    let shutdown = if prd.is_aborted() {
                        if !pbuf.wr.is_eof() {
                            pbuf.wr.abort();
                        }
                        std::net::Shutdown::Both
                    } else {
                        std::net::Shutdown::Write
                    };
                    match retry!(stream.shutdown(shutdown)) {
                        Err(ref e) if e.kind() == ErrorKind::WouldBlock => (),
                        Err(e) => return Err(e),
                        Ok(_) => {
                            prd.consume_eof();
                        }
                    }
                }
            }
        }
        Ok(prd.is_tripped(trip))
    }

    /// Read as much data as possible from to the given Unix stream,
    /// up to **max_read_unit** bytes.  Returns the activity status:
    /// `Ok(true)` if something changed, `Ok(false)` if no progress
    /// could be made, or `Err(_)` if there was a fatal error on the
    /// stream.
    ///
    /// Assumes that it is always called with the same `UnixStream`
    /// and pipe-buffer.  Things will behave unpredictably otherwise.
    pub fn process_in(&mut self, stream: &mut UnixStream, pbuf: PBufRdWr) -> Result<bool> {
        let mut pwr = pbuf.wr;
        if self.pause_reads || pwr.is_eof() {
            return Ok(false);
        }

        let trip = pwr.tripwire();
        if let Err(e) = pwr.input_from(stream, self.max_read_unit) {
            match e.kind() {
                ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted => pwr.abort(),
                ErrorKind::WouldBlock => (),
                _ => return Err(e),
            }
        }
        Ok(pwr.is_tripped(trip))
    }
}

impl Default for UnixStreamLink {
    fn default() -> Self {
        Self::new()
    }
}
