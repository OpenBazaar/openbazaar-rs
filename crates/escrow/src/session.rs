//! Funded escrow session.
//!
//! `fund` runs the buyer and vendor ceremony in one process. It samples the
//! moderator halves of the adaptor secrets, encrypts them, and drops the
//! plaintext. Those halves are not fields of [`Escrow`].

use bitcoin::absolute::{self, LOCK_TIME_THRESHOLD};
use bitcoin::hashes::Hash;
use bitcoin::{Amount, OutPoint, ScriptBuf, Transaction, TxOut};
use core::fmt;
use musig2::secp::{Point, Scalar};
use musig2::{AdaptorSignature, LiftedSignature, PartialSignature, PubNonce, SecNonce};

use crate::crypto::{self, point_from_compressed, EncryptedSecret, MacContext};
use crate::payload::{self, Claim};
use crate::tx::{self, funding_script};
use crate::{Error, Party, Phase};

/// Inputs to the funding ceremony.
///
/// Secrets are 32-byte secp256k1 scalars. The moderator key is compressed
/// (33 bytes). The moderator secret is not accepted here.
pub struct FundParams {
    pub buyer_secret: [u8; 32],
    pub vendor_secret: [u8; 32],
    pub moderator_public: [u8; 33],
    pub funding_outpoint: OutPoint,
    pub funding_value: Amount,
    pub fee: Amount,
    /// Absolute block height stored in the refund's `nLockTime`.
    ///
    /// Must be in `1..500_000_000`. Height 0 would disable the lock. The
    /// refund becomes final at `timeout_height + 1`.
    pub timeout_height: u32,
    pub buyer_payout: ScriptBuf,
    pub vendor_payout: ScriptBuf,
}

#[derive(Clone)]
struct Branch {
    tx: Transaction,
    sighash: [u8; 32],
    adaptor_point: Point,
    adaptor_sig: AdaptorSignature,
    encrypted: EncryptedSecret,
}

/// A funded 2-of-2 taproot output plus the presigned refund and dispute letters.
#[derive(Clone)]
pub struct Escrow {
    phase: Phase,
    buyer_point: Point,
    vendor_point: Point,
    key_agg: musig2::KeyAggContext,
    output_key: [u8; 32],
    funding_outpoint: OutPoint,
    funding_output: TxOut,
    timeout_height: u32,
    release_tx: Transaction,
    release_sighash: [u8; 32],
    refund_tx: Transaction,
    buyer_branch: Branch,
    vendor_branch: Branch,
}

/// First round of a happy-path MuSig2 release.
pub struct ReleaseNonce {
    party: Party,
    secnonce: SecNonce,
    pub nonce: PubNonce,
}

/// A happy-path partial signature and the nonce that produced it.
#[derive(Clone)]
pub struct Contribution {
    pub party: Party,
    pub nonce: PubNonce,
    partial: PartialSignature,
}

impl Contribution {
    pub fn partial_bytes(&self) -> [u8; 32] {
        crypto::encode_partial(&self.partial)
    }

    pub fn set_partial_bytes(&mut self, bytes: [u8; 32]) -> Result<(), Error> {
        self.partial = crypto::decode_partial(&bytes)?;
        Ok(())
    }
}

/// The moderator's decrypted adaptor half.
///
/// This is not a Bitcoin signature. Adapting the dispute spend also needs the
/// favored party's half, which is derived from that party's secret key.
pub struct ModeratorShare {
    favored: Party,
    outpoint: OutPoint,
    secret: Scalar,
}

impl fmt::Debug for ModeratorShare {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModeratorShare")
            .field("favored", &self.favored)
            .field("outpoint", &self.outpoint)
            .finish_non_exhaustive()
    }
}

