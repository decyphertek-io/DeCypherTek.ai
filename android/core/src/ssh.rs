//! SSH transport — the only road into the VM.
//!
//! russh (pure Rust, rustls) speaks SSH to the guest's sshd through the
//! loopback forward QEMU holds on 127.0.0.1:<port>. Two auth modes:
//! the app's ed25519 key (sealed in the lock profile) once provisioned,
//! or the guest's auto-generated one-time first-boot secret during
//! provisioning (no credential ships anywhere — the guest mints it at
//! first boot and the app reads it from the serial log, once).
//! The guest host key is pinned at provisioning time; every later
//! connection verifies it and refuses anything else.
//!
//! Terminal sessions are a PTY + shell on one channel: reads are pushed
//! to the Flutter terminal as base64 events, writes/resize/close are
//! serialized through a per-session action queue so keystroke order is
//! exactly the order the user typed.

use anyhow::{anyhow, bail, Result};
use russh::keys::{Algorithm, PrivateKey, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::{client, Channel, ChannelMsg};
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};

type ClientChannel = Channel<client::Msg>;

/// Zero-dependency OS RNG: satisfies ssh-key's rand_core 0.10 bounds using
/// getrandom (the same primitive the vault uses for nonces).
struct OsRng;
impl rand_core::TryRng for OsRng {
    type Error = Infallible;
    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        let mut b = [0u8; 4];
        getrandom::getrandom(&mut b).expect("getrandom");
        Ok(u32::from_le_bytes(b))
    }
    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        let mut b = [0u8; 8];
        getrandom::getrandom(&mut b).expect("getrandom");
        Ok(u64::from_le_bytes(b))
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), Self::Error> {
        getrandom::getrandom(dest).expect("getrandom");
        Ok(())
    }
}
impl rand_core::TryCryptoRng for OsRng {}

/// Credentials for one connection.
pub struct Auth {
    /// Host to dial — always loopback in practice (the QEMU forward).
    pub host: String,
    pub user: String,
    pub key: Option<String>,
    pub password: Option<String>,
    /// Pinned host key (openssh public key line). `None` only during
    /// provisioning, where the pin is being established.
    pub host_key_pin: Option<String>,
}

fn auth_host(auth: &Auth) -> &str {
    if auth.host.is_empty() {
        "127.0.0.1"
    } else {
        &auth.host
    }
}

/// Generate a fresh ed25519 keypair. Returns (private openssh PEM, public
/// openssh line).
pub fn generate_keypair() -> Result<(String, String)> {
    let mut rng = OsRng;
    let key =
        PrivateKey::random(&mut rng, Algorithm::Ed25519).map_err(|e| anyhow!("keygen: {e}"))?;
    let pem = key
        .to_openssh(russh::keys::ssh_key::LineEnding::LF)
        .map_err(|e| anyhow!("encode private key: {e}"))?
        .to_string();
    let pub_line = key
        .public_key()
        .to_openssh()
        .map_err(|e| anyhow!("encode public key: {e}"))?;
    Ok((pem, pub_line))
}

pub struct ClientHandler {
    pin: Option<String>,
    seen: Arc<std::sync::Mutex<Option<String>>>,
}

impl client::Handler for ClientHandler {
    type Error = anyhow::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let line = server_public_key
            .public_key()
            .to_openssh()
            .map_err(|e| anyhow!("encode host key: {e}"))?;
        *self.seen.lock().unwrap() = Some(line.clone());
        match &self.pin {
            None => Ok(true), // provisioning: trust on first use, pin after
            Some(pin) => Ok(line.trim() == pin.trim()),
        }
    }
}

/// Connect + authenticate on `port` (the loopback forward), retrying until
/// the guest's sshd answers or `timeout_secs` elapses (a fresh boot is
/// slow). Returns the handle and the host key line the server presented.
pub async fn connect_port(
    auth: &Auth,
    port: u16,
    timeout_secs: u64,
) -> Result<(client::Handle<ClientHandler>, String)> {
    let config = Arc::new(client::Config {
        keepalive_interval: Some(Duration::from_secs(15)),
        ..Default::default()
    });
    let seen = Arc::new(std::sync::Mutex::new(None));
    let handler = ClientHandler {
        pin: auth.host_key_pin.clone(),
        seen: seen.clone(),
    };

    let deadline = Duration::from_secs(timeout_secs);
    let started = std::time::Instant::now();
    let mut handle = loop {
        match client::connect(
            config.clone(),
            (auth_host(auth), port),
            handler_clone(&handler),
        )
        .await
        {
            Ok(handle) => break handle,
            Err(e) => {
                if started.elapsed() >= deadline {
                    bail!("ssh: no answer from the VM: {e}");
                }
            }
        }
        sleep(Duration::from_secs(2)).await;
    };

    let authed = if let Some(pem) = &auth.key {
        let key = PrivateKey::from_openssh(pem).map_err(|e| anyhow!("parse key: {e}"))?;
        handle
            .authenticate_publickey(
                auth.user.clone(),
                PrivateKeyWithHashAlg::new(Arc::new(key), None),
            )
            .await?
    } else if let Some(pw) = &auth.password {
        handle.authenticate_password(auth.user.clone(), pw).await?
    } else {
        bail!("no credentials provided");
    };
    if !authed.success() {
        bail!("ssh authentication failed");
    }
    let host_key = seen.lock().unwrap().clone();
    Ok((
        handle,
        host_key.ok_or_else(|| anyhow!("host key not captured"))?,
    ))
}

