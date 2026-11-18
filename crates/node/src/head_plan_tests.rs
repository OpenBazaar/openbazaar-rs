use bitcoin::secp256k1::SecretKey;
use openbazaar_protocol::{secret_from_label, xonly, DirectoryTerms, SeedDocument, EPOCH_BLOCKS, FEE_SATS, NOTICE_BLOCKS, RELAX_NOTICE_BLOCKS};

use super::{sign_with, HeadPlan, TermsChange};

fn operator() -> SecretKey {
    secret_from_label("head-plan-operator")
}

fn first() -> Vec<u8> {
    sign_with(&HeadPlan { sequence: 1, ..Default::default() }, &operator()).unwrap().v3
}

fn next(previous: Vec<u8>, sequence: u64, tip: u32, append: Option<TermsChange>) -> Result<super::SignedHeads, String> {
    sign_with(&HeadPlan { sequence, previous: Some(previous), tip: Some(tip), append, ..Default::default() }, &operator())
}

fn raise(start: u32, sats: u64) -> Option<TermsChange> {
    Some(TermsChange { start_height: start, fee_sats: Some(sats), ..Default::default() })
}

#[test]
fn the_first_head_is_the_standard_terms_in_both_versions() {
    let heads = sign_with(&HeadPlan { sequence: 1, v2: true, ..Default::default() }, &operator()).unwrap();
    let v3 = SeedDocument::decode(&heads.v3).unwrap();
    let v2 = SeedDocument::decode(&heads.v2.unwrap()).unwrap();
    assert_eq!((v3.version, v2.version), (3, 2));
    assert_eq!(v3.schedule, v2.schedule);
    assert_eq!(v3.schedule.at(0).unwrap().terms, DirectoryTerms::standard(xonly(&operator())));
    assert_eq!(v3.introductions.len(), 2, "no list is the compiled onions");
}

#[test]
fn a_new_entry_needs_a_tip_and_notice_after_it() {
    let tip = 10_000;
    let err = sign_with(&HeadPlan { sequence: 2, previous: Some(first()), append: raise(20_000, 30_000), ..Default::default() }, &operator()).unwrap_err();
    assert!(err.contains("--tip"), "{err}");
    let err = next(first(), 2, tip, raise(tip, 30_000)).unwrap_err();
    assert!(err.contains(&format!("block {}", tip + NOTICE_BLOCKS)), "{err}");
    assert!(next(first(), 2, tip, raise(tip + NOTICE_BLOCKS - 1, 30_000)).is_err());
    let heads = next(first(), 2, tip, raise(tip + NOTICE_BLOCKS, 30_000)).unwrap();
    let document = SeedDocument::decode(&heads.v3).unwrap();
    let added = document.schedule.entries()[1];
    assert_eq!((added.start_height, added.announced_height, added.terms.fee_sats), (tip + NOTICE_BLOCKS, tip, 30_000));
    assert_eq!(added.terms.epoch_blocks, EPOCH_BLOCKS, "a value left out keeps the last entry's");
    assert!(heads.summary.last().unwrap().contains(&format!("before block {}", tip + 144)), "{:?}", heads.summary);
    assert!(next(first(), 2, tip, raise(tip + RELAX_NOTICE_BLOCKS, 10_000)).is_ok(), "asking less needs less notice");
    assert!(next(first(), 1, tip, None).unwrap_err().contains("above the previous"));
    assert!(next(heads.v3, 3, tip - 1, raise(tip + 5_000, 40_000)).unwrap_err().contains("below the previous head's announced"));
}

#[test]
fn a_head_holds_eight_entries_until_old_ones_are_pruned() {
    let mut bytes = first();
    let mut tip = 1_000;
    for sequence in 2..=8 {
        bytes = next(bytes, sequence, tip, Some(TermsChange { start_height: tip + RELAX_NOTICE_BLOCKS, ..Default::default() })).unwrap().v3;
        tip += 300;
    }
    assert_eq!(SeedDocument::decode(&bytes).unwrap().schedule.entries().len(), 8);
    let err = next(bytes.clone(), 9, tip, Some(TermsChange { start_height: tip + RELAX_NOTICE_BLOCKS, ..Default::default() })).unwrap_err();
    assert!(err.contains("--prune"), "{err}");
    let pruned = sign_with(
        &HeadPlan {
            sequence: 9,
            previous: Some(bytes),
            tip: Some(tip + EPOCH_BLOCKS + 144),
            prune: true,
            append: Some(TermsChange { start_height: tip + EPOCH_BLOCKS + 144 + RELAX_NOTICE_BLOCKS, ..Default::default() }),
            ..Default::default()
        },
        &operator(),
    )
    .unwrap();
    let schedule = SeedDocument::decode(&pruned.v3).unwrap().schedule;
    assert!(schedule.entries().len() < 8 && schedule.entries()[0].start_height > 0, "retired entries dropped, one added");
}

#[test]
fn pruning_keeps_a_skew_of_margin() {
    let raised = next(first(), 2, 1_000, raise(1_000 + NOTICE_BLOCKS, 30_000)).unwrap().v3;
    let retired = 1_000 + NOTICE_BLOCKS + EPOCH_BLOCKS;
    let prune = |tip: u32| {
        let heads = sign_with(&HeadPlan { sequence: 3, previous: Some(raised.clone()), tip: Some(tip), prune: true, ..Default::default() }, &operator()).unwrap();
        SeedDocument::decode(&heads.v3).unwrap().schedule.entries().len()
    };
    assert_eq!(prune(retired), 2, "a node a few blocks behind would still list a store under it");
    assert_eq!(prune(retired + 143), 2);
    assert_eq!(prune(retired + 144), 1);
}

#[test]
fn version_2_output_refuses_terms_that_would_hide_stores_from_old_nodes() {
    let plan = |append| HeadPlan { sequence: 2, previous: Some(first()), tip: Some(5_000), append, v2: true, ..Default::default() };
    let err = sign_with(&plan(raise(5_000 + RELAX_NOTICE_BLOCKS, FEE_SATS - 1)), &operator()).unwrap_err();
    assert!(err.contains("--v2-out"), "{err}");
    let longer = Some(TermsChange { start_height: 5_000 + RELAX_NOTICE_BLOCKS, epoch_blocks: Some(4_032), ..Default::default() });
    assert!(sign_with(&plan(longer), &operator()).is_err(), "a longer epoch lists stores old nodes already dropped");
    let heads = sign_with(&plan(raise(5_000 + NOTICE_BLOCKS, 30_000)), &operator()).unwrap();
    assert_eq!(SeedDocument::decode(&heads.v2.unwrap()).unwrap().schedule.at(0).unwrap().terms.fee_sats, FEE_SATS);
}

#[test]
fn append_terms_parses_and_refuses_a_fee_key() {
    assert_eq!(
        TermsChange::parse("start=125000,sats=30000,confs=2,epoch=4032").unwrap(),
        TermsChange { start_height: 125_000, fee_sats: Some(30_000), fee_confs: Some(2), epoch_blocks: Some(4_032) }
    );
    assert_eq!(TermsChange::parse("start=7").unwrap(), TermsChange { start_height: 7, ..Default::default() });
    assert!(TermsChange::parse("sats=1").unwrap_err().contains("start"));
    assert!(TermsChange::parse("start=1,fee-key=ab").unwrap_err().contains("fee key"));
    assert!(TermsChange::parse("start=x").is_err());
    assert!(TermsChange::parse("start=1,color=blue").is_err());
}
