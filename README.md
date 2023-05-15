# `PipeBuf` support for `mio` byte-streams

This provides types to manage transferring data between pipe-buffers
and `mio` byte-stream types such as `TcpStream` and `UnixStream`.
This would typically be combined with `PipeBuf`-based protocol
handlers (TLS, WS, HTTP, etc) to form a complete working solution.

### Documentation

See the [crate documentation](http://docs.rs/pipebuf_mio).

# License

This project is licensed under either the Apache License version 2 or
the MIT license, at your option.  (See
[LICENSE-APACHE](LICENSE-APACHE) and [LICENSE-MIT](LICENSE-MIT)).

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in this crate by you, as defined in the
Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.
