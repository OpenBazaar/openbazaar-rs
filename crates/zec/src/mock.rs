//! An in-memory chain of compact blocks for offline tests.
//!
//! Payments are real notes: built for the recipient's Orchard-protocol
//! receiver, encrypted with Ironwood's note encryption (or Orchard's, before
//! NU6.3), and cut down to the compact form a lightwalletd or Zaino serves.
//! So the wallet's trial decryption, address matching, and nullifiers run as
//! they would against a live server. Nothing is proved; compact blocks carry
//! no proofs.
//!
//! The chain follows NU6.3's rule: a payment lands in Ironwood once it is
//! active and in Orchard only before, and an Orchard note can be spent but
//! receives nothing. [`MockChain::pay_in_pool`] breaks the rule on purpose,
//! for tests of a server that does not follow consensus.

use std::sync::Mutex;

use orchard::note::{ExtractedNoteCommitment, NoteVersion, RandomSeed, Rho};
use orchard::note_encryption::{IronwoodDomain, IronwoodNoteEncryption, OrchardDomain, OrchardNoteEncryption};
use orchard::value::NoteValue;
use orchard::{Address, Note};
use zcash_keys::address::UnifiedAddress;
use zcash_note_encryption::Domain;
use zcash_protocol::consensus::{BlockHeight, NetworkUpgrade, Parameters};

use crate::proto::{ChainMetadata, CompactBlock, CompactOrchardAction, CompactTx};
use crate::{BackendStatus, Pool, ZecChain, ZecNetwork};

/// Blocks before the first one a test mines, so the first sync has history to walk back over.
pub const HISTORY: u64 = 200;
/// For [`MockChain::before_ironwood`]: blocks left after the history until NU6.3 activates.
pub const BEFORE_IRONWOOD: u64 = 20;

struct State {
    blocks: Vec<CompactBlock>,
    /// Transactions waiting for the next block.
    pool: Vec<CompactTx>,
    offline: bool,
}

pub struct MockChain {
    network: ZecNetwork,
    /// The height of `blocks[0]`.
    start: u64,
    state: Mutex<State>,
}

fn random32() -> [u8; 32] {
    rand::random()
}

/// A random value that is a valid nullifier (and so a valid rho).
fn random_nullifier() -> [u8; 32] {
    loop {
        let mut bytes = random32();
        bytes[31] &= 0x3f;
        if Rho::from_bytes(&bytes).is_some().into() {
            return bytes;
        }
    }
}

fn activation(network: ZecNetwork, upgrade: NetworkUpgrade) -> u64 {
    u64::from(network.params().activation_height(upgrade).expect("the upgrade has an activation height"))
}

impl MockChain {
    /// A chain at NU7's activation height on `network`, well after Ironwood,
    /// with [`HISTORY`] empty blocks.
    pub fn new(network: ZecNetwork) -> Self {
        Self::starting_at(network, activation(network, NetworkUpgrade::Nu7))
    }

    /// A chain that reaches NU6.3 [`BEFORE_IRONWOOD`] blocks after its
    /// history, so a test can pay into Orchard and then cross into Ironwood.
    pub fn before_ironwood(network: ZecNetwork) -> Self {
        Self::starting_at(network, activation(network, NetworkUpgrade::Nu6_3) - BEFORE_IRONWOOD - HISTORY)
    }

    fn starting_at(network: ZecNetwork, start: u64) -> Self {
        let chain = Self { network, start, state: Mutex::new(State { blocks: Vec::new(), pool: Vec::new(), offline: false }) };
        chain.mine(HISTORY);
        chain
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|err| err.into_inner())
    }

    pub fn height(&self) -> u64 {
        self.start + self.state().blocks.len() as u64 - 1
    }

    /// Where a payment mined in the next block lands: Ironwood once NU6.3 is
    /// active, Orchard before.
    pub fn payment_pool(&self) -> Pool {
        let next = u32::try_from(self.height() + 1).expect("mock heights fit in u32");
        if self.network.params().is_nu_active(NetworkUpgrade::Nu6_3, BlockHeight::from_u32(next)) {
            Pool::Ironwood
        } else {
            Pool::Orchard
        }
    }

    /// Queue a payment of `zatoshi` to the Orchard-protocol receiver of
    /// `address` (a Unified Address), mined with the next block in the pool
    /// consensus puts it in. Returns the transaction id.
    pub fn pay(&self, address: &str, zatoshi: u64) -> [u8; 32] {
        self.pay_in_pool(address, zatoshi, self.payment_pool())
    }

    /// [`MockChain::pay`] into `pool` whatever the height, as a server that
    /// does not follow consensus might send.
    pub fn pay_in_pool(&self, address: &str, zatoshi: u64, pool: Pool) -> [u8; 32] {
        let ua = UnifiedAddress::decode(&self.network.params(), address).expect("a Unified Address on this network");
        let receiver = *ua.orchard().expect("an Orchard-protocol receiver");
        // A second action to someone else, as real bundles have at least two.
        let actions = vec![note_action(receiver, zatoshi, pool, random_nullifier()), note_action(stranger(), 1_000, pool, random_nullifier())];
        self.queue(actions, pool)
    }

    /// Queue a transaction that spends the note whose nullifier is
    /// `nullifier` out of `pool`. The action's own output is a zero-value
    /// dummy, so a spend out of Orchard puts no value back in.
    pub fn spend(&self, nullifier: [u8; 32], pool: Pool) -> [u8; 32] {
        self.queue(vec![note_action(stranger(), 0, pool, nullifier)], pool)
    }

    fn queue(&self, actions: Vec<CompactOrchardAction>, pool: Pool) -> [u8; 32] {
        let txid = random32();
        let mut tx = CompactTx { txid: txid.to_vec(), ..Default::default() };
        match pool {
            Pool::Orchard => tx.actions = actions,
            Pool::Ironwood => tx.ironwood_actions = actions,
        }
        self.state().pool.push(tx);
        txid
    }

    /// Mine `count` blocks; the first takes every queued transaction.
    pub fn mine(&self, count: u64) {
        let mut state = self.state();
        for _ in 0..count {
            let txs = std::mem::take(&mut state.pool);
            let block = next_block(self.start, &state.blocks, txs);
            state.blocks.push(block);
        }
    }

    /// Drop the newest `depth` blocks. Their transactions are gone, not requeued.
    pub fn reorg(&self, depth: u64) {
        let mut state = self.state();
        let keep = state.blocks.len().saturating_sub(depth as usize).max(1);
        state.blocks.truncate(keep);
    }

    pub fn set_offline(&self, offline: bool) {
        self.state().offline = offline;
    }

    fn online(&self, state: &State) -> Result<(), String> {
        if state.offline {
            Err("the Zcash server is offline".into())
        } else {
            Ok(())
        }
    }
}

