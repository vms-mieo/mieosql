//! SSH tunnel (direct-tcpip port forwarding) for database connections.
//!
//! [`SshTunnel`] opens an SSH connection to the jump host described by a
//! [`SshSpec`], authenticates with a password, a private key or the user's
//! ssh-agent, then listens on a random loopback port. Every TCP connection
//! accepted there is forwarded (`direct-tcpip`) to the database endpoint on
//! the far side of the SSH connection.
//!
//! Drivers keep the returned tunnel in their struct for the lifetime of the
//! database connection; dropping it aborts the accept loop, which releases
//! the SSH session.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use russh::client::{self, Config, Handle};
use russh::keys::agent::AgentIdentity;
use russh::keys::agent::client::AgentClient;
use russh::keys::{
    PrivateKeyWithHashAlg, PublicKeyOrCertificate, check_known_hosts, load_secret_key,
};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use mieosql_core::{ConnectionSpec, ErrorCode, MieoError, MieoResult, SshAuthKind, SshSpec};

/// A live SSH tunnel: local listener → SSH direct-tcpip → remote host:port.
pub struct SshTunnel {
    local_port: u16,
    /// Keeps the SSH session alive; every forwarded connection opens its
    /// channel through this handle.
    handle: Arc<Handle<TunnelHandler>>,
    /// Accept loop task; aborted when the tunnel is dropped.
    accept_task: tokio::task::JoinHandle<()>,
}

impl SshTunnel {
    /// Loopback port the drivers should connect to.
    pub fn local_port(&self) -> u16 {
        self.local_port
    }

    /// Open an SSH session to `spec` and forward `target_host:target_port`
    /// through it. `password` is the SSH password / key passphrase.
    pub async fn open(
        spec: &SshSpec,
        target_host: &str,
        target_port: u16,
        password: Option<&str>,
        timeout: Duration,
    ) -> MieoResult<Self> {
        let reason = Arc::new(Mutex::new(None));
        let handler = TunnelHandler {
            host: spec.host.clone(),
            port: spec.port,
            host_key_check: spec.host_key_check,
            reason: Arc::clone(&reason),
        };
        let config = Arc::new(Config {
            nodelay: true,
            keepalive_interval: Some(Duration::from_secs(30)),
            ..Default::default()
        });

        let mut session = match tokio::time::timeout(
            timeout,
            client::connect(config, (spec.host.as_str(), spec.port), handler),
        )
        .await
        {
            Err(_) => {
                return Err(MieoError::new(
                    ErrorCode::Timeout,
                    format!("SSH connection to {}:{} timed out", spec.host, spec.port),
                ));
            }
            Ok(Err(err)) => {
                let reason = reason.lock().await.take();
                return Err(connect_error(err, reason, spec));
            }
            Ok(Ok(session)) => session,
        };

        match tokio::time::timeout(timeout, authenticate(&mut session, spec, password)).await {
            Err(_) => {
                return Err(MieoError::new(
                    ErrorCode::Timeout,
                    format!(
                        "SSH authentication for {}@{}:{} timed out",
                        spec.username, spec.host, spec.port
                    ),
                ));
            }
            Ok(result) => result?,
        }

        let listener = TcpListener::bind(("127.0.0.1", 0)).await.map_err(|err| {
            MieoError::new(
                ErrorCode::Connection,
                format!("cannot bind the SSH tunnel listener: {err}"),
            )
        })?;
        let local_port = listener
            .local_addr()
            .map_err(|err| {
                MieoError::new(
                    ErrorCode::Connection,
                    format!("cannot read the SSH tunnel listener address: {err}"),
                )
            })?
            .port();

        let handle = Arc::new(session);
        let accept_handle = Arc::clone(&handle);
        let target_host = target_host.to_string();
        let accept_task = tokio::spawn(async move {
            loop {
                let (mut socket, _) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(err) => {
                        tracing::warn!("SSH tunnel listener stopped: {err}");
                        break;
                    }
                };
                let handle = Arc::clone(&accept_handle);
                let target_host = target_host.clone();
                tokio::spawn(async move {
                    match handle
                        .channel_open_direct_tcpip(
                            target_host.as_str(),
                            u32::from(target_port),
                            "127.0.0.1",
                            0,
                        )
                        .await
                    {
                        Ok(channel) => {
                            // ChannelStream implements tokio's AsyncRead/AsyncWrite.
                            let mut remote = channel.into_stream();
                            if let Err(err) =
                                tokio::io::copy_bidirectional(&mut socket, &mut remote).await
                            {
                                tracing::debug!("SSH tunnel stream closed: {err}");
                            }
                        }
                        Err(err) => {
                            tracing::warn!(
                                "SSH tunnel could not open a channel to {target_host}:{target_port}: {err}"
                            );
                        }
                    }
                });
            }
        });