/// Open a moderated escrow. Both commercial keys are required. The moderator is not.
pub fn fund(params: FundParams) -> Result<Escrow, Error> {
    if params.timeout_height == 0 || params.timeout_height >= LOCK_TIME_THRESHOLD {
        return Err(Error::InvalidParams(
            "timeout height must be a non-zero block height",
        ));
    }
    if params.buyer_payout.is_empty() || params.vendor_payout.is_empty() {
        return Err(Error::InvalidParams("payout script is empty"));
    }
    let payout = params
        .funding_value
        .checked_sub(params.fee)
        .ok_or(Error::InvalidParams("fee exceeds the funding value"))?;
    if payout.to_sat() == 0 {
        return Err(Error::InvalidParams("payout is zero"));
    }

    let buyer = scalar_key(&params.buyer_secret)?;
    let vendor = scalar_key(&params.vendor_secret)?;
    let buyer_point = buyer.base_point_mul();
    let vendor_point = vendor.base_point_mul();
    let moderator_point = point_from_compressed(&params.moderator_public)?;
    if moderator_point == buyer_point || moderator_point == vendor_point {
        return Err(Error::InvalidParams("moderator key must be distinct"));
    }

    let internal = crypto::internal_xonly(buyer_point, vendor_point)?;
    let key_agg = crypto::aggregate(buyer_point, vendor_point)?;
    let output_key = crypto::output_xonly(&key_agg);
    let script = funding_script(&internal, &output_key)?;
    let funding_output = TxOut {
        value: params.funding_value,
        script_pubkey: script,
    };

    let timeout = absolute::LockTime::from_height(params.timeout_height)
        .map_err(|_| Error::InvalidParams("timeout height"))?;
    let refund_template = tx::spend_template(
        params.funding_outpoint,
        timeout,
        params.buyer_payout.clone(),
        payout,
    );
    let buyer_template = tx::spend_template(
        params.funding_outpoint,
        absolute::LockTime::ZERO,
        params.buyer_payout,
        payout,
    );
    let vendor_template = tx::spend_template(
        params.funding_outpoint,
        absolute::LockTime::ZERO,
        params.vendor_payout,
        payout,
    );

    let refund_sighash = tx::key_spend_sighash(&refund_template, &funding_output)?;
    let buyer_sighash = tx::key_spend_sighash(&buyer_template, &funding_output)?;
    let vendor_sighash = tx::key_spend_sighash(&vendor_template, &funding_output)?;

    let refund_sig = crypto::cosign_plain(&key_agg, buyer, vendor, &refund_sighash, "refund")?;
    let refund_bytes = crypto::signature_bytes(&refund_sig);
    crypto::bip340_verify(&output_key, &refund_bytes, &refund_sighash)?;
    let refund_tx = tx::attach_key_spend(refund_template, &refund_bytes);

    let buyer_branch = dispute_branch(
        &key_agg,
        buyer,
        vendor,
        moderator_point,
        params.funding_outpoint,
        buyer_template,
        buyer_sighash,
        buyer,
        b"buyer-wins",
        "buyer-wins",
        Party::Buyer,
    )?;
    let vendor_branch = dispute_branch(
        &key_agg,
        buyer,
        vendor,
        moderator_point,
        params.funding_outpoint,
        vendor_template.clone(),
        vendor_sighash,
        vendor,
        b"vendor-wins",
        "vendor-wins",
        Party::Vendor,
    )?;

    Ok(Escrow {
        phase: Phase::Funded,
        buyer_point,
        vendor_point,
        key_agg,
        output_key,
        funding_outpoint: params.funding_outpoint,
        funding_output,
        timeout_height: params.timeout_height,
        release_tx: vendor_template,
        release_sighash: vendor_sighash,
        refund_tx,
        buyer_branch,
        vendor_branch,
    })
}

