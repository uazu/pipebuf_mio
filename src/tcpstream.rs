use mio::net::TcpStream;
use pipebuf::PBufRdWr;
use std::io::{ErrorKind, Result};

/// Exchange stream data via the `mio` [`TcpStream`] type
///
/// For the TCP incoming stream both TCP "close" (FIN) and "abort"
/// (RST) are detected and passed on.  For the TCP outgoing stream,
/// "close" handling is mapped to a normal shutdown on the outgoing
/// half of the TCP stream.  The incoming half of the TCP stream, if
/// still open, remains open until the other end closes, as expected.
///
/// For TCP outgoing "abort", ideally we'd generate a TCP RST to tear
/// things down at both ends as soon as possible.  This can be done
/// with `set_linger(Some(0))` and a close.  However the linger API is
/// not yet stable on `std`, is not present at all in `mio`.  So on
/// "abort", this code does a normal shutdown on both incoming and
/// outgoing TCP streams, and does an "abort" on the side of the pipe
/// for incoming TCP data.  This should cause rapid shutdown of things
/// locally.  The remote end however will not know that this is an
/// abort.  Linger-based handling of outgoing "abort" may be added
/// later as a runtime option once it is stable in the APIs.
///
/// To start with both reading and writing via the TCP stream are
/// paused.  This is because, depending on the platform, reading or
/// writing may give an error if a "ready" indication has not yet been
/// received.  So call `set_pause_writes(false)` or
/// `set_pause_reads(false)` as soon as the stream indicates "ready"
/// in order to allow data to flow.
pub struct TcpLink {
    // Maximum amount of data to read in one go (in bytes)
    max_read_unit: usize,

    // TCP_NODELAY flag
    nodelay: bool,

    // Set to pause writes (waiting for first "ready" indication)
    pause_writes: bool,

    // Set to pause reads (waiting for first "ready" indication)
    pause_reads: bool,

    // Pending set_nodelay()
    pending_set_nodelay: bool,
}

impl TcpLink {
    /// Create the component with default settings:
    ///
    /// - **max_read_unit** of 2048.  This is bigger than a typical IP
    /// packet's data load but you may want to increase this.
    ///
    /// - **nodelay** set to `false`, i.e. using the Nagle algorithm to
    /// delay output to attempt to batch up data into fewer IP packets
    ///
    /// - Both reads and writes paused
    #[inline]
    pub fn new() -> Self {
        Self {
            max_read_unit: 2048,
            nodelay: false,
            pause_writes: true,
            pause_reads: true,
            pending_set_nodelay: false,
        }
    }

    /// Change the maximum number of bytes to read in each `process`
    /// call.  This allows managing how much data you wish to handle
    /// at a time, to allow the possibility of backpressure, and to
    /// control how large the pipe buffers in your processing chain
    /// will grow.  If memory is not an issue, then there is no
    /// problem with setting this large, which will likely give higher
    /// efficiency if there is a lot of data queued.
    #[inline]
    pub fn set_max_read_unit(&mut self, max_read_unit: usize) {
        self.max_read_unit = max_read_unit;
    }

    /// Change the "no delay" flag on the stream.  This will be
    /// updated on the next `process` call.
    ///
    /// Use `true` if you wish to disable the Nagle algorithm, e.g. if
    /// you are sending large chunks or packets of data and wish them
    /// to be sent immediately, even if that means using a part-filled
    /// IP packet for the last part of the data.
    ///
    /// Use `false` if you are sending byte data a few bytes at a time
    /// (e.g. terminal interaction) and you'd rather that TCP waited a
    /// moment to attempt to batch up data together to reduce the
    /// number of IP packets sent.  For the last part of the data it
    /// may add a delay of a network round-trip.
    #[inline]
    pub fn set_nodelay(&mut self, nodelay: bool) {
        if self.nodelay != nodelay {
            self.nodelay = nodelay;
            self.pending_set_nodelay = true
        }
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
    /// TCP stream.  Returns the activity status: `Ok(true)` if
    /// something changed, `Ok(false)` if no progress could be made,
    /// or `Err(_)` if there was a fatal error on the stream.
    ///
    /// Assumes that it is always called with the same TcpStream and
    /// pipe-buffer.  Things will behave unpredictably otherwise.
    pub fn process(&mut self, stream: &mut TcpStream, mut pbuf: PBufRdWr) -> Result<bool> {
        let rd_activity = self.process_out(stream, pbuf.reborrow())?;
        let wr_activity = self.process_in(stream, pbuf.reborrow())?;
        Ok(rd_activity || wr_activity)
    }

    /// Write as much data as possible out to the given TCP stream.
    /// Returns the activity status: `Ok(true)` if something changed,
    /// `Ok(false)` if no progress could be made, or `Err(_)` if there
    /// was a fatal error on the stream.
    ///
    /// Assumes that it is always called with the same TcpStream and
    /// pipe-buffer.  Things will behave unpredictably otherwise.
    pub fn process_out(&mut self, stream: &mut TcpStream, mut pbuf: PBufRdWr) -> Result<bool> {
        if self.pause_writes {
            return Ok(false);
        }

        if self.pending_set_nodelay {
            self.pending_set_nodelay = false;
            retry!(stream.set_nodelay(self.nodelay))?;
        }

        // TcpStream::flush() does nothing as it does write() syscalls
        // directly (which don't buffer).  So there is no need to give
        // the option to force flushes.
        let mut prd = pbuf.rd;
        let trip = prd.tripwire();
        match prd.output_to(stream, false) {
            Err(ref e) if e.kind() == ErrorKind::WouldBlock => (),
            Err(e) => return Err(e),
            Ok(_) => {
                if prd.is_empty() && prd.has_pending_eof() {
                    let shutdown = if prd.is_aborted() {
                        pbuf.wr.abort();
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

    /// Read as much data as possible from to the given TCP stream, up
    /// to **max_read_unit** bytes.  Returns the activity status:
    /// `Ok(true)` if something changed, `Ok(false)` if no progress
    /// could be made, or `Err(_)` if there was a fatal error on the
    /// stream.
    ///
    /// Assumes that it is always called with the same TcpStream and
    /// pipe-buffer.  Things will behave unpredictably otherwise.
    pub fn process_in(&mut self, stream: &mut TcpStream, pbuf: PBufRdWr) -> Result<bool> {
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

impl Default for TcpLink {
    fn default() -> Self {
        Self::new()
    }
}
