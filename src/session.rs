//! Authenticated WIT sessions between independently released WASI components.
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2_current::Sha256;
use std::{
    fs,
    io::{self, Write},
    net::Ipv4Addr,
    path::Path,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
const STARTUP: Duration = Duration::from_secs(10);

#[derive(Deserialize, Serialize)]
struct Address {
    port: u16,
    key: [u8; 32],
}

fn random() -> [u8; 32] {
    wasip2::random::random::get_random_bytes(32)
        .try_into()
        .expect("32 random bytes")
}
fn proof(key: &[u8; 32], role: &[u8], client: &[u8; 32], server: &[u8; 32]) -> Hmac<Sha256> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC key");
    mac.update(role);
    mac.update(client);
    mac.update(server);
    mac
}
fn denied(_: hmac::digest::MacError) -> io::Error {
    io::ErrorKind::PermissionDenied.into()
}
async fn accept(socket: &mut TcpStream, key: &[u8; 32]) -> io::Result<()> {
    let mut client = [0; 32];
    socket.read_exact(&mut client).await?;
    let server = random();
    socket.write_all(&server).await?;
    socket
        .write_all(
            &proof(key, b"support", &client, &server)
                .finalize()
                .into_bytes(),
        )
        .await?;
    let mut tag = [0; 32];
    socket.read_exact(&mut tag).await?;
    proof(key, b"caller", &client, &server)
        .verify_slice(&tag)
        .map_err(denied)
}
pub(crate) async fn connect(dir: &Path) -> io::Result<TcpStream> {
    tokio::time::timeout(STARTUP, async {
        let path = dir.join("support.json");
        let address: Address = loop {
            match fs::read(&path) {
                Ok(data) => break serde_json::from_slice(&data)?,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    tokio::time::sleep(Duration::from_millis(10)).await
                }
                Err(e) => return Err(e),
            }
        };
        let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, address.port)).await?;
        let _ = socket.set_nodelay(true);
        let client = random();
        socket.write_all(&client).await?;
        let mut server = [0; 32];
        let mut tag = [0; 32];
        socket.read_exact(&mut server).await?;
        socket.read_exact(&mut tag).await?;
        proof(&address.key, b"support", &client, &server)
            .verify_slice(&tag)
            .map_err(denied)?;
        socket
            .write_all(
                &proof(&address.key, b"caller", &client, &server)
                    .finalize()
                    .into_bytes(),
            )
            .await?;
        Ok(socket)
    })
    .await?
}
pub(crate) async fn serve() -> anyhow::Result<()> {
    let root = crate::connection::home()?;
    let dir = std::env::var_os("MN_WASI_SUPPORT_DIR")
        .ok_or_else(|| io::Error::other("missing private session directory"))?;
    let dir = Path::new(&dir);
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let key = random();
    let address = Address {
        port: listener.local_addr()?.port(),
        key,
    };
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer(&mut tmp, &address)?;
    tmp.flush()?;
    tmp.persist(dir.join("support.json"))?;
    loop {
        let (mut socket, _) = listener.accept().await?;
        let _ = socket.set_nodelay(true);
        let root = root.clone();
        tokio::spawn(async move {
            if let Ok(Ok(())) = tokio::time::timeout(STARTUP, accept(&mut socket, &key)).await {
                let (rx, tx) = socket.into_split();
                let _ = crate::backend::serve_pipes(root, rx, tx).await;
            }
        });
    }
}
