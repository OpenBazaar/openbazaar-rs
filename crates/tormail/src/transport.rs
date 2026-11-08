use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tor_rtcompat::Runtime;

use crate::onion::{OnionAddress, ORDER_PORT};
use crate::seal::{SealedLetter, MAX_SEALED_LEN};
use crate::Error;

const FRAME_MAGIC: &[u8; 4] = b"OBML";
const FRAME_VERSION: u8 = 1;
/// Sent by the vendor on the same stream, after `Mailbox::accept`.
pub(crate) const STORED_ACK: u8 = 1;

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

    /// Same as [`Self::from_onion_address`], with Arti state kept in `state_dir`.
    ///
    /// Call this inside a running Tokio runtime. Nothing is dialed here.
    pub fn from_state_dir(state_dir: &std::path::Path, onion: impl AsRef<str>) -> Result<Self, Error> {
        Self::from_client(crate::client::open_client(state_dir)?, onion)
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
        // One byte, written by the vendor only after the letter is on disk.
        // A dropped circuit is an error: flush alone is not delivery.
        let mut ack = [0u8; 1];
        match stream.read(&mut ack).await {
            Ok(1) if ack[0] == STORED_ACK => {
                let _ = stream.close().await;
                Ok(())
            }
            Ok(_) => Err(Error::Transport("vendor did not acknowledge the stored letter")),
            Err(err) => Err(err.into()),
        }
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
    use crate::letter::SenderKey;
    use crate::reply::{DropId, ReplyDelivery, ReplyPath};
    use crate::seal::{
        open_order, open_reply, seal_order, seal_reply, ReplyKeypair, StoreKeypair, LETTER_BUCKETS, MAX_REPLY_LEN,
        REPLY_BUCKETS,
    };

    fn order_with(ship_to: &str, reply: &ReplyKeypair) -> Order {
        let mut options = BTreeMap::new();
        options.insert("size".into(), "m".into());
        Order::new(
            ListingHash([9u8; 32]),
            1,
            options,
            ship_to,
            PaymentRail::OnChain,
            ReplyPath {
                key: *reply.public_key(),
                onion: Some(OnionAddress::parse(TEST_ONION).unwrap()),
                drop: DropId::generate(),
            },
        )
        .unwrap()
    }

    fn sample() -> (StoreKeypair, ReplyKeypair, Order, SealedLetter) {
        let vendor = StoreKeypair::generate();
        let reply = ReplyKeypair::generate();
        let order = order_with("14 Harbor Lane", &reply);
        let sealed = seal_order(&order, vendor.public(), &SenderKey::generate()).unwrap();
        (vendor, reply, order, sealed)
    }

    #[test]
    fn messages_are_separate_from_orders_and_fill_one_frame() {
        use crate::seal::{open_message, seal_message, MAX_MESSAGE_LEN};
        let (vendor, _, _, order_letter) = sample();
        let from = SenderKey::generate();
        let body = vec![0x5a; MAX_MESSAGE_LEN];
        let sealed = seal_message(&body, vendor.public(), &from).unwrap();
        assert_eq!(sealed.to_bytes().len(), MAX_SEALED_LEN);
        let decoded = SealedLetter::from_bytes(&sealed.to_bytes()).unwrap();
        let opened = open_message(&decoded, vendor.secret()).unwrap();
        assert_eq!((opened.body, opened.sender), (body, from.id()));
        assert!(open_order(&sealed, vendor.secret()).is_err());
        assert!(open_message(&order_letter, vendor.secret()).is_err());
        assert!(open_message(&sealed, StoreKeypair::generate().secret()).is_err());
        assert!(seal_message(&vec![0; MAX_MESSAGE_LEN + 1], vendor.public(), &from).is_err());
        assert!(seal_message(&[], vendor.public(), &from).is_err());
    }

    #[test]
    fn roundtrip_seal_and_reply() {
        let (vendor, reply, order, sealed) = sample();
        let opened = open_order(&sealed, vendor.secret()).unwrap().body;
        assert_eq!(opened, order);
        assert_eq!(opened.reply.key, *reply.public_key());

        let vendor_key = SenderKey::generate();
        let body = b"paid, shipping friday";
        let sealed_reply = seal_reply(body, &opened.reply.key, &vendor_key).unwrap();
        let read = open_reply(&sealed_reply, reply.secret()).unwrap();
        assert_eq!((read.body.as_slice(), read.sender), (&body[..], vendor_key.id()));
        assert!(seal_reply(&vec![1; MAX_REPLY_LEN + 1], &opened.reply.key, &vendor_key).is_err());
    }

    #[test]
    fn reply_path_routes_a_reply_without_naming_the_buyer() {
        let (vendor, reply, _order, sealed) = sample();
        let path = open_order(&sealed, vendor.secret()).unwrap().body.reply;
        let sealed_reply = seal_reply(b"shipped", &path.key, &SenderKey::generate()).unwrap();
        let delivery = path.delivery(sealed_reply).unwrap();
        assert_eq!(delivery.onion.as_str(), TEST_ONION);
        let frame = delivery.to_frame();
        let (drop, received) = ReplyDelivery::from_frame(&frame).unwrap();
        assert_eq!(drop, path.drop);
        assert_eq!(open_reply(&received, reply.secret()).unwrap().body, b"shipped");
        assert!(ReplyDelivery::from_frame(&frame[..frame.len() - 1]).is_err());

        let pending = ReplyPath::sealed_only(path.key, path.drop);
        let reply_letter = seal_reply(b"shipped", &pending.key, &SenderKey::generate()).unwrap();
        assert!(pending.delivery(reply_letter).is_none(), "no onion yet: sealed, not delivered");
    }

    #[test]
    fn letters_are_padded_to_fixed_buckets() {
        let vendor = StoreKeypair::generate();
        let reply = ReplyKeypair::generate();
        let from = SenderKey::generate();
        let short = seal_order(&order_with("A", &reply), vendor.public(), &from).unwrap();
        let long = seal_order(&order_with(&"x".repeat(1000), &reply), vendor.public(), &from).unwrap();
        assert_eq!(short.to_bytes().len(), LETTER_BUCKETS[0]);
        assert_eq!(long.to_bytes().len(), LETTER_BUCKETS[0], "ship-to length does not show");
        let mut options = BTreeMap::new();
        for i in 0..16 {
            options.insert(format!("{i:064}"), "9".repeat(200));
        }
        let mut big = order_with("A", &reply);
        big.options = options;
        assert_eq!(seal_order(&big, vendor.public(), &from).unwrap().to_bytes().len(), LETTER_BUCKETS[1]);
        for body in [&b"ok"[..], &[7u8; 3000][..]] {
            let sealed = seal_reply(body, reply.public_key(), &from).unwrap();
            assert!(REPLY_BUCKETS.contains(&sealed.to_bytes().len()));
        }

        let mut odd = short.to_bytes();
        odd.extend_from_slice(&[0; 16]);
        let len_at = 4 + 1 + 32;
        let ct_len = (odd.len() - len_at - 4) as u32;
        odd[len_at..len_at + 4].copy_from_slice(&ct_len.to_le_bytes());
        assert!(SealedLetter::from_bytes(&odd).is_err(), "a size off the buckets is refused");
        let mut v1 = short.to_bytes();
        v1[4] = 1;
        assert!(SealedLetter::from_bytes(&v1).is_err(), "unauthenticated v1 letters are refused");
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
        let mut flipped = sealed.to_bytes();
        flipped[100] ^= 1;
        assert!(open_order(&SealedLetter::from_bytes(&flipped).unwrap(), vendor.secret()).is_err());

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
        assert_eq!(open_order(&inbox[0], vendor.secret()).unwrap().body, order);
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
