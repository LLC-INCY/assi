# assi-session

Direct or relayed remote-viewing, consent-controlled input and approved file transfer in Rust. Capture,
rendering, OS permissions and injection belong to the platform adapters; this crate
never captures or injects by itself.

## Public integration contract

Create an `Arc<SessionEngine>` with `SessionEngine::new()`. All async methods run
inside a Tokio runtime with I/O and timers enabled.

- `start_host(bind, advertised_ip, name)` returns a confidential `assi://…`
  invitation. The caller chooses the bind address explicitly; use loopback by
  default, and expose a LAN interface only from an explicit sharing action.
  Port zero asks the OS for an available port. The invitation contains its actual
  port and the advertised IP, which must be reachable by the viewer.
- `connect(invitation, name)` starts connection establishment in the background.
  Poll `snapshot()` for progress/errors; its return does not mean connected.
- Both endpoints become `Pending`. Show the owner the incoming `peer_name` and
  separate viewing/control choices. Names are self-asserted, not verified identity.
  `accept(snapshot.request_id, control)` grants exactly this request. `reject(id)`
  sends a rejection and closes it. Requests expire after 60 seconds.
- Only an active host can publish video. `publish_video(request_id, frame)` uses
  an ordered bounded queue, JPEG fallback and negotiated H.264. The viewer requests
  the next frame through `latest_video(request_id, after_sequence)`. A full queue
  returns `Backpressure`; the native encoder must request a fresh IDR, not stop the
  session. `publish_frame(jpeg)` and `latest_frame()` remain JPEG compatibility APIs.
- Only an active viewer granted control may `send_input(event)`. The host drains
  `take_input()` and rechecks active state before OS injection. Move coordinates
  are normalized finite values in `[0,1]`. Key names are finite enum values;
  letters serialize as `a`…`z`, digits as `0`…`9`, arrows as `arrow_up`, etc.
  `ReleaseAll` serializes as `{"type":"release_all"}`; map it to OS key/button
  release on pointer cancellation, window blur and equivalent interruption.
- `stop()` immediately revokes local control, clears frames/queues, cancels network
  work and awaits its completion, including releasing the listening socket.
  Restart closes the previous session; generation checks prevent old tasks from
  changing the replacement's state. **Always release OS-held keys/buttons on any
  transition out of Active**, even when a release event could not be delivered.

`Snapshot` contains role/state/request ID/peer name/control/error. It intentionally
never returns the invitation, token, certificate or private key.

## Wire protocol and trust

TLS is implemented by rustls with the ring provider and standard certificate
verification. Each hosted invitation creates a fresh ephemeral key/certificate
for the `assi.local` server name and a cryptographically random 256-bit token.
The invitation carries the server certificate as the only trusted root, the
literal IP/port, protocol version and token inside base64url JSON. The token is
verified in constant time inside TLS **before** any owner prompt. No unsafe
certificate verifier or self-designed encryption is used. The private key is
never in the invitation.

An invitation is a bearer credential: pass it over a trusted channel; do not log,
persist, publish, include it in analytics or place it in public URL navigation.
Base64 is encoding, not encryption. TLS trust assumes the invitation itself has
not been replaced. An invitation is single-session: accepting one authenticated
viewer closes the listener; rejection, disconnect, stop or restart invalidate it.
There is no account registry, public rendezvous service or UDP hole punching.
An explicit local opt-in can instead call `start_trusted_host` with a persisted
`HostIdentity`: that listener accepts sequential connections bearing the same
credential, grants only its configured screen-control permission, and resets
clipboard/audio/terminal permission for every new peer. Storage and protection of
that identity belong to the native access module. Optional relay routing is below.

TLS payload framing is `u32` big-endian length (including kind byte), `u8` kind,
then body. Kind 1 is bounded JSON control; kind 2 is legacy JPEG; kind 3 is file data;
kind 4 is PCM audio; kind 5 is the versioned video-frame envelope. Flow:
`Hello(token,name)` → `Waiting(request_id,name)` → `Accepted(control)` or
`Rejected`; thereafter host JPEG frames, viewer input and bidirectional heartbeats.
Only messages valid for the current role/state are accepted. The transport is a
prototype protocol, not a published interoperability guarantee. Both clients must
use protocol 4; earlier invitations are explicitly rejected.

