# assi-access

Opt-in persistent **direct** access for a logged-in desktop user. Construction does
not create a listener, register startup, change OS settings or execute actions.

- `AccessStore::enable` creates a self-signed TLS identity and random 256-bit token,
  replaces existing trust atomically, and returns private material only to native code.
- Unix: dedicated directory owned by the user with mode 0700; identity file 0600;
  reject symlinks, wrong owners and permissive modes. Windows: current-user DPAPI,
  never `CRYPTPROTECT_LOCAL_MACHINE`; reject reparse points.
- `load` is bounded and does not generate replacement secrets on corrupt data.
- `revoke` removes the identity. The app must also stop the current listener.
- Tokens, private keys and invitation are absent from status/Debug output. The
  invitation is a bearer credential: anyone holding it can connect with the saved
  view/control permission. It is revealed only on explicit local action.
- Autostart is separate opt-in. macOS LaunchAgent, Linux XDG autostart, Windows HKCU
  Run register the app at **user login**, never before login. No forced restart or
  auto-respawn on app exit. Keep the installed app at the registered path.
- `SystemAction` is an enum, never arbitrary shell text. Lock and restart are
  implemented for Windows/Linux. macOS/Android return unsupported. The caller must
  enforce session-specific owner approval; this crate grants no authorization.

This is not a Windows service or macOS privileged helper. It does not provide
login-screen/UAC access, NAT traversal, ID discovery or automatic reconnection by
a viewer. Screen/Accessibility permissions are still required. Active sessions
show the normal app UI and can be stopped locally. Stored access is not silently
enabled by merely opening the settings page.

`apps/client/src-tauri/src/access.rs` integrates the store with
`SessionEngine::start_trusted_host`. Startup calls `restore` only when a prior local
opt-in exists. Revoke clears disk trust and stops the listener. A session stop is
separate from persistent trust revocation; use the settings action to revoke it.
Status distinguishes configured trust from a running listener; Resume restores
saved trust without rotating the invitation. Re-enabling with new configuration
rotates both certificate and token.

System requests use typed messages, a single pending request, bounded replay
protection, and an explicit host-only decision tied to the session/request ID.
Only that decision can return an action to the native executor. Result `dispatched`
means the OS accepted the command, not that a restart completed. A restart can
close the connection before its result reaches the viewer. Spawned OS commands
have a five-second deadline; tests never execute them.

Validation uses temp directories and loopback sessions. Tests never register real
autostart, lock the developer's screen or restart the machine.

Primary references:
- [Apple launchd jobs](https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/CreatingLaunchdJobs.html)
- [Desktop Entry Exec quoting](https://specifications.freedesktop.org/desktop-entry/latest-single/)
- [XDG autostart](https://specifications.freedesktop.org/autostart/latest/)
- [Microsoft DPAPI](https://learn.microsoft.com/en-us/windows/win32/api/dpapi/nf-dpapi-cryptprotectdata)
- [Microsoft Run keys](https://learn.microsoft.com/en-us/windows/win32/setupapi/run-and-runonce-registry-keys)
- [Microsoft LockWorkStation](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-lockworkstation)