#[allow(clippy::too_many_arguments)]
fn dispute_branch(
    ctx: &musig2::KeyAggContext,
    buyer: Scalar,
    vendor: Scalar,
    moderator: Point,
    outpoint: OutPoint,
    tx: Transaction,
    sighash: [u8; 32],
    favored_key: Scalar,
    party_label: &[u8],
    nonce_label: &str,
    favored: Party,
) -> Result<Branch, Error> {
    let party_half = crypto::party_adaptor_secret(&favored_key, party_label, &sighash)?;
    let moderator_half = crypto::random_scalar();
    let adaptor_point = (party_half.base_point_mul() + moderator_half.base_point_mul())
        .not_inf()
        .map_err(|_| Error::BadSignature)?;
    let adaptor_sig =
        crypto::cosign_adaptor(ctx, buyer, vendor, &sighash, nonce_label, adaptor_point)?;
    let output = tx
        .output
        .first()
        .ok_or(Error::InvalidParams("missing payout"))?;
    let point_bytes = adaptor_point.serialize();
    let encrypted = EncryptedSecret::seal(
        moderator,
        moderator_half,
        crypto::random_scalar(),
        &MacContext {
            adaptor_point: &point_bytes,
            sighash: &sighash,
            txid: outpoint.txid.as_byte_array(),
            vout: outpoint.vout,
            lock_time: tx.lock_time.to_consensus_u32(),
            value: output.value.to_sat(),
            favored: favored as u8,
            script: output.script_pubkey.as_bytes(),
        },
    );
    Ok(Branch {
        tx,
        sighash,
        adaptor_point,
        adaptor_sig,
        encrypted,
    })
}

impl Escrow {
    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn funding_outpoint(&self) -> OutPoint {
        self.funding_outpoint
    }

    pub fn funding_output(&self) -> &TxOut {
        &self.funding_output
    }

    /// Tweaked MuSig2 x-only key. Every valid spend verifies under this key.
    pub fn output_key(&self) -> [u8; 32] {
        self.output_key
    }

    pub fn timeout_height(&self) -> u32 {
        self.timeout_height
    }

    pub fn release_message(&self) -> [u8; 32] {
        self.release_sighash
    }

    /// Fully signed refund. Consensus still rejects it until the timeout height has passed.
    pub fn refund_transaction(&self) -> &Transaction {
        &self.refund_tx
    }

    pub fn release_nonce(&self, secret: &[u8; 32]) -> Result<ReleaseNonce, Error> {
        let scalar = scalar_key(secret)?;
        let point = scalar.base_point_mul();
        let party = self.party_of(&point).ok_or(Error::WrongKey)?;
        let extra: &[u8] = match party {
            Party::Buyer => b"ob-escrow/release/buyer",
            Party::Vendor => b"ob-escrow/release/vendor",
        };
        let secnonce = crypto::fresh_nonce(scalar, &self.key_agg, &self.release_sighash, extra);
        let nonce = crypto::public_nonce(&secnonce);
        Ok(ReleaseNonce {
            party,
            secnonce,
            nonce,
        })
    }

    pub fn release_partial(
        &self,
        secret: &[u8; 32],
        nonce: ReleaseNonce,
        counterparty_nonce: &PubNonce,
    ) -> Result<Contribution, Error> {
        let scalar = scalar_key(secret)?;
        if self.party_of(&scalar.base_point_mul()) != Some(nonce.party) {
            return Err(Error::WrongKey);
        }
        let ours = nonce.nonce.clone();
        let (buyer_nonce, vendor_nonce) = match nonce.party {
            Party::Buyer => (&ours, counterparty_nonce),
            Party::Vendor => (counterparty_nonce, &ours),
        };
        let aggregated = crypto::aggregate_nonce(buyer_nonce, vendor_nonce);
        let partial = crypto::partial_sign(
            &self.key_agg,
            scalar,
            nonce.secnonce,
            &aggregated,
            &self.release_sighash,
            None,
        )?;
        crypto::verify_plain_partial(
            &self.key_agg,
            partial,
            &aggregated,
            scalar.base_point_mul(),
            &ours,
            &self.release_sighash,
        )?;
        Ok(Contribution {
            party: nonce.party,
            nonce: ours,
            partial,
        })
    }

