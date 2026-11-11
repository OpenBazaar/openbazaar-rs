//! A BDK descriptor wallet over one wallet's public descriptors.
//!
//! The wallet's chain state (BDK change set) and the next escrow key index
//! are sealed in `state.obv` under the vault key. The wallet itself is
//! watch-only: it is opened from the receive tier ([`crate::wallet_keys`]),
//! so it can sync, show balances, and derive addresses while the node is
//! locked. [`NodeWallet::sign`] takes a [`Seed`] from the spend tier for one
//! signature and builds its signers then; no private key stays in the wallet.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use bdk_wallet::chain::{ChainPosition, Merge};
use bdk_wallet::descriptor::IntoWalletDescriptor;
use bdk_wallet::signer::SignersContainer;
use bdk_wallet::{Balance, ChangeSet, KeychainKind, SignOptions, Update, Wallet};
use bitcoin::{Address, OutPoint, Psbt};
use serde_json::{json, Value};

use crate::chain::WalletRequest;
use crate::network::NetworkMode;
use crate::seed::Seed;
use crate::vault::Vault;
use crate::wallet_keys::{self, WatchKeys};
use crate::Error;

pub(crate) struct NodeWallet {
    pub(crate) watch: WatchKeys,
    pub(crate) wallet: Wallet,
    /// `wallets/<name>/`: `watch.obv`, `spend.obv`, `state.obv`.
    pub(crate) dir: PathBuf,
    network: NetworkMode,
    changeset: ChangeSet,
    escrow_next: u32,
    state_path: PathBuf,
    vault: Vault,
    pub(crate) last_sync: Option<u64>,
    pub(crate) sync_error: Option<String>,
}

impl NodeWallet {
    pub(crate) fn open(dir: &Path, name: &str, vault: &Vault, network: NetworkMode) -> Result<Self, Error> {
        let dir = dir.join(name);
        fs::create_dir_all(&dir).map_err(|err| Error::Detail(err.to_string()))?;
        let watch = wallet_keys::open_watch(&dir, vault, network)?;
        let (external, internal) = (watch.external.clone(), watch.internal.clone());
        let state_path = dir.join("state.obv");
        let (changeset, escrow_next) = match vault.read_sealed(&state_path)? {
            Some(bytes) => decode_state(&bytes)?,
            None => (ChangeSet::default(), 0),
        };
        let loaded = Wallet::load()
            .descriptor(KeychainKind::External, Some(external.clone()))
            .descriptor(KeychainKind::Internal, Some(internal.clone()))
            .check_network(network.bitcoin())
            .load_wallet_no_persist(changeset.clone())
            .map_err(|err| Error::Detail(format!("wallet {name}: {err}")))?;
        let mut wallet = match loaded {
            Some(wallet) => wallet,
            None => Wallet::create(external.clone(), internal.clone())
                .network(network.bitcoin())
                .create_wallet_no_persist()
                .map_err(|err| Error::Detail(format!("wallet {name}: {err}")))?,
        };
        let mut changeset = changeset;
        if let Some(staged) = wallet.take_staged() {
            changeset.merge(staged);
        }
        let mut opened = Self {
            watch,
            wallet,
            dir,
            network,
            changeset,
            escrow_next,
            state_path,
            vault: vault.clone(),
            last_sync: None,
            sync_error: None,
        };
        opened.persist()?;
        Ok(opened)
    }

    /// Write staged wallet changes. Call after anything that reveals an address.
    pub(crate) fn persist(&mut self) -> Result<(), Error> {
        if let Some(staged) = self.wallet.take_staged() {
            self.changeset.merge(staged);
        }
        let value = json!({
            "changeset": self.changeset,
            "escrowNext": self.escrow_next,
        });
        let bytes = serde_json::to_vec(&value).map_err(|err| Error::Detail(err.to_string()))?;
        self.vault.write_sealed(&self.state_path, &bytes)
    }

    /// A receive address that has never been handed out before.
    pub(crate) fn fresh_address(&mut self) -> Result<Address, Error> {
        let info = self.wallet.reveal_next_address(KeychainKind::External);
        self.persist()?;
        Ok(info.address)
    }

    /// The first revealed receive address with no history, revealing one if needed.
    pub(crate) fn receive_address(&mut self) -> Result<Address, Error> {
        let info = self.wallet.next_unused_address(KeychainKind::External);
        self.persist()?;
        Ok(info.address)
    }

    /// The next escrow key index. Each one is used once.
    pub(crate) fn next_escrow_index(&mut self) -> Result<u32, Error> {
        let index = self.escrow_next;
        self.escrow_next = index
            .checked_add(1)
            .ok_or_else(|| Error::Detail("escrow keys are exhausted".to_string()))?;
        self.persist()?;
        Ok(index)
    }

    pub(crate) fn update_request(&self) -> WalletRequest {
        if self.wallet.latest_checkpoint().height() == 0 {
            WalletRequest::Full(self.wallet.start_full_scan().build())
        } else {
            WalletRequest::Sync(self.wallet.start_sync_with_revealed_spks().build())
        }
    }

