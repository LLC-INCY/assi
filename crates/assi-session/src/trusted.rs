//! Persistent direct listener enabled only through a local opt-in. Possession of
//! the full invitation is the trust credential; it grants exactly allow_control.
//! Other session permissions (audio, clipboard, terminal) still start disabled.
use super::*;

// No Debug/Serialize: do not log private keys or bearer tokens.
pub struct HostIdentity {
    pub certificate_der: Vec<u8>,
    pub private_key_pkcs8: Vec<u8>,
    pub token: [u8; 32],
}
impl HostIdentity {
    fn validate(&self) -> Result<()> {
        if self.certificate_der.is_empty()
            || self.certificate_der.len() > 4096
            || self.private_key_pkcs8.is_empty()
            || self.private_key_pkcs8.len() > 4096
            || self.token == [0; 32]
        {
            return Err(Error::Authentication);
        }
        Ok(())
    }
    pub fn invitation(&self, endpoint: SocketAddr) -> Result<String> {
        self.validate()?;
        if endpoint.port() == 0 || !usable_ip(endpoint.ip()) {
            return Err(Error::InvalidInvitation);
        }
        Invitation {
            version: PROTOCOL_VERSION,
            endpoint,
            relay: None,
            certificate: URL_SAFE_NO_PAD.encode(&self.certificate_der),
            token: URL_SAFE_NO_PAD.encode(self.token),
        }
        .encode()
    }
}
impl SessionEngine {
    pub fn trusted_listener_active(&self) -> bool {
        self.lock()
            .map(|i| {
                i.auto_accept.is_some()
                    && i.snapshot.role == Role::Host
                    && matches!(
                        i.snapshot.state,
                        SessionState::Listening | SessionState::Pending | SessionState::Active
                    )
            })
            .unwrap_or(false)
    }
    /// A local explicit grant is required before calling this API. Keeps one
    /// direct listener alive across sequential authenticated connections.
    pub async fn start_trusted_host(
        self: &Arc<Self>,
        bind: SocketAddr,
        advertised_ip: IpAddr,
        name: String,
        identity: HostIdentity,
        allow_control: bool,
    ) -> Result<String> {
        if !name_ok(&name) {
            return Err(Error::InvalidName);
        }
        if bind.port() == 0 || !usable_ip(advertised_ip) {
            return Err(Error::InvalidInvitation);
        }
        identity.validate()?;
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|_| Error::Tls)?
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(identity.certificate_der.clone())],
            PrivatePkcs8KeyDer::from(identity.private_key_pkcs8.clone()).into(),
        )
        .map_err(|_| Error::Tls)?;
        let _op = self.lifecycle.lock().await;
        self.shutdown_current().await?;
        let listener = TcpListener::bind(bind).await?;
        let invitation = identity.invitation(SocketAddr::new(
            advertised_ip,
            listener.local_addr()?.port(),
        ))?;
        let (generation, mut cancel) = self.reset(Role::Host, SessionState::Listening)?;
        self.transition(generation, |inner| inner.auto_accept = Some(allow_control))?;
        let engine = self.clone();
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let task = tokio::spawn(async move {
            let result = tokio::select! {biased;_=cancel.changed()=>Ok(()),result=engine.trusted_host_loop(generation,listener,acceptor,identity.token,name)=>result};
            engine.finish(generation, result);
        });
        self.lock()?.task = Some(task);
        Ok(invitation)
    }
    async fn trusted_host_loop(
        self: &Arc<Self>,
        generation: u64,
        listener: TcpListener,
        acceptor: TlsAcceptor,
        token: [u8; 32],
        name: String,
    ) -> Result<()> {
        loop {
            let (tcp, _) = listener.accept().await?;
            tcp.set_nodelay(true)?;
            let handshake = timeout(HANDSHAKE_TIMEOUT, async {
                let mut tls = acceptor.accept(tcp).await.map_err(|_| Error::Tls)?;
                let peer = read_hello(&mut tls, &token).await?;
                Ok::<_, Error>((tls, peer))
            })
            .await;
            if let Ok(Ok((tls, peer))) = handshake {
                // A peer error ends only that connection, not the opted-in listener.
                let _ = self.host_session(generation, tls, peer, name.clone()).await;
                self.reset_trusted_peer(generation)?;
                let cleanup = { std::mem::take(&mut self.lock()?.file_cleanup) };
                for task in cleanup {
                    task.await.map_err(|_| Error::State)??;
                }
            }
            // Bound repeated failed authentication and rapid reconnect loops.
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn identity() -> HostIdentity {
        let key = rcgen::generate_simple_self_signed(vec!["assi.local".into()]).unwrap();
        let mut token = [0; 32];
        SystemRandom::new().fill(&mut token).unwrap();
        HostIdentity {
            certificate_der: key.cert.der().to_vec(),
            private_key_pkcs8: key.signing_key.serialize_der(),
            token,
        }
    }
    async fn wait_state(engine: &SessionEngine, state: SessionState) {
        timeout(Duration::from_secs(3), async {
            loop {
                if engine.snapshot().state == state {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn trusted_host_reaccepts_with_fresh_request_and_cleared_permissions() {
        let reserve = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = reserve.local_addr().unwrap();
        drop(reserve);
        let host = SessionEngine::new();
        let viewer = SessionEngine::new();
        let invite = host
            .start_trusted_host(address, address.ip(), "Owner".into(), identity(), false)
            .await
            .unwrap();
        viewer
            .connect(invite.clone(), "Viewer".into())
            .await
            .unwrap();
        wait_state(&viewer, SessionState::Active).await;
        let first = host.snapshot().request_id;
        assert!(!host.snapshot().control);
        assert!(!host.snapshot().audio_enabled);
        viewer.stop().await.unwrap();
        wait_state(&host, SessionState::Listening).await;
        viewer.connect(invite, "Viewer".into()).await.unwrap();
        wait_state(&viewer, SessionState::Active).await;
        assert_ne!(first, host.snapshot().request_id);
        assert!(!host.snapshot().audio_enabled);
        host.stop().await.unwrap();
        assert!(!host.trusted_listener_active());
        viewer.stop().await.unwrap();
    }
    #[tokio::test]
    async fn stopped_trusted_listener_does_not_accept_old_invitation() {
        let reserve = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = reserve.local_addr().unwrap();
        drop(reserve);
        let host = SessionEngine::new();
        let viewer = SessionEngine::new();
        let invite = host
            .start_trusted_host(address, address.ip(), "Owner".into(), identity(), true)
            .await
            .unwrap();
        host.stop().await.unwrap();
        viewer.connect(invite, "Viewer".into()).await.unwrap();
        wait_state(&viewer, SessionState::Error).await;
        viewer.stop().await.unwrap();
    }
    #[tokio::test]
    async fn passive_disconnect_cleanup_barrier_keeps_trusted_listener_alive() {
        let reserve = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = reserve.local_addr().unwrap();
        drop(reserve);
        let host = SessionEngine::new();
        let viewer = SessionEngine::new();
        let invite = host
            .start_trusted_host(address, address.ip(), "Owner".into(), identity(), false)
            .await
            .unwrap();
        viewer
            .connect(invite.clone(), "Viewer".into())
            .await
            .unwrap();
        wait_state(&host, SessionState::Active).await;
        wait_state(&viewer, SessionState::Active).await;
        let request = host.snapshot().request_id.unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let source = source_dir.path().join("transfer.bin");
        std::fs::write(&source, vec![3u8; 1024 * 1024]).unwrap();
        let destination_dir = tempfile::tempdir().unwrap();
        let destination = destination_dir.path().join("received.bin");
        let transfer = viewer.offer_file(request.clone(), source).await.unwrap();
        timeout(Duration::from_secs(3), async {
            while host.snapshot().transfer.is_none() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        host.accept_file(request.clone(), transfer.clone(), destination)
            .await
            .unwrap();
        timeout(Duration::from_secs(3), async {
            while host.snapshot().transfer.is_none_or(|t| t.transferred == 0) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        host.pause_file(request.clone(), transfer).await.unwrap();
        timeout(Duration::from_secs(3), async {
            while !host.snapshot().transfer.is_some_and(|t| t.pause_ready)
                || !viewer.snapshot().transfer.is_some_and(|t| t.pause_ready)
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let files = host.lock().unwrap().files.clone().unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocked = std::thread::spawn(move || {
            files.run(|_| {
                entered_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(4)).unwrap();
                Ok(())
            })
        });
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        viewer.stop().await.unwrap();
        wait_state(&host, SessionState::Listening).await;
        // trusted_host_loop has already taken the JoinHandles out of Inner.
        timeout(Duration::from_secs(1), async {
            while !host.lock().unwrap().file_cleanup.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let waiting = host.clone();
        let barrier = tokio::spawn(async move { waiting.wait_file_cleanup().await });
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(!barrier.is_finished());
        assert_eq!(host.snapshot().state, SessionState::Listening);
        assert_eq!(
            std::fs::read_dir(destination_dir.path()).unwrap().count(),
            1
        );
        release_tx.send(()).unwrap();
        blocked.join().unwrap().unwrap();
        barrier.await.unwrap().unwrap();
        assert_eq!(
            std::fs::read_dir(destination_dir.path()).unwrap().count(),
            0
        );
        assert_eq!(host.snapshot().state, SessionState::Listening);
        viewer.connect(invite, "Next peer".into()).await.unwrap();
        wait_state(&viewer, SessionState::Active).await;
        assert_ne!(
            host.snapshot().request_id.as_deref(),
            Some(request.as_str())
        );
        host.stop().await.unwrap();
        viewer.stop().await.unwrap();
    }
}