    pub fn complete_release(
        &mut self,
        buyer: Contribution,
        vendor: Contribution,
    ) -> Result<Transaction, Error> {
        if buyer.party != Party::Buyer || vendor.party != Party::Vendor {
            return Err(Error::WrongKey);
        }
        let aggregated = crypto::aggregate_nonce(&buyer.nonce, &vendor.nonce);
        crypto::verify_plain_partial(
            &self.key_agg,
            buyer.partial,
            &aggregated,
            self.buyer_point,
            &buyer.nonce,
            &self.release_sighash,
        )?;
        crypto::verify_plain_partial(
            &self.key_agg,
            vendor.partial,
            &aggregated,
            self.vendor_point,
            &vendor.nonce,
            &self.release_sighash,
        )?;
        let signature = crypto::aggregate_plain(
            &self.key_agg,
            &aggregated,
            [buyer.partial, vendor.partial],
            &self.release_sighash,
        )?;
        let bytes = crypto::signature_bytes(&signature);
        let tx = tx::attach_key_spend(self.release_tx.clone(), &bytes);
        self.verify_key_spend(&tx)?;
        self.enter(Phase::Completed)?;
        Ok(tx)
    }

    /// Broadcast path for the presigned refund.
    ///
    /// Fails without changing phase when `chain_height` has not yet passed the timeout.
    pub fn refund(&mut self, chain_height: u32) -> Result<Transaction, Error> {
        tx::locktime_error(&self.refund_tx, chain_height)?;
        self.verify_key_spend(&self.refund_tx)?;
        self.enter(Phase::Refunded)?;
        Ok(self.refund_tx.clone())
    }

    /// Bytes the moderator needs: outpoint, claim, and the encrypted adaptor half.
    pub fn dispute_payload(&self, claim: &Claim) -> Result<Vec<u8>, Error> {
        let branch = self.branch(claim.favored);
        let letter = payload::letter_from_tx(
            self.funding_outpoint,
            claim.clone(),
            &branch.tx,
            branch.sighash,
            branch.adaptor_point.serialize(),
            branch.adaptor_sig.serialize(),
            branch.encrypted.clone(),
        )?;
        payload::build(&letter)
    }

    /// BIP341 key-spend check against the funding output. Does not apply `nLockTime`.
    pub fn verify_key_spend(&self, tx: &Transaction) -> Result<(), Error> {
        if tx.input.len() != 1 || tx.input[0].previous_output != self.funding_outpoint {
            return Err(Error::BadSignature);
        }
        if tx.output.len() != 1 {
            return Err(Error::BadSignature);
        }
        let signature = tx::witness_signature(tx)?;
        let sighash = tx::key_spend_sighash(tx, &self.funding_output)?;
        crypto::bip340_verify(&self.output_key, &signature, &sighash)?;
        let committed =
            tx::p2tr_output_key(&self.funding_output.script_pubkey).ok_or(Error::KeyMismatch)?;
        if committed != self.output_key {
            return Err(Error::KeyMismatch);
        }
        let pubkey: Point = self.key_agg.aggregated_pubkey();
        let lifted = LiftedSignature::from_bytes(&signature).map_err(|_| Error::BadSignature)?;
        musig2::verify_single(pubkey, lifted, sighash).map_err(|_| Error::BadSignature)?;
        Ok(())
    }

    fn branch(&self, party: Party) -> &Branch {
        match party {
            Party::Buyer => &self.buyer_branch,
            Party::Vendor => &self.vendor_branch,
        }
    }

    fn party_of(&self, point: &Point) -> Option<Party> {
        if point == &self.buyer_point {
            Some(Party::Buyer)
        } else if point == &self.vendor_point {
            Some(Party::Vendor)
        } else {
            None
        }
    }

    fn enter(&mut self, next: Phase) -> Result<(), Error> {
        let allowed = matches!(
            (self.phase, next),
            (Phase::Funded, Phase::Completed)
                | (Phase::Funded, Phase::Refunded)
                | (Phase::Funded, Phase::Disputed)
        );
        if !allowed {
            return Err(Error::InvalidTransition {
                from: self.phase,
                to: next,
            });
        }
        self.phase = next;
        Ok(())
    }

