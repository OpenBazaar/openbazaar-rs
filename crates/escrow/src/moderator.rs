//! The moderator: published record, keys, chain hook, ruling store.
//!
//! A moderator publishes a [`ModeratorRecord`]: a signing key for rulings, a
//! separate encryption key for shares, its payout script, its fee, and its
//! acceptance policy. Both parties embed that exact record in the signed
//! terms, so the moderator's fee, payout, and policy are fixed at funding.
//!
//! [`Moderator::open`] is the only way to produce a ruling. Before decrypting
//! it checks the letter, checks the funding output against the chain through
//! [`FundingChain`], and records the branch in a [`RulingStore`] so a second
//! letter for the other branch of the same escrow is refused.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use bitcoin::{Amount, OutPoint, Script, ScriptBuf, TxOut};
use musig2::secp::{Point, Scalar, G};

use crate::context::Context;
use crate::crypto::point_from_compressed;
use crate::letter::{DisputeCase, ModeratorRuling};
use crate::policy::{check_payout, Policy};
use crate::proofs::tagged;
use crate::wire::{Reader, Writer};
use crate::{Error, Party};

const RECORD_MAGIC: &[u8; 4] = b"OBMR";
const RECORD_VERSION: u8 = 1;

/// What a moderator publishes. Parties copy it verbatim into the terms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModeratorRecord {
    /// Verifies rulings.
    pub signing_key: [u8; 33],
    /// Receives verifiably encrypted shares. Distinct from `signing_key`.
    pub encryption_key: [u8; 33],
    pub payout: ScriptBuf,
    pub fee: Amount,
    pub policy: Policy,
}

impl ModeratorRecord {
    pub fn validate(&self) -> Result<(), Error> {
        let signing = point_from_compressed(&self.signing_key)?;
        let encryption = point_from_compressed(&self.encryption_key)?;
        if signing == encryption {
            return Err(Error::InvalidParams(
                "moderator signing and encryption keys must differ",
            ));
        }
        check_payout(&self.payout, self.fee)
    }

    pub(crate) fn signing_point(&self) -> Result<Point, Error> {
        point_from_compressed(&self.signing_key)
    }

    pub(crate) fn encryption_point(&self) -> Result<Point, Error> {
        point_from_compressed(&self.encryption_key)
    }

    pub(crate) fn write(&self, w: &mut Writer) {
        w.bytes(&self.signing_key);
        w.bytes(&self.encryption_key);
        w.script(&self.payout);
        w.amount(self.fee);
        self.policy.write(w);
    }

    pub(crate) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            signing_key: r.array()?,
            encryption_key: r.array()?,
            payout: r.script()?,
            fee: r.amount()?,
            policy: Policy::read(r)?,
        })
    }

    /// The record as a signed moderator profile carries it.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new(RECORD_MAGIC, RECORD_VERSION);
        self.write(&mut w);
        w.finish()
    }

    /// Read and validate a record written by [`Self::to_bytes`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Reader::new(bytes, RECORD_MAGIC, RECORD_VERSION)?;
        let record = Self::read(&mut r)?;
        r.finish()?;
        record.validate()?;
        Ok(record)
    }

    /// The moderator's id: the x-only form of the key that signs its rulings.
    pub fn id(&self) -> [u8; 32] {
        let mut id = [0u8; 32];
        id.copy_from_slice(&self.signing_key[1..]);
        id
    }
}

/// Moderator secrets, derived from one seed with distinct tags.
pub struct ModeratorKeys {
    signing: Scalar,
    decryption: Scalar,
}

impl core::fmt::Debug for ModeratorKeys {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ModeratorKeys").finish_non_exhaustive()
    }
}

impl ModeratorKeys {
    pub fn from_seed(seed: &[u8; 32]) -> Result<Self, Error> {
        let derive = |tag: &str| {
            Scalar::from_slice(&tagged(tag, &[seed])).map_err(|_| Error::InvalidParams("seed"))
        };
        Ok(Self {
            signing: derive("ob-escrow/moderator-signing/v1")?,
            decryption: derive("ob-escrow/moderator-decryption/v1")?,
        })
    }

    pub fn signing_public(&self) -> [u8; 33] {
        (self.signing * G).serialize()
    }

    pub fn encryption_public(&self) -> [u8; 33] {
        (self.decryption * G).serialize()
    }

    /// The record to publish for these keys.
    pub fn record(&self, payout: ScriptBuf, fee: Amount, policy: Policy) -> ModeratorRecord {
        ModeratorRecord {
            signing_key: self.signing_public(),
            encryption_key: self.encryption_public(),
            payout,
            fee,
            policy,
        }
    }

    pub(crate) fn signing(&self) -> &Scalar {
        &self.signing
    }

    pub(crate) fn decryption(&self) -> &Scalar {
        &self.decryption
    }
}

/// The moderator's view of the chain. Implementations must answer from a
/// node or index the moderator trusts, not from data in the letter.
///
/// `script` is the funding script the signed terms commit to, and
/// `not_before` the lowest height the funding can have confirmed at
/// (`refund_height` minus the policy's `max_refund_horizon`). A backend that
/// cannot look a transaction up by txid, such as compact block filters,
/// follows `script` from `not_before` and finds the output among the
/// transactions that pay it.
pub trait FundingChain {
    /// The confirmed output created at `outpoint`, if it exists on chain.
    /// [`Error::ChainNotReady`] means "ask again later", not "absent".
    fn funding_output(
        &self,
        outpoint: &OutPoint,
        script: &Script,
        not_before: u32,
    ) -> Result<Option<TxOut>, Error>;
}