Limits:

- Invitation: 16 KiB, certificate: 4 KiB; literal IPv4/IPv6 endpoint, nonzero port.
- Frames: 4 MiB, dimensions at most 4096 each and 8,388,608 pixels; JPEG headers,
  dimensions and boundary markers are checked before a frame reaches rendering.
- Control packets: 8 KiB. Length/kind is checked before allocating payload memory.
- Input queues: 128 events; overflow terminates the session, because silently
  dropping a key-up would leave remote keys held. Video has an eight-frame bound;
  decoder gaps require a keyframe. Monitor changes use screen epochs and reject
  stale input/drawing coordinates until the new frame is displayed.
- Connect/TLS/auth handshake: 8 seconds; owner consent: 60 seconds; socket writes:
  5 seconds; active inbound inactivity: 30 seconds, heartbeats every 5 seconds.
- At most 16 failed authentication/handshake attempts per invitation. One pending
  or active viewer per engine. This limits resources but is not an internet-scale
  abuse protection service; a reachable unauthenticated peer can exhaust attempts.

## Validation

`cargo test -p assi-session` runs synthetic JPEG and actual TLS tests using only
`127.0.0.1` listeners. Tests exercise consent, framing limits, wrong tokens and
certificates, expiration, input permission/overflow, disconnect cleanup, request
invalidation and immediate port reuse after stop. They do not claim to verify
screen capture, OS input injection, mobile permissions or inter-network reachability.

