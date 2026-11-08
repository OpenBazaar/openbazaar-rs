//! A public moderator ruling and a seller's reputation bond.
//!
//! The ruling cites the digest of an existing `ModeratorRuling` v2 and adds
//! the store signing key and the listing id. It is not that ruling: the v2
//! bytes stay what an in-flight claim already verifies.
//!
//! The bond is a store-signed outpoint the store key alone can spend after
//! a CLTV or CSV lock. There is no second key and no slash. Mainnet is refused.

use bitcoin::opcodes::all::{OP_CHECKSIG, OP_CLTV, OP_CSV, OP_DROP};
use bitcoin::script::Builder;
use bitcoin::{PublicKey, ScriptBuf};

use crate::encode::{Reader, Writer};
use crate::public_text;
use crate::sign::PUBKEY_LEN;
use crate::signed::{Signed, Terms};
use crate::Error;

/// Lowest bond a new seller can post.
pub const MIN_BOND_SATS: u64 = 10_000;
/// Shortest lock, in blocks, on that bond.
pub const MIN_BOND_LOCK: u32 = 2_016;
/// Same refusal as mainnet escrow.
pub const MAINNET_BOND_REFUSAL: &str = "Moderated escrow is not available on mainnet. It is a testnet4 demo until the two-party escrow protocol is reviewed.";

pub const NETWORK_MAINNET: u8 = 0;
pub const NETWORK_TESTNET4: u8 = 1;

const CLTV: u8 = 0;
const CSV: u8 = 1;

/// Tagged hash of the unchanged ModeratorRuling v2 bytes.
pub fn ruling_digest(ruling_bytes: &[u8]) -> [u8; 32] {
    crate::sign::tagged_hash("ob-escrow/ruling-digest/v1", ruling_bytes)
}

/// What a buyer fetches. The moderator's signing key signs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicRuling {
    pub ruling_digest: [u8; 32],
    /// Store signing key, x-only. Not the per-order vendor key.
    pub store_key: [u8; PUBKEY_LEN],
    pub listing_id: [u8; 16],
    pub moderator_key: [u8; PUBKEY_LEN],
}

pub type SignedPublicRuling = Signed<PublicRuling>;

impl Terms for PublicRuling {
    const TAG: &'static str = "ob/public-ruling/v1";
    const MAGIC: [u8; 4] = *b"OBPR";
    const VERSION: u8 = 1;

    fn signer(&self) -> [u8; PUBKEY_LEN] {
        self.moderator_key
    }

    fn check(&self) -> Result<(), Error> {
        refuse_private(&self.ruling_digest)?;
        refuse_private(&self.store_key)?;
        refuse_private(&self.listing_id)?;
        refuse_private(&self.moderator_key)?;
        Ok(())
    }

    fn write(&self, writer: &mut Writer) -> Result<(), Error> {
        writer.bytes(&self.ruling_digest);
        writer.bytes(&self.store_key);
        writer.bytes(&self.listing_id);
        writer.bytes(&self.moderator_key);
        Ok(())
    }

    fn read(reader: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            ruling_digest: reader.array()?,
            store_key: reader.array()?,
            listing_id: reader.array()?,
            moderator_key: reader.array()?,
        })
    }
}

/// A name, a street, an email, a phone, coordinates, or a ship-to, if these
/// bytes are text. Random keys are not text and pass.
fn refuse_private(bytes: &[u8]) -> Result<(), Error> {
    let end = bytes.iter().rposition(|byte| *byte != 0).map(|index| index + 1).unwrap_or(0);
    let Ok(text) = std::str::from_utf8(&bytes[..end]) else {
        return Ok(());
    };
    if text.is_empty() {
        return Ok(());
    }
    if let Some(kind) = public_text::find(text) {
        return Err(Error::PrivateText { field: "public ruling", kind });
    }
    let lower = text.to_ascii_lowercase();
    if lower.contains("ship") || text.chars().any(|ch| ch.is_ascii_alphabetic()) {
        return Err(Error::BadTerms(
            "a public ruling carries no name, street, email, phone, coordinates, or ship-to",
        ));
    }
    Ok(())
}

/// Absolute height (CLTV) or relative blocks (CSV).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BondLock {
    Cltv(u32),
    Csv(u32),
}

impl BondLock {
    pub fn blocks(self) -> u32 {
        match self {
            BondLock::Cltv(blocks) | BondLock::Csv(blocks) => blocks,
        }
    }

