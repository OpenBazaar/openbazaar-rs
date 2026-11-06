use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tor_rtcompat::Runtime;

use crate::onion::{OnionAddress, ORDER_PORT};
use crate::seal::{SealedLetter, MAX_SEALED_LEN};
use crate::Error;

const FRAME_MAGIC: &[u8; 4] = b"OBML";
const FRAME_VERSION: u8 = 1;

/// Carries a sealed letter to one vendor.
///
/// The memory fake is what tests use. [`ArtiOrderTransport`] is the real
/// carrier: it writes the same framed bytes onto an Arti onion stream.
#[async_trait]
pub trait OrderTransport: Send + Sync {
    async fn deliver(&self, letter: &SealedLetter) -> Result<(), Error>;
}

/// In-memory stand-in for the vendor mailbox.
///
/// `deliver` stores the sealed letter. It does not open a socket and it
/// does not decrypt.
#[derive(Clone, Default)]
pub struct MemoryTransport {
    inbox: Arc<Mutex<Vec<SealedLetter>>>,
}

impl MemoryTransport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn inbox(&self) -> Result<Vec<SealedLetter>, Error> {
        self.inbox
            .lock()
            .map(|guard| guard.clone())
            .map_err(|_| Error::Transport("memory transport lock poisoned"))
    }
}

#[async_trait]
impl OrderTransport for MemoryTransport {
    async fn deliver(&self, letter: &SealedLetter) -> Result<(), Error> {
        self.inbox
            .lock()
            .map_err(|_| Error::Transport("memory transport lock poisoned"))?
            .push(letter.clone());
        Ok(())
    }
}

/// Arti client aimed at one vendor onion.
///
/// Constructing this does not open a circuit. [`OrderTransport::deliver`]
/// bootstraps (if the client has not already) and connects to [`ORDER_PORT`].
pub struct ArtiOrderTransport<R: Runtime> {
    client: Arc<arti_client::TorClient<R>>,
    onion: OnionAddress,
}

impl ArtiOrderTransport<tor_rtcompat::PreferredRuntime> {
    /// Build an unbootstrapped Arti client aimed at `onion`.
    ///
    /// Call this inside a running Tokio runtime. Arti's client builder uses
    /// the current runtime and will panic without one. No directory fetch
    /// and no circuit is started here.
    pub fn from_onion_address(onion: impl AsRef<str>) -> Result<Self, Error> {
        let onion = OnionAddress::parse(onion.as_ref())?;
        let client = arti_client::TorClient::builder()
            .bootstrap_behavior(arti_client::BootstrapBehavior::OnDemand)
            .create_unbootstrapped()
            .map_err(|err| Error::Arti(err.to_string()))?;
        Ok(Self { client, onion })
    }
}

impl<R: Runtime> ArtiOrderTransport<R> {
    /// Aim an existing Arti client at `onion` without dialing.
    pub fn from_client(
        client: Arc<arti_client::TorClient<R>>,
        onion: impl AsRef<str>,
    ) -> Result<Self, Error> {
        Ok(Self {
            client,
            onion: OnionAddress::parse(onion.as_ref())?,
        })
    }

    pub fn onion(&self) -> &OnionAddress {
        &self.onion
    }

    pub fn port(&self) -> u16 {
        ORDER_PORT
    }
}

#[async_trait]
impl<R: Runtime + Send + Sync> OrderTransport for ArtiOrderTransport<R> {
    async fn deliver(&self, letter: &SealedLetter) -> Result<(), Error> {
        self.client
            .bootstrap()
            .await
            .map_err(|err| Error::Arti(err.to_string()))?;
        let target = format!("{}:{}", self.onion.as_str(), ORDER_PORT);
        let mut stream = self
            .client
            .connect(target)
            .await
            .map_err(|err| Error::Arti(err.to_string()))?;
        write_letter(&mut stream, letter).await?;
        stream
            .flush()
            .await
            .map_err(|err| Error::Arti(err.to_string()))?;
        Ok(())
    }
}

pub(crate) async fn write_letter<W>(writer: &mut W, letter: &SealedLetter) -> Result<(), Error>
where
    W: AsyncWrite + Unpin,
{
    let payload = letter.to_bytes();
    if payload.len() > MAX_SEALED_LEN {
        return Err(Error::Malformed);
    }
    let len = u32::try_from(payload.len()).map_err(|_| Error::Malformed)?;
    writer.write_all(FRAME_MAGIC).await?;
    writer.write_all(&[FRAME_VERSION]).await?;
    writer.write_all(&len.to_le_bytes()).await?;
    writer.write_all(&payload).await?;
    Ok(())
}

