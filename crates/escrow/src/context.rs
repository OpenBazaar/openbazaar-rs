//! The one value every contract check is made against.

use crate::moderator::ModeratorRecord;
use crate::policy::Policy;

/// What a buyer, vendor, or moderator knows independently of the counterparty.
///
/// * `tip`: the current chain height. At funding it bounds the timeline; for
///   the moderator it is the height the letter arrived.
/// * `moderator`: the moderator's published record. Terms naming any other
///   key, payout script, fee, or policy are rejected.
/// * `policy`: this side's own floor. Terms must satisfy it and the
///   moderator's published policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context {
    pub tip: u32,
    pub moderator: ModeratorRecord,
    pub policy: Policy,
}