    fn kind(self) -> u8 {
        match self {
            BondLock::Cltv(_) => CLTV,
            BondLock::Csv(_) => CSV,
        }
    }

    fn from_parts(kind: u8, blocks: u32) -> Result<Self, Error> {
        match kind {
            CLTV => Ok(BondLock::Cltv(blocks)),
            CSV => Ok(BondLock::Csv(blocks)),
            _ => Err(Error::BadTerms("a bond lock is CLTV or CSV")),
        }
    }
}

/// The witness script: the lock, then the store key, and nothing else.
pub fn bond_script(store_pubkey: &[u8; 33], lock: BondLock) -> Result<ScriptBuf, Error> {
    let key = PublicKey::from_slice(store_pubkey).map_err(|_| Error::BadKey)?;
    let opcode = match lock {
        BondLock::Cltv(_) => OP_CLTV,
        BondLock::Csv(_) => OP_CSV,
    };
    Ok(Builder::new()
        .push_int(i64::from(lock.blocks()))
        .push_opcode(opcode)
        .push_opcode(OP_DROP)
        .push_key(&key)
        .push_opcode(OP_CHECKSIG)
        .into_script())
}

/// A store-signed bond. The script spends only with the store key after the lock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bond {
    pub network: u8,
    /// Compressed store signing key. Its x-only form signs the record.
    pub store_pubkey: [u8; 33],
    pub txid: [u8; 32],
    pub vout: u32,
    pub amount_sats: u64,
    pub lock: BondLock,
    pub script: Vec<u8>,
}

pub type SignedBond = Signed<Bond>;

impl Bond {
    pub fn xonly(&self) -> Result<[u8; PUBKEY_LEN], Error> {
        let key = PublicKey::from_slice(&self.store_pubkey).map_err(|_| Error::BadKey)?;
        Ok(key.inner.x_only_public_key().0.serialize())
    }
}

impl Terms for Bond {
    const TAG: &'static str = "ob/bond/v1";
    const MAGIC: [u8; 4] = *b"OBBD";
    const VERSION: u8 = 1;

    fn signer(&self) -> [u8; PUBKEY_LEN] {
        self.xonly().unwrap_or([0; PUBKEY_LEN])
    }

    fn check(&self) -> Result<(), Error> {
        if self.network != NETWORK_TESTNET4 {
            return Err(Error::BadTerms(MAINNET_BOND_REFUSAL));
        }
        if self.amount_sats < MIN_BOND_SATS {
            return Err(Error::BadTerms("a reputation bond is at least 10000 sats"));
        }
        if self.lock.blocks() < MIN_BOND_LOCK {
            return Err(Error::BadTerms("a reputation bond locks for at least 2016 blocks"));
        }
        let want = bond_script(&self.store_pubkey, self.lock)?;
        if self.script != want.to_bytes() {
            return Err(Error::BadTerms(
                "a reputation bond is spent by the store key after the lock, with no second key",
            ));
        }
        Ok(())
    }

    fn write(&self, writer: &mut Writer) -> Result<(), Error> {
        writer.u8(self.network);
        writer.bytes(&self.store_pubkey);
        writer.bytes(&self.txid);
        writer.u32(self.vout);
        writer.u64(self.amount_sats);
        writer.u8(self.lock.kind());
        writer.u32(self.lock.blocks());
        writer.blob(&self.script)?;
        Ok(())
    }

    fn read(reader: &mut Reader<'_>) -> Result<Self, Error> {
        let network = reader.u8()?;
        let store_pubkey = reader.array()?;
        let txid = reader.array()?;
        let vout = reader.u32()?;
        let amount_sats = reader.u64()?;
        let lock = BondLock::from_parts(reader.u8()?, reader.u32()?)?;
        let script = reader.blob()?;
        Ok(Self { network, store_pubkey, txid, vout, amount_sats, lock, script })
    }
}