pub(crate) async fn read_letter<R>(reader: &mut R) -> Result<SealedLetter, Error>
where
    R: AsyncRead + Unpin,
{
    let mut magic = [0u8; 4];
    read_exact(reader, &mut magic).await?;
    if &magic != FRAME_MAGIC {
        return Err(Error::Malformed);
    }
    let mut version = [0u8; 1];
    read_exact(reader, &mut version).await?;
    if version[0] != FRAME_VERSION {
        return Err(Error::Malformed);
    }
    let mut len_bytes = [0u8; 4];
    read_exact(reader, &mut len_bytes).await?;
    let len = u32::from_le_bytes(len_bytes) as usize;
    if len > MAX_SEALED_LEN {
        return Err(Error::Malformed);
    }
    let mut payload = vec![0u8; len];
    read_exact(reader, &mut payload).await?;
    SealedLetter::from_bytes(&payload)
}

async fn read_exact<R>(reader: &mut R, buf: &mut [u8]) -> Result<(), Error>
where
    R: AsyncRead + Unpin,
{
    match reader.read_exact(buf).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => Err(Error::Malformed),
        Err(err) => Err(err.into()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use futures::io::Cursor;

    use super::*;
    use crate::onion::TEST_ONION;
    use crate::order::{ListingHash, Order, PaymentRail};
    use crate::seal::{open_order, open_reply, seal_order, seal_reply, ReplyKeypair, StoreKeypair};

    fn sample() -> (StoreKeypair, ReplyKeypair, Order, SealedLetter) {
        let vendor = StoreKeypair::generate();
        let reply = ReplyKeypair::generate();
        let mut options = BTreeMap::new();
        options.insert("size".into(), "m".into());
        let order = Order::new(
            ListingHash([9u8; 32]),
            1,
            options,
            "14 Harbor Lane",
            PaymentRail::OnChain,
            *reply.public_key(),
        )
        .unwrap();
        let sealed = seal_order(&order, vendor.public()).unwrap();
        (vendor, reply, order, sealed)
    }

    #[test]
    fn roundtrip_seal_and_reply() {
        let (vendor, reply, order, sealed) = sample();
        let opened = open_order(&sealed, vendor.secret()).unwrap();
        assert_eq!(opened, order);
        assert_eq!(opened.reply_key, *reply.public_key());

        let body = b"paid, shipping friday";
        let sealed_reply = seal_reply(body, &opened.reply_key).unwrap();
        assert_eq!(open_reply(&sealed_reply, reply.secret()).unwrap(), body);
    }

    #[test]
    fn wrong_key_cannot_open() {
        let (vendor, _reply, _order, sealed) = sample();
        let other = StoreKeypair::generate();
        assert_ne!(vendor.public(), other.public());
        assert!(open_order(&sealed, other.secret()).is_err());
    }

    #[test]
    fn truncated_ciphertext_fails() {
        let (vendor, _reply, _order, sealed) = sample();
        let mut short = sealed.to_bytes();
        short.pop();
        assert!(SealedLetter::from_bytes(&short).is_err());

        let mut ciphertext = sealed.ciphertext().to_vec();
        ciphertext.pop();
        let truncated = SealedLetter::from_parts(*sealed.encap_key(), ciphertext).unwrap();
        assert!(open_order(&truncated, vendor.secret()).is_err());
    }

    #[test]
    fn memory_transport_delivers_sealed_letter() {
        let (vendor, _reply, order, sealed) = sample();
        let transport = MemoryTransport::new();
        futures::executor::block_on(transport.deliver(&sealed)).unwrap();
        let inbox = transport.inbox().unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(open_order(&inbox[0], vendor.secret()).unwrap(), order);
    }

    #[test]
    fn truncated_frame_fails() {
        let (_vendor, _reply, _order, sealed) = sample();
        let bytes = futures::executor::block_on(async {
            let mut buf = Vec::new();
            write_letter(&mut buf, &sealed).await.unwrap();
            buf
        });
        let err =
            futures::executor::block_on(read_letter(&mut Cursor::new(&bytes[..bytes.len() - 1])))
                .unwrap_err();
        assert!(matches!(err, Error::Malformed));
    }

    #[test]
    fn arti_destination_is_the_parsed_onion() {
        // Parsing is the offline half of constructing an Arti transport.
        // Building the Tor client itself is the ignored test below.
        let onion = OnionAddress::parse(TEST_ONION).unwrap();
        assert_eq!(onion.as_str(), TEST_ONION);
        assert_eq!(ORDER_PORT, 80);
    }

    #[tokio::test]
    #[ignore = "bootstraps Arti and dials a non-routed test onion"]
    async fn arti_transport_dials_over_tor() {
        let transport = ArtiOrderTransport::from_onion_address(TEST_ONION).unwrap();
        assert_eq!(transport.onion().as_str(), TEST_ONION);
        assert_eq!(transport.port(), ORDER_PORT);
        let (_vendor, _reply, _order, sealed) = sample();
        // The test onion is an RFC 8032 key encoded as an address, not a
        // live shop. A network failure is an acceptable result. CI does not
        // run this test.
        let _ = transport.deliver(&sealed).await;
    }
}
