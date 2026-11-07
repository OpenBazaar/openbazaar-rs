/// Roles from one backup phrase.
///
/// The node derives each role on its own BIP 32 path (see `ob-node`'s
/// `seed` module). The scan role is watch-only on the home node and is not a spend key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyRole {
    /// Signs store and listing records. Does not spend.
    Store,
    /// Spends coins. Does not sign the catalog.
    Spend,
    /// BIP 352 scan key. Watch-only copy lives on the home node.
    Scan,
}