    /// Finish a dispute. `share` is the moderator half; `party_secret` is the favored party.
    pub fn settle_dispute(
        &mut self,
        payload: &[u8],
        share: &ModeratorShare,
        party_secret: &[u8; 32],
    ) -> Result<Transaction, Error> {
        let letter = payload::parse(payload)?;
        if letter.outpoint != self.funding_outpoint || share.outpoint != self.funding_outpoint {
            return Err(Error::BadPayload);
        }
        if letter.claim.favored != share.favored {
            return Err(Error::WrongKey);
        }
        let branch = self.branch(share.favored);
        if letter.sighash != branch.sighash
            || letter.lock_time != branch.tx.lock_time.to_consensus_u32()
            || letter.value != branch.tx.output[0].value
            || letter.script != branch.tx.output[0].script_pubkey
            || letter.adaptor_point != branch.adaptor_point.serialize()
            || letter.adaptor_signature != branch.adaptor_sig.serialize()
            || letter.encrypted.ciphertext != branch.encrypted.ciphertext
            || letter.encrypted.ephemeral_pk != branch.encrypted.ephemeral_pk
            || letter.encrypted.mac != branch.encrypted.mac
        {
            return Err(Error::BadPayload);
        }

        let party = scalar_key(party_secret)?;
        let party_point = party.base_point_mul();
        if self.party_of(&party_point) != Some(share.favored) {
            return Err(Error::WrongKey);
        }
        let label: &[u8] = match share.favored {
            Party::Buyer => b"buyer-wins",
            Party::Vendor => b"vendor-wins",
        };
        let party_half = crypto::party_adaptor_secret(&party, label, &branch.sighash)?;
        let full = crypto::combine_adaptor_halves(party_half, share.secret)?;
        if full.base_point_mul() != branch.adaptor_point {
            return Err(Error::BadSignature);
        }
        let lifted: LiftedSignature = branch.adaptor_sig.adapt(full).ok_or(Error::BadSignature)?;
        let bytes = crypto::signature_bytes(&lifted);
        crypto::bip340_verify(&self.output_key, &bytes, &branch.sighash)?;
        let tx = tx::attach_key_spend(branch.tx.clone(), &bytes);
        self.verify_key_spend(&tx)?;
        self.enter(Phase::Disputed)?;
        Ok(tx)
    }

    #[cfg(test)]
    pub(crate) fn adapt_for_test(&self, party: Party, secret: Scalar) -> Result<[u8; 64], Error> {
        let branch = self.branch(party);
        let lifted: LiftedSignature = branch
            .adaptor_sig
            .adapt(secret)
            .ok_or(Error::BadSignature)?;
        let bytes = crypto::signature_bytes(&lifted);
        crypto::bip340_verify(&self.output_key, &bytes, &branch.sighash)?;
        Ok(bytes)
    }
}

/// Decrypt the moderator half from a dispute letter.
///
/// The moderator does not need the escrow session. The letter is enough.
/// The returned share does not verify as a Schnorr signature.
pub fn moderator_open(
    payload: &[u8],
    moderator_secret: &[u8; 32],
) -> Result<ModeratorShare, Error> {
    let letter = payload::parse(payload)?;
    let secret_scalar = scalar_key(moderator_secret)?;
    let opened = letter.encrypted.open(secret_scalar, &letter.context())?;
    Ok(ModeratorShare {
        favored: letter.claim.favored,
        outpoint: letter.outpoint,
        secret: opened,
    })
}

fn scalar_key(bytes: &[u8; 32]) -> Result<Scalar, Error> {
    musig2::secp::Scalar::from_slice(bytes).map_err(|_| Error::WrongKey)
}

#[cfg(test)]
impl ModeratorShare {
    pub(crate) fn scalar(&self) -> Scalar {
        self.secret
    }
}
