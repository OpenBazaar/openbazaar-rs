//! Read a v6 transaction we just built, enough to broadcast it and to mine
//! it on the mock chain. The mock chain only needs each Ironwood action's
//! compact form: the spent nullifier, the new commitment, and the first 52
//! bytes of the encrypted note.

use std::io::Cursor;

use orchard::note_encryption::COMPACT_NOTE_SIZE;
use zcash_primitives::transaction::Transaction;
use zcash_protocol::consensus::BranchId;

use crate::proto::CompactOrchardAction;

/// The id compact blocks carry, and the Ironwood actions in bundle order.
pub(crate) fn ironwood_compact(raw: &[u8]) -> Result<([u8; 32], Vec<CompactOrchardAction>), String> {
    let tx = Transaction::read(Cursor::new(raw), BranchId::Nu6_3).map_err(|_| "that is not a Zcash transaction".to_string())?;
    let txid = *tx.txid().as_ref();
    let bundle = tx.ironwood_bundle().ok_or("the transaction has no Ironwood bundle")?;
    let mut actions = Vec::with_capacity(bundle.actions().len());
    for action in bundle.actions().iter() {
        let note = action.encrypted_note();
        actions.push(CompactOrchardAction {
            nullifier: action.nullifier().to_bytes().to_vec(),
            cmx: action.cmx().to_bytes().to_vec(),
            ephemeral_key: note.epk_bytes.to_vec(),
            ciphertext: note.enc_ciphertext.0[..COMPACT_NOTE_SIZE].to_vec(),
        });
    }
    Ok((txid, actions))
}

pub(crate) fn txid_of(raw: &[u8]) -> Result<[u8; 32], String> {
    Ok(ironwood_compact(raw)?.0)
}

pub(crate) fn expiry_of(raw: &[u8]) -> Result<u64, String> {
    let tx = Transaction::read(Cursor::new(raw), BranchId::Nu6_3).map_err(|_| "that is not a Zcash transaction".to_string())?;
    Ok(u64::from(u32::from(tx.expiry_height())))
}