        Ok(Self {
            local_port,
            handle,
            accept_task,
        })
    }
}

impl Drop for SshTunnel {
    fn drop(&mut self) {
        self.accept_task.abort();
        // Dropping the last `handle` clone ends the russh session task and
        // closes the underlying TCP connection.
        let _ = &self.handle;
    }
}

/// Resolve the endpoint a driver should connect to, opening an SSH tunnel
/// when `spec.ssh` is set. The returned tunnel must be kept alive by the
/// caller (driver struct) for as long as the database connection lives.
pub(crate) async fn prepare_endpoint(
    spec: &ConnectionSpec,
    default_host: &str,
    default_port: u16,
) -> MieoResult<(String, u16, Option<SshTunnel>)> {
    let host = spec
        .host
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .unwrap_or(default_host)
        .to_string();
    let port = spec.port.unwrap_or(default_port);
    let Some(ssh) = spec.ssh.as_ref() else {
        return Ok((host, port, None));
    };
    let timeout = Duration::from_secs(spec.connect_timeout_secs.max(1));
    let tunnel = SshTunnel::open(ssh, &host, port, spec.ssh_password.as_deref(), timeout).await?;
    Ok(("127.0.0.1".to_string(), tunnel.local_port(), Some(tunnel)))
}

/// Fail fast for engines that do not support tunnels yet, instead of
/// silently connecting directly and bypassing the jump host.
pub(crate) fn ensure_no_tunnel(spec: &ConnectionSpec, engine_label: &str) -> MieoResult<()> {
    if spec.ssh.is_some() {
        return Err(MieoError::unsupported(format!(
            "SSH tunnels are not supported for {engine_label} yet — disable the tunnel in the connection profile"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// russh client handler + authentication
// ---------------------------------------------------------------------------

struct TunnelHandler {
    host: String,
    port: u16,
    host_key_check: bool,
    /// Human-readable rejection reason, filled by `check_server_key`.
    reason: Arc<Mutex<Option<String>>>,
}

impl client::Handler for TunnelHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        if !self.host_key_check {
            tracing::warn!(
                ssh_host = %self.host,
                ssh_port = self.port,
                "SSH host key verification is disabled for this connection"
            );
            return Ok(true);
        }
        let key = server_public_key.public_key();
        match check_known_hosts(&self.host, self.port, &key) {
            Ok(true) => Ok(true),
            Ok(false) => {
                *self.reason.lock().await = Some(format!(
                    "host key for {}:{} is not in ~/.ssh/known_hosts",
                    self.host, self.port
                ));
                Ok(false)
            }
            Err(err) => {
                *self.reason.lock().await = Some(format!(
                    "host key verification failed for {}:{}: {err}",
                    self.host, self.port
                ));
                Ok(false)
            }
        }
    }
}

async fn authenticate(
    session: &mut Handle<TunnelHandler>,
    spec: &SshSpec,
    password: Option<&str>,
) -> MieoResult<()> {
    match spec.auth {
        SshAuthKind::Password => {
            let result = session
                .authenticate_password(spec.username.as_str(), password.unwrap_or(""))
                .await
                .map_err(|err| ssh_error(spec, err.to_string()))?;
            if !result.success() {
                return Err(auth_failed(spec, "password rejected by the server"));
            }
        }
        SshAuthKind::PrivateKey => {
            let path = spec
                .private_key_path
                .as_deref()
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .ok_or_else(|| {
                    MieoError::new(
                        ErrorCode::Auth,
                        "SSH private key path is required for private-key authentication",
                    )
                })?;
            let path = expand_tilde(path);
            let key = load_secret_key(&path, password).map_err(|err| {
                MieoError::new(
                    ErrorCode::Auth,
                    format!("cannot load SSH private key {}: {err}", path.display()),
                )
                .with_hint("Check the path and set the key passphrase if the key is encrypted.")
            })?;
            let hash = session
                .best_supported_rsa_hash()
                .await
                .map_err(|err| ssh_error(spec, err.to_string()))?
                .flatten();
            let result = session
                .authenticate_publickey(
                    spec.username.as_str(),
                    PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                )
                .await
                .map_err(|err| ssh_error(spec, err.to_string()))?;
            if !result.success() {
                return Err(auth_failed(spec, "public key rejected by the server"));
            }
        }
        SshAuthKind::Agent => {
            authenticate_with_agent(session, spec).await?;
        }
    }
    Ok(())
}

#[cfg(unix)]
async fn authenticate_with_agent(
    session: &mut Handle<TunnelHandler>,
    spec: &SshSpec,
) -> MieoResult<()> {
    let mut agent = AgentClient::connect_env().await.map_err(|err| {
        MieoError::new(
            ErrorCode::Auth,
            format!("cannot connect to the SSH agent: {err}"),
        )
        .with_hint("Make sure ssh-agent is running and SSH_AUTH_SOCK is set.")
    })?;
    let identities = agent.request_identities().await.map_err(|err| {
        MieoError::new(
            ErrorCode::Auth,
            format!("cannot list SSH agent identities: {err}"),
        )
    })?;
    if identities.is_empty() {
        return Err(
            MieoError::new(ErrorCode::Auth, "the SSH agent holds no identities")
                .with_hint("Add a key with `ssh-add` first."),
        );
    }

    let hash = session
        .best_supported_rsa_hash()
        .await
        .map_err(|err| ssh_error(spec, err.to_string()))?
        .flatten();
    let mut last_error = None;
    for identity in &identities {
        let result = match identity {
            AgentIdentity::PublicKey { key, .. } => {
                session
                    .authenticate_publickey_with(
                        spec.username.as_str(),
                        key.clone(),
                        hash,
                        &mut agent,
                    )
                    .await
            }
            AgentIdentity::Certificate { certificate, .. } => {
                session
                    .authenticate_certificate_with(
                        spec.username.as_str(),
                        certificate.clone(),
                        hash,
                        &mut agent,
                    )
                    .await
            }
        };
        match result {
            Ok(result) if result.success() => return Ok(()),
            Ok(_) => last_error = Some("agent key rejected by the server".to_string()),
            Err(err) => last_error = Some(err.to_string()),
        }
    }
    Err(auth_failed(
        spec,
        last_error
            .as_deref()
            .unwrap_or("no agent identity was accepted"),
    ))
}

#[cfg(not(unix))]
async fn authenticate_with_agent(
    _session: &mut Handle<TunnelHandler>,
    _spec: &SshSpec,
) -> MieoResult<()> {
    Err(MieoError::unsupported(
        "SSH agent authentication is only supported on Unix platforms",
    ))
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(path)
}

fn ssh_error(spec: &SshSpec, detail: impl std::fmt::Display) -> MieoError {
    MieoError::new(
        ErrorCode::Connection,
        format!(
            "SSH connection to {}:{} failed: {detail}",
            spec.host, spec.port
        ),
    )
}

fn auth_failed(spec: &SshSpec, detail: &str) -> MieoError {
    MieoError::new(
        ErrorCode::Auth,
        format!(
            "SSH authentication failed for {}@{}:{} — {detail}",
            spec.username, spec.host, spec.port
        ),
    )
}

fn connect_error(err: russh::Error, reason: Option<String>, spec: &SshSpec) -> MieoError {
    let unknown_key = matches!(err, russh::Error::UnknownKey);
    let message = reason.unwrap_or_else(|| {
        format!(
            "SSH connection to {}:{} failed: {err}",
            spec.host, spec.port
        )
    });
    let mut error = MieoError::new(
        if unknown_key {
            ErrorCode::Auth
        } else {
            ErrorCode::Connection
        },
        message,
    );
    if unknown_key {
        error = error.with_hint(format!(
            "Approve the host key with `ssh-keyscan -p {} {}` added to ~/.ssh/known_hosts, or disable host key verification in the profile.",
            spec.port, spec.host
        ));
    }
    error
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use mieosql_core::{Engine, SslMode};

    fn ssh_spec() -> SshSpec {
        SshSpec {
            host: "jump.example.com".into(),
            port: 22,
            username: "ubuntu".into(),
            auth: SshAuthKind::Password,
            private_key_path: None,
            host_key_check: true,
        }
    }

    fn spec(ssh: Option<SshSpec>) -> ConnectionSpec {
        ConnectionSpec {
            profile_id: None,
            name: "test".into(),
            engine: Engine::Postgres,
            host: Some("db.internal".into()),
            port: Some(5432),
            username: None,
            password: None,
            database: None,
            file: None,
            params: BTreeMap::new(),
            ssl_mode: SslMode::Prefer,
            variant: None,
            ssh,
            ssh_password: None,
            connect_timeout_secs: 15,
        }
    }

    #[tokio::test]
    async fn without_ssh_the_endpoint_is_used_directly() {
        let (host, port, tunnel) = prepare_endpoint(&spec(None), "127.0.0.1", 5432)
            .await
            .unwrap();
        assert_eq!(host, "db.internal");
        assert_eq!(port, 5432);
        assert!(tunnel.is_none());
    }

    #[tokio::test]
    async fn missing_host_falls_back_to_the_default() {
        let mut spec = spec(None);
        spec.host = None;
        spec.port = None;
        let (host, port, _) = prepare_endpoint(&spec, "127.0.0.1", 5432).await.unwrap();
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 5432);
    }

    #[test]
    fn ensure_no_tunnel_rejects_profiles_with_ssh() {
        let err = ensure_no_tunnel(&spec(Some(ssh_spec())), "MongoDB").unwrap_err();
        assert_eq!(err.code(), ErrorCode::Unsupported);
        assert!(ensure_no_tunnel(&spec(None), "MongoDB").is_ok());
    }

    #[test]
    fn expand_tilde_resolves_the_home_directory() {
        let expanded = expand_tilde("~/.ssh/id_ed25519");
        assert!(expanded.is_absolute());
        assert!(expanded.to_string_lossy().ends_with(".ssh/id_ed25519"));
        assert_eq!(expand_tilde("/tmp/key"), PathBuf::from("/tmp/key"));
    }
}

#[cfg(test)]
mod e2e {
    //! End-to-end tunnel test: an in-process russh SSH server (password auth,
    //! direct-tcpip forwarding) in front of a TCP echo server.

    use std::sync::Arc;
    use std::time::Duration;

    use russh::keys::{Algorithm, PrivateKey};
    use russh::server::{Auth, Msg, Server as _, Session};
    use russh::{Channel, server};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    #[derive(Clone)]
    struct TestSshServer;

    impl server::Server for TestSshServer {
        type Handler = Self;

        fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
            self.clone()
        }
    }

    impl server::Handler for TestSshServer {
        type Error = russh::Error;

        async fn auth_password(
            &mut self,
            _user: &str,
            password: &str,
        ) -> Result<Auth, Self::Error> {
            Ok(if password == "secret" {
                Auth::Accept
            } else {
                Auth::Reject {
                    proceed_with_methods: None,
                    partial_success: false,
                }
            })
        }

        async fn channel_open_direct_tcpip(
            &mut self,
            channel: Channel<Msg>,
            host_to_connect: &str,
            port_to_connect: u32,
            _originator_address: &str,
            _originator_port: u32,
            reply: server::ChannelOpenHandle,
            _session: &mut Session,
        ) -> Result<(), Self::Error> {
            reply.accept().await;
            let target = format!("{host_to_connect}:{port_to_connect}");
            tokio::spawn(async move {
                match tokio::net::TcpStream::connect(target.as_str()).await {
                    Ok(mut tcp) => {
                        let mut stream = channel.into_stream();
                        let _ = tokio::io::copy_bidirectional(&mut stream, &mut tcp).await;
                    }
                    Err(err) => {
                        tracing::warn!("test SSH server cannot reach {target}: {err}");
                        let _ = channel.close().await;
                    }
                }
            });
            Ok(())
        }
    }

    /// TCP echo server used as the "database" behind the jump host.
    async fn echo_server() -> std::net::SocketAddr {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    loop {
                        match socket.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                if socket.write_all(&buf[..n]).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                });
            }
        });
        addr
    }

    async fn start_ssh_server() -> (u16, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let config = Arc::new(server::Config {
            keys: vec![PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap()],
            auth_rejection_time: Duration::from_millis(10),
            auth_rejection_time_initial: Some(Duration::from_millis(0)),
            inactivity_timeout: Some(Duration::from_secs(60)),
            ..Default::default()
        });
        let mut server = TestSshServer;
        let task = tokio::spawn(async move {
            let _ = server.run_on_socket(config, &listener).await;
        });
        (port, task)
    }

    fn ssh_spec(port: u16) -> SshSpec {
        SshSpec {
            host: "127.0.0.1".into(),
            port,
            username: "tester".into(),
            auth: SshAuthKind::Password,
            private_key_path: None,
            host_key_check: false,
        }
    }

    #[tokio::test]
    async fn tunnel_forwards_tcp_through_password_auth() {
        let echo_addr = echo_server().await;
        let (ssh_port, server_task) = start_ssh_server().await;

        let tunnel = SshTunnel::open(
            &ssh_spec(ssh_port),
            "127.0.0.1",
            echo_addr.port(),
            Some("secret"),
            Duration::from_secs(10),
        )
        .await
        .unwrap();

        let mut client = tokio::net::TcpStream::connect(("127.0.0.1", tunnel.local_port()))
            .await
            .unwrap();
        client.write_all(b"hello through the tunnel").await.unwrap();
        let mut buf = vec![0u8; 64];
        let n = tokio::time::timeout(Duration::from_secs(10), client.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&buf[..n], b"hello through the tunnel");

        drop(tunnel);
        server_task.abort();
    }

    #[tokio::test]
    async fn wrong_password_fails_with_an_auth_error() {
        let echo_addr = echo_server().await;
        let (ssh_port, server_task) = start_ssh_server().await;

        let err = SshTunnel::open(
            &ssh_spec(ssh_port),
            "127.0.0.1",
            echo_addr.port(),
            Some("wrong"),
            Duration::from_secs(10),
        )
        .await
        .err()
        .expect("expected an authentication error");
        assert_eq!(err.code(), ErrorCode::Auth);

        server_task.abort();
    }
}
