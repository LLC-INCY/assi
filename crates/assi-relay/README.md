# assi-relay

Self-hosted TCP relay with two outgoing client connections, intended to work when
clients cannot accept inbound connections through NAT. This is a relay route, not
UDP hole punching, automatic route selection, a device-code registry or a deployed
cloud service. The server is implemented but public reachability has not been tested.

## Security boundary

The outer TLS connection authenticates the relay using the public certificate in
`RelayConfig`; rustls performs ordinary certificate and SAN verification for
`assi-relay.local`. No unsafe verifier or custom cryptography is used. Both clients
must receive the public server configuration through a trusted channel. The relay
has its own private key, kept in a separate identity file.

Pairing uses a random 96-bit room identifier and a separate random 256-bit ticket.
The ticket is transmitted only inside outer TLS, compared in constant time, and
must not be logged or stored. An incorrect ticket does not consume the room. A room
can pair exactly once; owner disconnect, expiry and shutdown remove it.

**Outer TLS alone does not hide the proxied bytes from the relay.** The returned
`RelayStream` must carry an independently authenticated end-to-end TLS connection
(e.g. assi-session's existing pinned invitation certificate/token). The relay then
sees timing, sizes and IP addresses, but not screen/file/input content. The relay can
still refuse, delay or terminate a connection. It never authorizes screen access;
owner confirmation remains in the inner session protocol.

No analytics, content persistence, room logs or tickets are emitted. The CLI prints
only a public configuration containing endpoint and certificate. Server limits
bound resource use, but are not account authentication, billing, abuse prevention
or protection from a volumetric denial-of-service attack.

## Library API

```rust,ignore
let server = Server::bind(
    "127.0.0.1:0".parse()?,
    "127.0.0.1".parse()?,
    Identity::generate()?,
    Limits::default(),
).await?;
let config = server.config();
let reservation = reserve(&config).await?;
// Pass room/ticket inside the application's confidential invitation.
let viewer = join(&config, &reservation.room, &reservation.ticket).await?;
let host = reservation.wait().await?;
// Wrap host/viewer streams in the inner application TLS now.
server.shutdown().await?;
```

`RelayConfig { endpoint: SocketAddr, certificate: String }` supports serde,
`validate()`, `encode()` / `decode()` using `assi-relay://` and base64url JSON.
The certificate field is base64url DER. Configuration decoding has an 8 KiB bound;
certificates are capped at 4 KiB. The endpoint is a literal IPv4/IPv6 address, not
an arbitrary URL or DNS name. Port zero, unspecified/multicast/broadcast addresses
are rejected. `Server::local_addr()` returns the actual bound address, whereas
`config().endpoint` contains the explicit advertised address and actual port.

`Reservation` exposes `room` and `ticket`, has no Debug implementation, and owns
the host socket. `wait(self)` returns `RelayStream`, a tokio-rustls client TLS stream
that implements AsyncRead/AsyncWrite. Dropping a reservation closes its room.

## Limits and lifetime

Defaults:

- 128 pending rooms, 256 connections including handshakes and both paired sockets.
- 300-second reservation lifetime, 8-second handshake/control operation timeout.
- 60-second idle read/write timeout in each direction, 12-hour maximum paired session.
- 2 KiB maximum control JSON, checked before allocation; 16 KiB copy buffer per direction.

`Limits` can lower/increase relevant operational limits within structural bounds;
room TTL cannot exceed 300 seconds. The inner session should send heartbeat traffic
in both directions before the idle timeout. Disconnect in either direction closes
the pair. Shutdown signals and drains/aborts connection tasks, closing pending rooms
and paired sockets. Dropping `Server` also signals shutdown; use async `shutdown()`
when you need to know cleanup is complete.

There is no unbounded stream queue. Rustls and the OS own their own bounded socket
and record buffers. A joining socket retains its semaphore permit after transfer to
the host task, so the connection cap continues to count both endpoints.

## CLI

Build/run manually on the intended server:

```sh
cargo run -p assi-relay -- --help
cargo run -p assi-relay -- --bind 127.0.0.1:4788 --identity ./assi-relay-identity.json
```

The default bind is loopback. For a public deployment the operator must explicitly
choose `--bind IP:PORT --advertise REACHABLE_IP`, configure firewall/routing and
service supervision. This implementation does not alter firewall rules or expose a
server automatically. The advertised port equals the bound port; external NAT port
translation is not currently modeled.

Identity creation uses exclusive create and Unix mode 0600. Existing identity files
must be regular files, at most 16 KiB, and mode 0600 on Unix. Corrupt files, symlinks
and insecure existing permissions are rejected without replacement. Keep the parent
directory private to the service account; the directory itself must not be writable
by untrusted users. On Windows arrange service-account ACLs yourself; Unix mode
checks are unavailable there. Do not copy the identity file to clients, source control
or diagnostics. The public configuration is safe to distribute to intended clients.

Ctrl+C closes the server. There is no auto-update, system service install, public
publication or background automation.

## Verified

`cargo test -p assi-relay`: 9 tests (1 framing unit and 8 integration/persistence).
They exercise actual loopback TLS, bidirectional bytes, wrong ticket without room
consumption, certificate rejection, expiry, socket cleanup, room/connection caps,
idle timeout and private identity persistence. `cargo clippy -p assi-relay
--all-targets -- -D warnings` passes. Tests do not capture a screen or execute input.
API signatures were checked against locally resolved official tokio-rustls 0.26.6
and rustls 0.23.46 sources; web copies for these exact versions were unavailable
through the documentation tool. End-to-end nesting is tested by assi-session's
integration tests, not by this crate's byte-pipe tests.