/// The buyer checks the signature, the script, the minimums, and the outpoint.
pub fn accept_bond(bytes: &[u8], txid: &[u8; 32], vout: u32) -> Result<Bond, Error> {
    let signed = SignedBond::decode(bytes)?;
    signed.verify()?;
    if signed.terms.txid != *txid || signed.terms.vout != vout {
        return Err(Error::BadTerms("the bond outpoint is not the one in the signed record"));
    }
    Ok(signed.terms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sign;

    fn key(label: &str) -> (bitcoin::secp256k1::SecretKey, [u8; 33]) {
        let secret = sign::secret_from_label(label);
        let public = bitcoin::secp256k1::PublicKey::from_secret_key(&bitcoin::secp256k1::Secp256k1::new(), &secret);
        (secret, public.serialize())
    }

    fn bond(amount: u64, lock: BondLock, network: u8, script: Option<Vec<u8>>) -> Result<SignedBond, Error> {
        let (secret, store_pubkey) = key("bond-store");
        let script = match script {
            Some(script) => script,
            None => bond_script(&store_pubkey, lock)?.to_bytes(),
        };
        Signed::sign(
            Bond {
                network,
                store_pubkey,
                txid: [0xab; 32],
                vout: 1,
                amount_sats: amount,
                lock,
                script,
            },
            &secret,
        )
    }

    #[test]
    fn a_public_ruling_that_carries_a_name_or_address_is_refused() {
        let (secret, _) = key("moderator");
        let moderator_key = sign::xonly(&secret);
        let mut street = [0u8; 16];
        street[..15].copy_from_slice(b"12 Cedar Street");
        let refused = Signed::sign(
            PublicRuling {
                ruling_digest: [1; 32],
                store_key: [2; 32],
                listing_id: street,
                moderator_key,
            },
            &secret,
        );
        assert!(refused.is_err(), "a street in the record is refused");
        let mut name = [0u8; 16];
        name[..5].copy_from_slice(b"Alice");
        let refused = Signed::sign(
            PublicRuling {
                ruling_digest: [3; 32],
                store_key: [4; 32],
                listing_id: name,
                moderator_key,
            },
            &secret,
        );
        assert!(refused.is_err(), "a name in the record is refused");
    }

    #[test]
    fn a_short_lock_is_refused() {
        assert!(bond(MIN_BOND_SATS, BondLock::Cltv(MIN_BOND_LOCK - 1), NETWORK_TESTNET4, None).is_err());
    }

    #[test]
    fn a_small_bond_is_refused() {
        assert!(bond(MIN_BOND_SATS - 1, BondLock::Csv(MIN_BOND_LOCK), NETWORK_TESTNET4, None).is_err());
    }

    #[test]
    fn a_slash_key_is_refused() {
        let (_, store_pubkey) = key("bond-store");
        let (_, other) = key("slash");
        let mut script = bond_script(&store_pubkey, BondLock::Cltv(MIN_BOND_LOCK)).unwrap().to_bytes();
        script.extend(bond_script(&other, BondLock::Cltv(MIN_BOND_LOCK)).unwrap().to_bytes());
        assert!(bond(MIN_BOND_SATS, BondLock::Cltv(MIN_BOND_LOCK), NETWORK_TESTNET4, Some(script)).is_err());
    }

    #[test]
    fn a_mainnet_bond_is_refused() {
        let refused = bond(MIN_BOND_SATS, BondLock::Cltv(MIN_BOND_LOCK), NETWORK_MAINNET, None);
        assert!(refused.is_err());
        let Err(Error::BadTerms(message)) = refused else {
            panic!("mainnet is refused");
        };
        assert_eq!(message, MAINNET_BOND_REFUSAL);
    }

    #[test]
    fn a_bond_script_that_is_not_the_store_key_is_refused() {
        let (_, other) = key("someone-else");
        let script = bond_script(&other, BondLock::Cltv(MIN_BOND_LOCK)).unwrap().to_bytes();
        assert!(bond(MIN_BOND_SATS, BondLock::Cltv(MIN_BOND_LOCK), NETWORK_TESTNET4, Some(script)).is_err());
    }

    #[test]
    fn a_passing_bond_is_accepted() {
        let signed = bond(MIN_BOND_SATS, BondLock::Cltv(MIN_BOND_LOCK), NETWORK_TESTNET4, None).unwrap();
        let bytes = signed.encode().unwrap();
        let accepted = accept_bond(&bytes, &[0xab; 32], 1).unwrap();
        assert_eq!(accepted.amount_sats, MIN_BOND_SATS);
        assert_eq!(accepted.lock, BondLock::Cltv(MIN_BOND_LOCK));
        assert!(accept_bond(&bytes, &[0xac; 32], 1).is_err(), "a different outpoint is not the signed one");
        let csv = bond(20_000, BondLock::Csv(MIN_BOND_LOCK), NETWORK_TESTNET4, None).unwrap();
        assert!(accept_bond(&csv.encode().unwrap(), &[0xab; 32], 1).is_ok());
    }
}