/// The external address 0 of a fixed key that is not any test wallet's.
fn stranger() -> Address {
    (0u8..=255)
        .find_map(|byte| orchard::keys::SpendingKey::from_bytes([byte; 32]).into_option())
        .map(|key| orchard::keys::FullViewingKey::from(&key).address_at(0u32, orchard::keys::Scope::External))
        .expect("some fixed spending key is valid")
}

fn next_block(start: u64, blocks: &[CompactBlock], mut txs: Vec<CompactTx>) -> CompactBlock {
    let (height, prev_hash, metadata) = match blocks.last() {
        Some(last) => (last.height + 1, last.hash.clone(), last.chain_metadata.unwrap_or_default()),
        None => (start, vec![0u8; 32], ChainMetadata::default()),
    };
    for (index, tx) in txs.iter_mut().enumerate() {
        tx.index = index as u64;
    }
    let orchard: u32 = txs.iter().map(|tx| tx.actions.len() as u32).sum();
    let ironwood: u32 = txs.iter().map(|tx| tx.ironwood_actions.len() as u32).sum();
    CompactBlock {
        height,
        hash: random32().to_vec(),
        prev_hash,
        vtx: txs,
        chain_metadata: Some(ChainMetadata {
            sapling_commitment_tree_size: metadata.sapling_commitment_tree_size,
            orchard_commitment_tree_size: metadata.orchard_commitment_tree_size + orchard,
            ironwood_commitment_tree_size: metadata.ironwood_commitment_tree_size + ironwood,
        }),
        ..Default::default()
    }
}

/// One action creating a note of `zatoshi` for `receiver` in `pool`,
/// spending the note whose nullifier is `nf_old`, in compact form.
fn note_action(receiver: Address, zatoshi: u64, pool: Pool, nf_old: [u8; 32]) -> CompactOrchardAction {
    let rho = Rho::from_bytes(&nf_old).expect("a valid nullifier is a valid rho");
    let version = match pool {
        Pool::Orchard => NoteVersion::V2,
        Pool::Ironwood => NoteVersion::V3,
    };
    let note = loop {
        let Some(rseed) = RandomSeed::from_bytes(random32(), &rho).into_option() else { continue };
        if let Some(note) = Note::from_parts(receiver, NoteValue::from_raw(zatoshi), rho, rseed, version).into_option() {
            break note;
        }
    };
    let cmx = ExtractedNoteCommitment::from(note.commitment()).to_bytes();
    let (epk, ciphertext) = match pool {
        Pool::Orchard => {
            let encryptor = OrchardNoteEncryption::new(None, note, [0u8; 512]);
            (OrchardDomain::epk_bytes(encryptor.epk()).0, encryptor.encrypt_note_plaintext().as_ref()[..52].to_vec())
        }
        Pool::Ironwood => {
            let encryptor = IronwoodNoteEncryption::new(None, note, [0u8; 512]);
            (IronwoodDomain::epk_bytes(encryptor.epk()).0, encryptor.encrypt_note_plaintext().as_ref()[..52].to_vec())
        }
    };
    CompactOrchardAction { nullifier: nf_old.to_vec(), cmx: cmx.to_vec(), ephemeral_key: epk.to_vec(), ciphertext }
}

impl ZecChain for MockChain {
    fn status(&self) -> BackendStatus {
        BackendStatus { mock: true, tip: Some(self.height()), ..BackendStatus::off(self.network) }
    }

    fn tip(&self) -> Result<u64, String> {
        let state = self.state();
        self.online(&state)?;
        Ok(self.start + state.blocks.len() as u64 - 1)
    }

    fn block_hash(&self, height: u64) -> Result<[u8; 32], String> {
        let state = self.state();
        self.online(&state)?;
        let block = height.checked_sub(self.start).and_then(|i| state.blocks.get(i as usize)).ok_or("no such block")?;
        block.hash.as_slice().try_into().map_err(|_| "a mock hash is 32 bytes".to_string())
    }

    fn blocks(&self, from: u64, to: u64) -> Result<Vec<CompactBlock>, String> {
        let state = self.state();
        self.online(&state)?;
        let first = from.checked_sub(self.start).ok_or("before the mock chain")? as usize;
        let last = to.checked_sub(self.start).ok_or("before the mock chain")? as usize;
        if first > last || last >= state.blocks.len() {
            return Err("no such blocks".into());
        }
        Ok(state.blocks[first..=last].to_vec())
    }
}
