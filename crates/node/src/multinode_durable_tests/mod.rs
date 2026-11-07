//! Durable escrow across restarts, retries, and the review's open items,
//! on the same three-node harness as [`crate::multinode_tests`].

mod disputes;
mod fees;
mod funding;

use crate::harness::{order, Market};

/// Run the mail rounds one node at a time, restarting every node before
/// each round unless `hold` says a node holds state that is only in memory
/// by design. Returns how many rounds ran.
fn deliver_restarting(market: &mut Market, hold: impl Fn(&Market) -> bool) -> usize {
    let mut rounds = 0;
    loop {
        if !hold(market) {
            market.restart_all();
        }
        if !market.deliver_one() {
            return rounds;
        }
        rounds += 1;
        assert!(rounds < 40, "the rounds do not settle");
    }
}

fn stage(market: &Market, id: &str) -> String {
    order(&market.buyer.node, id)["escrow"]["stage"].as_str().unwrap_or_default().to_string()
}