    pub(crate) fn apply(&mut self, update: Update, now: u64) -> Result<(), Error> {
        self.wallet
            .apply_update(update)
            .map_err(|err| Error::Detail(err.to_string()))?;
        self.last_sync = Some(now);
        self.sync_error = None;
        self.persist()
    }

    /// Sign every input this wallet owns with keys from `seed` (the spend
    /// tier). Returns `true` when the PSBT is final.
    pub(crate) fn sign(&self, psbt: &mut Psbt, seed: &Seed) -> Result<bool, Error> {
        let (external, internal) = seed.wallet_descriptors();
        let signers = [
            signers_for(&self.wallet, KeychainKind::External, &external, self.network)?,
            signers_for(&self.wallet, KeychainKind::Internal, &internal, self.network)?,
        ];
        let signers: Vec<&SignersContainer> = signers.iter().collect();
        self.wallet
            .sign_with_signers(psbt, &signers, SignOptions::default())
            .map_err(|err| Error::Detail(err.to_string()))
    }

    pub(crate) fn balance(&self) -> Balance {
        self.wallet.balance()
    }

    /// Unconfirmed coins someone else sent. They can still be replaced or
    /// double-spent, so payments and the directory join fee do not spend them.
    /// Unconfirmed change from this wallet's own transactions stays spendable.
    pub(crate) fn untrusted_pending(&self) -> Vec<OutPoint> {
        self.wallet
            .list_unspent()
            .filter(|utxo| !utxo.chain_position.is_confirmed() && utxo.keychain == KeychainKind::External)
            .map(|utxo| utxo.outpoint)
            .collect()
    }

    pub(crate) fn balance_value(&self) -> Value {
        let balance = self.balance();
        json!({
            "confirmedSats": balance.confirmed.to_sat(),
            // Spendable is confirmed mature coins plus this wallet's own
            // unconfirmed change. Pending adds unconfirmed coins from other
            // people; those are not spendable.
            "spendableSats": (balance.confirmed + balance.trusted_pending).to_sat(),
            "pendingSats": (balance.trusted_pending + balance.untrusted_pending).to_sat(),
            "immatureSats": balance.immature.to_sat(),
            "totalSats": balance.total().to_sat(),
        })
    }

    /// Deposits and payments this wallet already knows. Built from the local
    /// chain state, so no address is sent to a public server.
    pub(crate) fn activity_value(&self, labels: &HashMap<String, String>) -> Value {
        let mut rows = Vec::new();
        for tx in self.wallet.transactions() {
            let (sent, received) = self.wallet.sent_and_received(&tx.tx_node.tx);
            let (sent, received) = (sent.to_sat(), received.to_sat());
            let (direction, sats) = if received > sent {
                ("in", received - sent)
            } else if sent > received {
                ("out", sent - received)
            } else {
                continue;
            };
            let txid = tx.tx_node.txid.to_string();
            let (confirmed, height, time) = match tx.chain_position {
                ChainPosition::Confirmed { anchor, .. } => {
                    (true, anchor.block_id.height, anchor.confirmation_time)
                }
                ChainPosition::Unconfirmed { last_seen, first_seen } => {
                    (false, 0, last_seen.or(first_seen).unwrap_or(0))
                }
            };
            let label = if direction == "in" {
                "Deposit".to_string()
            } else {
                labels.get(&txid).cloned().unwrap_or_else(|| "Payment".to_string())
            };
            rows.push(ActivityRow {
                confirmed,
                height,
                time,
                txid: txid.clone(),
                value: json!({
                    "txid": txid,
                    "direction": direction,
                    "sats": sats,
                    "confirmed": confirmed,
                    "height": confirmed.then_some(height),
                    "time": (time > 0).then_some(time),
                    "label": label,
                }),
            });
        }
        rows.sort_by(|left, right| {
            left.confirmed
                .cmp(&right.confirmed)
                .then(right.height.cmp(&left.height))
                .then(right.time.cmp(&left.time))
                .then(left.txid.cmp(&right.txid))
        });
        Value::Array(rows.into_iter().map(|row| row.value).collect())
    }
}

struct ActivityRow {
    confirmed: bool,
    height: u32,
    time: u64,
    txid: String,
    value: Value,
}

fn signers_for(
    wallet: &Wallet,
    keychain: KeychainKind,
    descriptor: &str,
    network: NetworkMode,
) -> Result<SignersContainer, Error> {
    let (_, keymap) = descriptor
        .into_wallet_descriptor(wallet.secp_ctx(), network.bitcoin().into())
        .map_err(|err| Error::Detail(err.to_string()))?;
    Ok(SignersContainer::build(
        keymap,
        wallet.public_descriptor(keychain),
        wallet.secp_ctx(),
    ))
}

fn decode_state(bytes: &[u8]) -> Result<(ChangeSet, u32), Error> {
    let bad = || Error::Detail("the wallet state could not be read".to_string());
    let value: Value = serde_json::from_slice(bytes).map_err(|_| bad())?;
    let changeset: ChangeSet =
        serde_json::from_value(value.get("changeset").cloned().ok_or_else(bad)?).map_err(|_| bad())?;
    let escrow_next = value
        .get("escrowNext")
        .and_then(Value::as_u64)
        .and_then(|next| u32::try_from(next).ok())
        .unwrap_or(0);
    Ok((changeset, escrow_next))
}
