//! Snapshots of one party's escrow state, for durable storage.
//!
//! A snapshot carries no secret key and no secret nonce. Restoring one needs
//! the party's per-order secret and a [`Context`]: the contract is rebuilt
//! from the signed terms and the outpoint (heights are not checked against
//! the tip again), the secret must match the party's key in the terms, the
//! party's adaptor half must be the one its secret derives, the adaptor
//! pre-signature must verify for that branch, and every stored partial or
//! presigned transaction must verify under the aggregate key. The
//! verifiable shares are not re-proved: they were proved before the state
//! existed, and a snapshot is only read back from the party's own sealed
//! storage.
//!
//! A snapshot is a copy. The caller must store a new snapshot after every
//! phase change, before it sends or broadcasts what that change produced,
//! and must never act on two restored copies of one escrow.

use bitcoin::Transaction;
use musig2::secp::{Point, Scalar, G};

use crate::context::Context;
use crate::contract::{self, Branch, Contract, Role};
use crate::crypto;
use crate::terms::SignedTerms;
use crate::tx::Spend;
use crate::vencrypt::VerifiableShare;
use crate::wire::{Reader, Writer};
use crate::{Error, Party, Phase};

pub(crate) const VERSION: u8 = 1;

pub(crate) fn write_phase(w: &mut Writer, phase: Phase) {
    w.u8(match phase {
        Phase::Funded => 1,
        Phase::ReleaseSigned => 2,
        Phase::Completed => 3,
        Phase::Refunded => 4,
        Phase::Claimed => 5,
        Phase::Disputed => 6,
    });
}

pub(crate) fn read_phase(r: &mut Reader<'_>) -> Result<Phase, Error> {
    Ok(match r.u8()? {
        1 => Phase::Funded,
        2 => Phase::ReleaseSigned,
        3 => Phase::Completed,
        4 => Phase::Refunded,
        5 => Phase::Claimed,
        6 => Phase::Disputed,
        _ => return Err(Error::Malformed),
    })
}

pub(crate) fn write_contract(w: &mut Writer, contract: &Contract) {
    w.var(&contract.signed.encode());
    w.outpoint(&contract.outpoint);
}

/// Rebuild the contract and check `secret` is `party`'s per-order key in it.
pub(crate) fn read_contract(
    r: &mut Reader<'_>,
    secret: &[u8; 32],
    party: Party,
    ctx: &Context,
) -> Result<(Contract, Scalar), Error> {
    let signed = SignedTerms::decode(r.var(usize::MAX)?)?;
    let outpoint = r.outpoint()?;
    let contract = Contract::new(signed, outpoint, ctx, Role::Restore)?;
    let secret = crate::funding::own_secret(&contract, secret, party)?;
    Ok((contract, secret))
}

pub(crate) fn write_branch(w: &mut Writer, branch: &Branch) {
    w.point(&branch.party_point);
    branch.share.write(w);
    w.adaptor(&branch.adaptor_sig);
}

/// Read the branch favoring `favored` and check it against our own secret.
pub(crate) fn read_branch(
    r: &mut Reader<'_>,
    contract: &Contract,
    secret: &Scalar,
    favored: Party,
) -> Result<Branch, Error> {
    let branch = Branch {
        favored,
        party_point: r.point()?,
        share: VerifiableShare::read(r)?,
        adaptor_sig: r.adaptor()?,
    };
    check_half(contract, secret, favored, &branch.party_point)?;
    crypto::verify_adaptor(
        &contract.key_agg,
        &branch.adaptor_sig,
        &contract.sighash(Spend::dispute(favored))?,
        branch.adaptor_point()?,
    )?;
    Ok(branch)
}

/// The favored party's adaptor half must be the one its secret derives.
pub(crate) fn check_half(
    contract: &Contract,
    secret: &Scalar,
    favored: Party,
    half: &Point,
) -> Result<(), Error> {
    if contract::party_half(secret, contract, favored)? * G != *half {
        return Err(Error::WrongKey);
    }
    Ok(())
}

/// A stored presigned spend must be exactly this contract's template, signed.
pub(crate) fn check_template(
    contract: &Contract,
    spend: Spend,
    signed: &Transaction,
) -> Result<(), Error> {
    let mut bare = signed.clone();
    for input in &mut bare.input {
        input.witness.clear();
    }
    if &bare != contract.tx(spend) {
        return Err(Error::BadPayload);
    }
    contract.verify_key_spend(signed)
}
