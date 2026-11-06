/// Roles from one backup phrase.
///
/// HD derivation is not implemented. A later wallet can put each role on its
/// own path. The scan role is watch-only on the home node and is not a spend key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyRole {
    /// Signs store and listing records. Does not spend.
    Store,
    /// Spends coins. Does not sign the catalog.
    Spend,
    /// BIP 352 scan key. Watch-only copy lives on the home node.
    Scan,
}