Dependency API references used: [rustls server configuration](https://docs.rs/rustls/0.23.46/rustls/struct.ServerConfig.html),
[tokio-rustls](https://docs.rs/tokio-rustls/0.26.6/tokio_rustls/),
[rcgen certificate generation](https://docs.rs/rcgen/0.14.10/rcgen/fn.generate_simple_self_signed.html).


## Relay routing

`start_host_relay(config: assi_relay::RelayConfig, name)` returns the same confidential
`assi://` invitation form, currently protocol version 4. Both peers must use the
same supported protocol version. The optional relay route includes the outer
relay TLS certificate, endpoint, room and ticket. The host connects outbound and
reserves a short-lived room. A viewer also connects outbound; after pairing,
**a second TLS session is established directly between host and viewer inside the
relay stream**. Its certificate and 256-bit secret come from the invitation, not
from a relay lookup. All existing peer-name/consent/deadline rules apply unchanged.
The relay can see connection metadata and disrupt availability, but cannot decrypt
screen, input or files or replace the inner certificate without TLS rejection.
This assumes that the invitation was delivered over a trusted channel.

The relay is separately self-hosted. This module does not deploy a public service
or promise connectivity until a configured relay is reachable by both peers.
A relay invitation still requires its full secret; a short numeric room alone is
not treated as authenticated device identity. See `../assi-relay/README.md`.

## File API and recipient consent

All APIs require the current accepted session's `request_id`; paths never come
from a remote packet. Native UI must choose paths using the local OS picker.
The sender may call `offer_file(request_id, source: PathBuf) -> transfer_id` only
while Active. The file is opened and metadata validated, but bytes are not read
for transmission until the recipient calls
`accept_file(request_id, transfer_id, destination: PathBuf)`.
`reject_file(request_id, transfer_id)` declines an offer;
`cancel_file(request_id, transfer_id)` cancels an offered/in-progress transfer.
File permission is separate from mouse/keyboard control; the receiver's explicit
per-file approval is always required.

`Snapshot.transfer` is optional and contains `id`, `name`, `size`, `transferred`,
`direction` (`sending`/`receiving`), `state` (`offered`, `transferring`, `paused`,
`completed`, `rejected`, `cancelled`, `error`), pause readiness and resume request and a sanitized optional `error`. No filesystem
path, file bytes, or secret is exposed in the snapshot. Only one file per endpoint
can be offered or transferred at once; simultaneous offers are rejected safely.
Sender progress counts prepared bytes and may lead socket delivery by the bounded
queue; completion is only shown after recipient acknowledgement.

Limits and cleanup:

- Maximum file size 1 GiB. A basename of at most 255 UTF-8 bytes is validated;
  path separators, ADS delimiters, control/bidi characters, reserved Windows names
  and trailing dot/space are rejected. The receiver still chooses its own path.
- Binary file chunks contain UUID (16 bytes), offset (8 bytes), and 1–65536 data
  bytes. Offset, total length and SHA-256 are checked before publication.
- Two prepared chunks can queue; control/input takes priority and file/video
  payloads share a fair select.
  File preparation is paced at 20 ms and is not intended as a high-throughput
  bulk-copy implementation. No entire file is loaded into memory.
- Offers expire after 60 seconds; no transfer progress for 30 seconds cancels the
  file, except a stable user pause. Both errors and cancellation discard partial files.
- Reception uses a mode-private random temporary sibling and `persist_noclobber`.
  A completed destination is never overwritten, including if another process
  creates it after acceptance. Hash and length must match before publication.
- Disk work, hash updates and temporary cleanup run on blocking workers under a
  separate file mutex. Main session state uses an atomic revocation flag and a
  small cached transfer snapshot, so a slow disk operation does not block consent
  revocation or status reads. Stop/restart await cleanup outside the session mutex.
- Revocation is rechecked after the potentially slow `sync_all`, immediately before
  no-clobber publication. An already-started filesystem commit may finish while
  shutdown is pending; shutdown waits for it before returning or starting a new
  session. A transfer that has completed is not deleted by later cancellation.

Validation additionally covers genuine nested TLS through loopback relay, inner
certificate substitution, approved bidirectional multi-chunk/zero-byte files,
wrong size/hash/offset, late destination creation, cancellation, disconnect cleanup,
expired offers and a controlled stop-between-flush-and-commit regression. All files
are synthetic temporary files; no user files, desktop capture or OS input are used.

## Live-session file pause and resume (protocol 4)

`pause_file(request_id, transfer_id)` pauses either direction. Pause is an ordered
barrier: previously queued chunks finish arriving before the receiving side fixes
its byte offset and SHA-256 prefix. `TransferState::Paused` remains busy; the next
queued file must not replace it. `pause_ready` means the barrier completed.

`resume_file` on the sender requests continuation; the receiver must explicitly
call `resume_file` to approve it. The receiver can also initiate continuation from
a ready pause. `resume_requested` reports the pending handshake. Before continuing,
both sides re-read the retained prefix, the sender checks source length and
modification time, and offset/digest must match the checkpoint. Data continues
at that offset; it is not resent from byte zero. Cancellation, integrity failure
and disconnect remove the temporary destination. Final publication remains
no-clobber, including when another file appears at the chosen destination while
paused.

A stable pause does not expire after the normal progress timeout. Incomplete
pause/resume handshakes still time out. Prefix verification uses bounded 64 KiB
reads and checks revocation between reads, but may take time for large files.

This is **not persistent resume after reconnect/restart**. A new session has no
access to the previous transfer or its consent. Persistent recovery requires a
separate peer-bound checkpoint store and renewed recipient approval. Invitations
from prior protocol versions are rejected; both peers must update and exchange a
new invitation.

## Native temporary-directory cleanup barrier

After observing a prior request leave Active, native folder-transfer guards can
call `wait_file_cleanup().await`. It waits for the cleanup operations scheduled
before the call, including blocking operations whose JoinHandles were already
moved into the persistent listener loop. It does not stop that listener or touch
a newer active peer's files. Cleanup success is signalled only after the file
manager releases its source/destination handles. Panics, failed cleanup, and the
five-second wait timeout are errors: retain native directory guards and retry;
never treat an error as permission to delete a directory still in use.
