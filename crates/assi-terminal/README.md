# assi-terminal

Opt-in PTY backend for an approved session. Current native support: macOS/Linux.
Windows and Android return `Unsupported`; no fake shell or elevated fallback.

`TerminalManager::start_approved(request_id, cols, rows)` must only be called from
local owner approval, after the active session/request has been checked. Screen
control permission never grants terminal access. Shell executable is selected
locally (`SHELL` absolute existing path or `/bin/sh`), never from network input.
The shell runs as the application user, with `TERM=dumb`.

One nonblocking worker owns the PTY and child. Input chunks are at most 1 KiB,
input channel 16 messages, pending input/output each bounded at 64 KiB. Overflow
ends the PTY rather than accumulating memory. Stop signals the worker immediately;
`finish_stop(timeout)` is a blocking drain for shutdown, and must run in a blocking
executor. Restart is refused until the previous worker has finished. Worker kills
its foreground process group and shell, closes descriptors and reaps the child.
This is not a sandbox; deliberately detached processes started by an authorized
shell user are not guaranteed to terminate with the terminal.

`drain` consumes up to 1 KiB at a time. Preserve byte order and stream UTF-8 decoding.
A plain transcript UI must remove terminal escape sequences, render via textContent,
and never execute OSC clipboard/link commands. The crate itself does not render.

Protocol is `assi-session/src/terminal.rs`: viewer request → explicit host approval
→ epoch-scoped input/output. On disconnect or revocation, the application must
stop the PTY before accepting another request. Terminal control is separate from
screen control, clipboard, and persistent device trust.

Validated locally: real `/bin/echo` output, stale request rejection, invalid sizes
and chunks, `/bin/sleep` termination. Tests intentionally do not start the user's
interactive shell or execute commands received from an external device.

Primary API reference: https://docs.rs/portable-pty/0.9.0/portable_pty/