/// The handler is consumed by connect(); rebuild it with the same pin for
/// each retry.
fn handler_clone(h: &ClientHandler) -> ClientHandler {
    ClientHandler {
        pin: h.pin.clone(),
        seen: h.seen.clone(),
    }
}

/// Run one command over a fresh exec channel, streaming output lines to
/// `on_line`. Returns the exit status.
pub async fn exec<F>(
    handle: &client::Handle<ClientHandler>,
    cmd: &str,
    stdin: Option<&[u8]>,
    on_line: F,
) -> Result<i32>
where
    F: Fn(String) + Send + 'static,
{
    let channel: ClientChannel = handle.channel_open_session().await?;
    let (mut read_half, write_half) = channel.split();
    write_half.exec(true, cmd).await?;
    if let Some(data) = stdin {
        write_half.data_bytes(data.to_vec()).await?;
        write_half.eof().await?;
    }
    let mut code = None;
    let mut pending = String::new();
    while let Some(msg) = read_half.wait().await {
        match msg {
            ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                pending.push_str(&String::from_utf8_lossy(&data));
                while let Some(pos) = pending.find('\n') {
                    let line: String = pending.drain(..=pos).collect();
                    on_line(line.trim_end().to_string());
                }
            }
            ChannelMsg::ExitStatus { exit_status } => code = Some(exit_status as i32),
            ChannelMsg::Eof | ChannelMsg::Close => break,
            _ => {}
        }
    }
    if !pending.is_empty() {
        on_line(pending.trim_end().to_string());
    }
    write_half.close().await?;
    code.ok_or_else(|| anyhow!("command ended without an exit status"))
}

/// One PTY shell session, serialized through an action queue.
pub struct Term {
    #[allow(dead_code)]
    pub id: u32,
    tx: mpsc::UnboundedSender<TermAction>,
    _handle: Arc<client::Handle<ClientHandler>>,
}

enum TermAction {
    Write(Vec<u8>),
    Resize(u32, u32),
    Close,
}

/// Open a PTY + shell. Output is pushed via `on_out`; end-of-session via
/// `on_close`.
pub async fn open_pty<F, G>(
    handle: client::Handle<ClientHandler>,
    id: u32,
    cols: u32,
    rows: u32,
    on_out: F,
    on_close: G,
) -> Result<Term>
where
    F: Fn(&[u8]) + Send + 'static,
    G: Fn() + Send + 'static,
{
    let channel: ClientChannel = handle.channel_open_session().await?;
    let (mut read_half, write_half) = channel.split();
    write_half
        .request_pty(true, "xterm-256color", cols, rows, 0, 0, &[])
        .await?;
    write_half.request_shell(true).await?;

    // Reader: channel -> Flutter terminal.
    tokio::spawn(async move {
        while let Some(msg) = read_half.wait().await {
            match msg {
                ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => on_out(&data),
                ChannelMsg::Eof | ChannelMsg::Close => break,
                _ => {}
            }
        }
        on_close();
    });

    // Writer: action queue -> channel (strictly ordered).
    let (tx, mut rx) = mpsc::unbounded_channel();
    let write_half = Arc::new(write_half);
    tokio::spawn(async move {
        while let Some(action) = rx.recv().await {
            match action {
                TermAction::Write(bytes) => {
                    let _ = write_half.data_bytes(bytes).await;
                }
                TermAction::Resize(cols, rows) => {
                    let _ = write_half.window_change(cols, rows, 0, 0).await;
                }
                TermAction::Close => {
                    let _ = write_half.close().await;
                    break;
                }
            }
        }
    });

    Ok(Term {
        id,
        tx,
        _handle: Arc::new(handle),
    })
}

impl Term {
    pub fn write(&self, bytes: Vec<u8>) -> Result<()> {
        self.tx
            .send(TermAction::Write(bytes))
            .map_err(|_| anyhow!("terminal session closed"))
    }

    pub fn resize(&self, cols: u32, rows: u32) -> Result<()> {
        self.tx
            .send(TermAction::Resize(cols, rows))
            .map_err(|_| anyhow!("terminal session closed"))
    }

    pub fn close(&self) -> Result<()> {
        self.tx
            .send(TermAction::Close)
            .map_err(|_| anyhow!("terminal session closed"))
    }
}