impl FundingChain for HashMap<OutPoint, TxOut> {
    fn funding_output(&self, outpoint: &OutPoint, _: &Script, _: u32) -> Result<Option<TxOut>, Error> {
        Ok(self.get(outpoint).cloned())
    }
}

/// Which escrow a ruling belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RulingKey {
    pub terms_digest: [u8; 32],
    pub outpoint: OutPoint,
}

/// Durable record of which branch the moderator has ruled for.
pub trait RulingStore {
    /// Record `favored` for `key` unless a branch is already recorded.
    /// Returns the branch recorded after the call. Must be atomic.
    fn record_once(&mut self, key: RulingKey, favored: Party) -> Result<Party, Error>;
}

/// A [`RulingStore`] that forgets everything on drop. For tests and demos.
#[derive(Debug, Default)]
pub struct InMemoryRulingStore {
    rulings: HashMap<RulingKey, Party>,
}

impl RulingStore for InMemoryRulingStore {
    fn record_once(&mut self, key: RulingKey, favored: Party) -> Result<Party, Error> {
        Ok(*self.rulings.entry(key).or_insert(favored))
    }
}

const VERIFIED_CACHE_MAX: usize = 1024;

/// Hashes of letters whose proofs already verified. Share one between the
/// short-lived [`Moderator`] values a server builds per request.
#[derive(Debug, Default)]
pub struct LetterCache(Mutex<HashSet<[u8; 32]>>);

impl LetterCache {
    fn contains(&self, hash: &[u8; 32]) -> bool {
        self.0.lock().unwrap_or_else(|err| err.into_inner()).contains(hash)
    }

    fn insert(&self, hash: [u8; 32]) {
        let mut set = self.0.lock().unwrap_or_else(|err| err.into_inner());
        if set.len() >= VERIFIED_CACHE_MAX {
            set.clear();
        }
        set.insert(hash);
    }
}

/// Verify a letter written under `record` that arrived at `received_height`.
///
/// This needs no moderator secret, so a moderator whose keys are locked away
/// can still check the letters it receives. [`Moderator::review_gated`]
/// describes `gate` and `cache`.
pub fn review_letter(
    record: &ModeratorRecord,
    chain: &impl FundingChain,
    cache: &LetterCache,
    letter: &[u8],
    received_height: u32,
    gate: &mut dyn FnMut(&RulingKey, Party) -> Result<(), Error>,
) -> Result<DisputeCase, Error> {
    let ctx = Context {
        tip: received_height,
        moderator: record.clone(),
        policy: record.policy,
    };
    let hash = DisputeCase::letter_hash(letter);
    let cached = cache.contains(&hash);
    let case = DisputeCase::verify(letter, &ctx, chain, cached, gate)?;
    if !cached {
        cache.insert(hash);
    }
    Ok(case)
}

/// A moderator able to review letters and issue at most one ruling per escrow.
pub struct Moderator<C, S> {
    keys: ModeratorKeys,
    record: ModeratorRecord,
    chain: C,
    store: S,
    verified: Arc<LetterCache>,
}

impl<C: FundingChain, S: RulingStore> Moderator<C, S> {
    pub fn new(
        keys: ModeratorKeys,
        record: ModeratorRecord,
        chain: C,
        store: S,
    ) -> Result<Self, Error> {
        record.validate()?;
        if record.signing_key != keys.signing_public()
            || record.encryption_key != keys.encryption_public()
        {
            return Err(Error::WrongKey);
        }
        Ok(Self {
            keys,
            record,
            chain,
            store,
            verified: Arc::default(),
        })
    }

    /// Use `cache` for verified letters instead of this value's own.
    pub fn with_cache(mut self, cache: Arc<LetterCache>) -> Self {
        self.verified = cache;
        self
    }

    pub fn record(&self) -> &ModeratorRecord {
        &self.record
    }

    pub fn store(&self) -> &S {
        &self.store
    }

    pub fn chain(&self) -> &C {
        &self.chain
    }

    /// Verify a letter that arrived at `received_height`.
    ///
    /// The expensive proof checks are skipped for a letter whose exact bytes
    /// already passed them. The cheap checks (terms, deadline, chain) always run.
    pub fn review(&mut self, letter: &[u8], received_height: u32) -> Result<DisputeCase, Error> {
        self.review_gated(letter, received_height, &mut |_, _| Ok(()))
    }

    /// [`Self::review`], with `gate` called for a letter not yet verified,
    /// after the parse, terms, and claimant signature checks and before the
    /// chain lookup and the proof checks. A gate that returns an error (for
    /// example [`Error::RateLimited`]) stops the review there, so a rate
    /// limit costs the moderator neither a chain scan nor a proof check.
    pub fn review_gated(
        &mut self,
        letter: &[u8],
        received_height: u32,
        gate: &mut dyn FnMut(&RulingKey, Party) -> Result<(), Error>,
    ) -> Result<DisputeCase, Error> {
        review_letter(&self.record, &self.chain, &self.verified, letter, received_height, gate)
    }

    /// Review a letter and rule for its branch, unless the other branch of
    /// the same escrow was already ruled for.
    pub fn open(&mut self, letter: &[u8], received_height: u32) -> Result<ModeratorRuling, Error> {
        let case = self.review(letter, received_height)?;
        let key = case.ruling_key();
        let recorded = self.store.record_once(key, case.favored())?;
        if recorded != case.favored() {
            return Err(Error::AlreadyRuled);
        }
        case.rule(&self.keys)
    }
}
