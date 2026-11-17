//! Note-commitment frontiers and one witness per note.
//!
//! A lightwalletd `GetTreeState` is the frontier at a height, which does not
//! say which notes are ours. Notes that arrive in later blocks get a witness
//! by appending every commitment those blocks carry. A spend proves that
//! witness. The bytes are what the wallet file stores.

use incrementalmerkletree::frontier::CommitmentTree;
use incrementalmerkletree::witness::IncrementalWitness;
use orchard::note::ExtractedNoteCommitment;
use orchard::tree::MerkleHashOrchard;
use serde_json::{json, Value};
use subtle::CtOption;

use crate::Pool;

pub(crate) const DEPTH: u8 = 32;

pub(crate) type Node = MerkleHashOrchard;
pub(crate) type Tree = CommitmentTree<Node, DEPTH>;
type LiveWitness = IncrementalWitness<Node, DEPTH>;

/// A witness kept as bytes so the wallet file can store it and compare it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Witness {
    tree: Vec<u8>,
    filled: Vec<[u8; 32]>,
    cursor: Option<Vec<u8>>,
}

impl Witness {
    /// Witness the leaf most recently appended to `tree`.
    pub(crate) fn watching(tree: &Tree) -> Option<Self> {
        LiveWitness::from_tree(tree.clone()).map(|witness| Self::from_live(&witness))
    }

    pub(crate) fn append(&mut self, node: Node) -> Result<(), String> {
        let mut live = self.live()?;
        live.append(node).map_err(|_| "the note witness cannot take another commitment".to_string())?;
        *self = Self::from_live(&live);
        Ok(())
    }

    pub(crate) fn path(&self) -> Result<orchard::tree::MerklePath, String> {
        let path = self.live()?.path().ok_or("that note has no witness path")?;
        Ok(orchard::tree::MerklePath::from(path))
    }

    pub(crate) fn to_json(&self) -> Value {
        json!({
            "tree": hex::encode(&self.tree),
            "filled": self.filled.iter().map(hex::encode).collect::<Vec<_>>(),
            "cursor": self.cursor.as_ref().map(hex::encode),
        })
    }

    pub(crate) fn from_json(value: &Value) -> Option<Self> {
        let tree = hex_vec(&value["tree"])?;
        let filled = value["filled"].as_array()?.iter().map(|entry| hash32(entry.as_str()?)).collect::<Option<_>>()?;
        let cursor = match &value["cursor"] {
            Value::Null => None,
            other => Some(hex_vec(other)?),
        };
        Some(Self { tree, filled, cursor })
    }

    fn from_live(witness: &LiveWitness) -> Self {
        Self {
            tree: write_tree(witness.tree()),
            filled: witness.filled().iter().map(Node::to_bytes).collect(),
            cursor: witness.cursor().as_ref().map(write_tree),
        }
    }

    fn live(&self) -> Result<LiveWitness, String> {
        let filled = self.filled.iter().map(|bytes| node_from_bytes(bytes)).collect::<Option<Vec<_>>>().ok_or("a witness hash is not a field element")?;
        let cursor = match &self.cursor {
            Some(bytes) => Some(read_tree(bytes)?),
            None => None,
        };
        LiveWitness::from_parts(read_tree(&self.tree)?, filled, cursor).ok_or_else(|| "the note witness could not be read".to_string())
    }
}

pub(crate) fn read_tree(bytes: &[u8]) -> Result<Tree, String> {
    if bytes.is_empty() {
        return Ok(Tree::empty());
    }
    zcash_primitives::merkle_tree::read_commitment_tree(bytes).map_err(|_| "the note commitment tree could not be read".to_string())
}

pub(crate) fn write_tree(tree: &Tree) -> Vec<u8> {
    let mut bytes = Vec::new();
    zcash_primitives::merkle_tree::write_commitment_tree(tree, &mut bytes).expect("writing a commitment tree to memory");
    bytes
}

pub(crate) fn node_from_cmx(cmx: &[u8]) -> Option<Node> {
    let bytes: [u8; 32] = cmx.try_into().ok()?;
    let cmx = option(ExtractedNoteCommitment::from_bytes(&bytes))?;
    Some(Node::from_cmx(&cmx))
}

fn node_from_bytes(bytes: &[u8; 32]) -> Option<Node> {
    option(Node::from_bytes(bytes))
}

fn option<T>(value: CtOption<T>) -> Option<T> {
    bool::from(value.is_some()).then(|| value.unwrap())
}

fn hex_vec(value: &Value) -> Option<Vec<u8>> {
    hex::decode(value.as_str()?).ok()
}

fn hash32(text: &str) -> Option<[u8; 32]> {
    let mut out = [0u8; 32];
    hex::decode_to_slice(text, &mut out).ok()?;
    Some(out)
}

/// Which pool a compact-block action list belongs to. Orchard actions and
/// Ironwood actions are separate trees.
pub(crate) fn actions_of<'a>(tx: &'a crate::proto::CompactTx, pool: Pool) -> &'a [crate::proto::CompactOrchardAction] {
    match pool {
        Pool::Orchard => &tx.actions,
        Pool::Ironwood => &tx.ironwood_actions,
    }
}
