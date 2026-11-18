use crate::sign::{secret_from_label, xonly};
use crate::terms::*;

fn standard() -> DirectoryTerms {
    DirectoryTerms::standard(xonly(&secret_from_label("terms-test")))
}

fn entry(start: u32, announced: u32, fee_sats: u64) -> TermsEntry {
    TermsEntry { start_height: start, announced_height: announced, terms: DirectoryTerms { fee_sats, ..standard() } }
}

fn schedule(entries: &[TermsEntry]) -> TermsSchedule {
    TermsSchedule::new(entries.to_vec()).unwrap()
}

fn view(tip: u32, last_current: u32) -> HeadView {
    HeadView { tip: Some(tip), last_current }
}

#[test]
fn the_entry_at_a_height_is_the_last_that_started() {
    let held = schedule(&[TermsEntry::genesis(standard()), entry(5_000, 3_000, 30_000), entry(9_000, 7_000, 40_000)]);
    assert_eq!(held.at(0).unwrap().terms.fee_sats, FEE_SATS);
    assert_eq!(held.at(4_999).unwrap().terms.fee_sats, FEE_SATS);
    assert_eq!(held.at(5_000).unwrap().terms.fee_sats, 30_000);
    assert_eq!(held.at(5_001).unwrap().terms.fee_sats, 30_000);
    assert_eq!(held.at(u32::MAX).unwrap().terms.fee_sats, 40_000);
    assert_eq!(held.after(4_999).unwrap().start_height, 5_000);
    assert_eq!(held.after(5_000).unwrap().start_height, 9_000);
    assert!(held.after(9_000).is_none());
    let late = schedule(&[entry(5_000, 3_000, 30_000)]);
    assert!(late.at(4_999).is_none(), "nothing judges a fee before the first entry");
}

#[test]
fn an_entry_is_prunable_once_its_last_admission_left_the_epoch() {
    let held = schedule(&[TermsEntry::genesis(standard()), entry(5_000, 3_000, 30_000), entry(9_000, 7_000, 40_000)]);
    assert_eq!(held.prunable(5_000 + EPOCH_BLOCKS - 1), 0);
    assert_eq!(held.prunable(5_000 + EPOCH_BLOCKS), 1);
    assert_eq!(held.prunable(9_000 + EPOCH_BLOCKS), 2);
    assert_eq!(held.prunable(u32::MAX), 2, "the last entry stays");
}

#[test]
fn the_notice_depends_on_whether_the_terms_ask_more() {
    let base = standard();
    assert_eq!(notice_blocks(&base, &DirectoryTerms { fee_sats: base.fee_sats + 1, ..base }), NOTICE_BLOCKS);
    assert_eq!(notice_blocks(&base, &DirectoryTerms { fee_confs: 2, ..base }), NOTICE_BLOCKS);
    assert_eq!(notice_blocks(&base, &DirectoryTerms { epoch_blocks: 1_000, ..base }), NOTICE_BLOCKS);
    assert_eq!(notice_blocks(&base, &DirectoryTerms { fee_key: xonly(&secret_from_label("other")), ..base }), NOTICE_BLOCKS);
    assert_eq!(notice_blocks(&base, &DirectoryTerms { fee_sats: 1_000, epoch_blocks: 4_032, ..base }), RELAX_NOTICE_BLOCKS);
}

#[test]
fn a_successor_keeps_every_held_entry_and_adds_at_the_end() {
    let genesis = TermsEntry::genesis(standard());
    let raise = entry(5_000, 3_000, 30_000);
    let held = schedule(&[genesis, raise]);
    let added = entry(9_000, 7_000, 40_000);
    let rewrites = |next: &[TermsEntry], at: HeadView| matches!(held.successor(&schedule(next), at), Err(Refusal::Rewrites(_)));

    assert_eq!(held.successor(&held, HeadView { tip: None, last_current: 0 }), Ok(()), "same terms need no chain");
    assert_eq!(held.successor(&schedule(&[genesis, raise, added]), view(7_000, 7_000)), Ok(()));
    assert_eq!(held.successor(&schedule(&[genesis, raise, added]), HeadView { tip: None, last_current: 7_100 }), Ok(()), "an added entry needs no tip");
    assert!(rewrites(&[genesis, raise, added], view(8_000, 7_145)), "announced before this node last saw the head current");
    assert_eq!(held.successor(&schedule(&[genesis, raise, added]), view(8_000, 7_144)), Ok(()), "inside the skew");
    assert!(rewrites(&[genesis, entry(5_000, 3_000, 31_000)], view(4_000, 4_000)), "an edited entry");
    assert!(rewrites(&[genesis, entry(4_000, 2_000, 30_000), raise], view(2_000, 2_000)), "an entry inserted before a held one");
    assert!(rewrites(&[genesis], view(4_000, 4_000)), "a held entry dropped from the end");
    assert!(rewrites(&[TermsEntry::genesis(DirectoryTerms { fee_sats: 1_000, ..standard() })], view(0, 0)), "entry 0 rewritten");
}

#[test]
fn a_successor_drops_only_entries_no_store_is_listed_under() {
    let genesis = TermsEntry::genesis(standard());
    let raise = entry(5_000, 3_000, 30_000);
    let added = entry(9_000, 7_000, 40_000);
    let held = schedule(&[genesis, raise]);
    let pruned = schedule(&[raise]);
    assert_eq!(held.successor(&pruned, view(5_000 + EPOCH_BLOCKS, 0)), Ok(()));
    assert!(matches!(held.successor(&pruned, view(5_000 + EPOCH_BLOCKS - 1, 0)), Err(Refusal::NotYet(_))));
    assert!(matches!(held.successor(&pruned, HeadView { tip: None, last_current: 0 }), Err(Refusal::NotYet(_))));
    assert_eq!(held.successor(&schedule(&[raise, added]), view(7_100, 7_000)), Ok(()), "drop and add in one head");
    let only_new = schedule(&[added]);
    assert!(matches!(held.successor(&only_new, view(7_100, 7_000)), Err(Refusal::NotYet(_))), "the held last entry retires only after the new one starts");
    assert_eq!(held.successor(&only_new, view(9_000 + EPOCH_BLOCKS, 7_000)), Ok(()));
    let short = schedule(&[TermsEntry { start_height: 9_000, announced_height: 8_500, ..added }]);
    assert!(matches!(held.successor(&short, view(20_000, 8_500)), Err(Refusal::Rewrites(_))), "notice is checked against the held last entry");
}

#[test]
fn a_single_schedule_extends_to_a_longer_one() {
    let single = TermsSchedule::single(standard());
    assert!(single.is_single());
    let next = schedule(&[TermsEntry::genesis(standard()), entry(5_000, 3_000, 30_000)]);
    assert_eq!(single.successor(&next, view(3_000, 3_000)), Ok(()));
    assert!(!next.is_single());
}
