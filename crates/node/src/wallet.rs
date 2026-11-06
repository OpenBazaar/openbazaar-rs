//! A BDK descriptor wallet backed by one [`Seed`].
//!
//! The wallet's chain state (BDK change set) and the next escrow key index
//! are sealed in `state.obv` beside `seed.obv`. Both files are 0600. The
//! state holds public descriptors and transactions; private keys come from
//! the seed at load time and never touch the state file.

use std::fs;
use std::path::{Path, PathBuf};

use bdk_wallet::chain::Merge;
use bdk_wallet::descriptor::IntoWalletDescriptor;
use bdk_wallet::signer::SignersContainer;
use bdk_wallet::{Balance, ChangeSet, KeychainKind, SignOptions, Update, Wallet};
use bitcoin::{Address, OutPoint, Psbt};
use serde_json::{json, Value};

use crate::chain::WalletRequest;
use crate::network::NetworkMode;
use crate::seed::Seed;
use crate::vault::Vault;
use crate::Error;

pub(crate) struct NodeWallet {
    pub(crate) seed: Seed,
    pub(crate) wallet: Wallet,
    signers: [SignersContainer; 2],
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
        let seed = Seed::load_or_create(&dir.join("seed.obv"), vault, network)?;
        let (external, internal) = seed.wallet_descriptors();
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
        let signers = [
            signers_for(&wallet, KeychainKind::External, &external, network)?,
            signers_for(&wallet, KeychainKind::Internal, &internal, network)?,
        ];
        let mut changeset = changeset;
        if let Some(staged) = wallet.take_staged() {
            changeset.merge(staged);
        }
        let mut opened = Self {
            seed,
            wallet,
            signers,
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

    /// Sign every input this wallet owns. Returns `true` when the PSBT is final.
    pub(crate) fn sign(&self, psbt: &mut Psbt) -> Result<bool, Error> {
        let signers: Vec<&SignersContainer> = self.signers.iter().collect();
        self.wallet
            .sign_with_signers(psbt, &signers, SignOptions::default())
            .map_err(|err| Error::Detail(err.to_string()))
    }

    pub(crate) fn balance(&self) -> Balance {
        self.wallet.balance()
    }

    /// Unconfirmed coins someone else sent. They can still be replaced or
    /// double-spent, so payments do not spend them. Unconfirmed change from
    /// this wallet's own transactions stays spendable.
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
            "spendableSats": (balance.confirmed + balance.trusted_pending).to_sat(),
            "pendingSats": (balance.trusted_pending + balance.untrusted_pending).to_sat(),
            "immatureSats": balance.immature.to_sat(),
            "totalSats": balance.total().to_sat(),
        })
    }
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
