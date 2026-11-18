use bitcoin::secp256k1::SecretKey;
use openbazaar_protocol::{secret_from_label, xonly, DirectoryTerms, SeedDocument, TermsEntry, TermsSchedule, EPOCH_BLOCKS, NOTICE_BLOCKS};

use super::Heads;
use crate::harness::TempDir;

fn operator() -> SecretKey {
    secret_from_label("directory-head-test")
}

fn key() -> [u8; 32] {
    xonly(&operator())
}

fn standard() -> DirectoryTerms {
    DirectoryTerms::standard(key())
}

fn onion() -> String {
    format!("{}.onion", "a".repeat(56))
}

fn entry(announced: u32, fee_sats: u64) -> TermsEntry {
    TermsEntry { start_height: announced + NOTICE_BLOCKS, announced_height: announced, terms: DirectoryTerms { fee_sats, ..standard() } }
}

fn head(sequence: u64, entries: &[TermsEntry]) -> SeedDocument {
    let mut all = vec![TermsEntry::genesis(standard())];
    all.extend_from_slice(entries);
    SeedDocument::sign(sequence, vec![onion()], TermsSchedule::new(all).unwrap(), None, &operator()).unwrap()
}

fn v2(sequence: u64) -> SeedDocument {
    SeedDocument::sign_v2(sequence, vec![onion()], standard(), None, &operator()).unwrap()
}

fn open(dir: &TempDir) -> Heads {
    std::fs::create_dir_all(&dir.0).unwrap();
    Heads::load(&dir.0, &key())
}

fn sequence(heads: &Heads) -> Option<u64> {
    heads.document().map(|document| document.sequence)
}

#[test]
fn a_fresh_node_takes_the_highest_head_and_marks_the_tip() {
    let dir = TempDir::new();
    let mut heads = open(&dir);
    heads.keep(vec![head(2, &[]), head(4, &[entry(100, 30_000)])], Some(5_000)).unwrap();
    assert_eq!(sequence(&heads), Some(4), "nothing held, so the head is trusted on first use");
    assert_eq!(heads.last_current, 5_000);
    heads.keep(vec![], Some(6_000)).unwrap();
    assert_eq!(heads.last_current, 5_000, "a pass no source answered does not move the mark");
    heads.keep(vec![v2(4)], Some(7_000)).unwrap();
    assert_eq!(heads.last_current, 5_000, "an old introduction cannot have offered new terms");
}

#[test]
fn a_node_that_was_offline_takes_an_honest_change_and_an_online_one_refuses_a_backdated_one() {
    let dir = TempDir::new();
    let mut heads = open(&dir);
    heads.keep(vec![head(1, &[])], Some(1_000)).unwrap();
    heads.keep(vec![head(2, &[entry(1_500, 30_000)])], Some(9_000)).unwrap();
    assert_eq!(sequence(&heads), Some(2), "announced after this node last saw the head current");

    let online = TempDir::new();
    let mut heads = open(&online);
    heads.keep(vec![head(1, &[])], Some(3_000)).unwrap();
    heads.keep(vec![head(2, &[entry(2_000, 30_000)])], Some(3_010)).unwrap();
    assert_eq!(sequence(&heads), Some(1), "announced 1,000 blocks before this node last saw head 1 current");
    assert_eq!(heads.refused().map(|(sequence, _)| *sequence), Some(2));
    assert_eq!(heads.last_current, 3_010, "a refused head is still an answer");
    heads.keep(vec![head(3, &[entry(2_000, 30_000), entry(3_100, 40_000)])], Some(3_200)).unwrap();
    assert_eq!(sequence(&heads), Some(1), "a later head built on the backdated entry is refused too");
    heads.keep(vec![head(4, &[entry(3_150, 30_000)])], Some(3_300)).unwrap();
    assert_eq!(sequence(&heads), Some(4), "an honest head is kept");
    assert!(heads.refused().is_none(), "and clears the warning");
}

#[test]
fn an_edited_entry_and_a_second_head_at_one_sequence_are_refused() {
    let dir = TempDir::new();
    let mut heads = open(&dir);
    heads.keep(vec![head(5, &[entry(100, 30_000)])], Some(200)).unwrap();
    heads.keep(vec![head(6, &[entry(100, 31_000)])], Some(210)).unwrap();
    assert_eq!(sequence(&heads), Some(5));
    assert!(heads.refused().unwrap().1.contains("changes fee terms"));
    let twin = SeedDocument::sign(5, vec![onion(), format!("{}.onion", "b".repeat(56))], head(5, &[entry(100, 30_000)]).schedule, None, &operator()).unwrap();
    heads.keep(vec![twin], Some(220)).unwrap();
    assert_eq!(heads.document().unwrap().introductions.len(), 1);
    assert_eq!(heads.refused().map(|(sequence, _)| *sequence), Some(5));
    assert!(dir.0.join("equivocation-5.obsd").exists(), "the second head is kept as evidence");
}

#[test]
fn a_version_2_head_never_replaces_version_3_and_is_served_to_old_nodes() {
    let dir = TempDir::new();
    let mut heads = open(&dir);
    heads.accept(v2(3), Some(100)).unwrap();
    assert_eq!(heads.document().unwrap().version, 2, "an upgraded node reads a version 2 head as one entry");
    heads.accept(head(3, &[]), Some(100)).unwrap();
    assert_eq!(heads.document().unwrap().version, 3, "version 3 at the same sequence extends version 2");
    assert_eq!(heads.accept(v2(9), Some(100)).unwrap(), 9, "an introduction keeps it for old nodes");
    assert_eq!((sequence(&heads), heads.document().unwrap().version), (Some(3), 3));
    assert_eq!(SeedDocument::decode(&heads.seed_bytes().unwrap()).unwrap(), v2(9));
    assert_eq!(SeedDocument::decode(&heads.head_bytes().unwrap()).unwrap(), head(3, &[]));
    heads.keep(vec![v2(10)], Some(100)).unwrap();
    assert_eq!(sequence(&heads), Some(3));

    let other = TempDir::new();
    let mut heads = open(&other);
    heads.accept(v2(3), Some(100)).unwrap();
    let elsewhere = SeedDocument::sign(3, vec![format!("{}.onion", "b".repeat(56))], standard(), None, &operator()).unwrap();
    assert!(heads.accept(elsewhere, Some(100)).is_err(), "version 3 at the same sequence must name the same introductions");
    assert_eq!(heads.document().unwrap().version, 2);
}

#[test]
fn dropping_terms_waits_for_the_chain_and_holds_the_mark() {
    let dir = TempDir::new();
    let mut heads = open(&dir);
    let raise = entry(100, 30_000);
    heads.keep(vec![head(1, &[raise])], Some(200)).unwrap();
    let pruned = SeedDocument::sign(2, vec![onion()], TermsSchedule::new(vec![raise]).unwrap(), None, &operator()).unwrap();
    heads.keep(vec![pruned.clone()], None).unwrap();
    assert_eq!(sequence(&heads), Some(1), "no tip, so dropping an entry waits");
    heads.keep(vec![pruned.clone()], Some(raise.start_height + EPOCH_BLOCKS - 1)).unwrap();
    assert_eq!(sequence(&heads), Some(1), "a store paid just before the raise is still listed");
    assert_eq!(heads.last_current, 200, "a waiting head holds the mark");
    assert!(heads.refused().is_none());
    heads.keep(vec![pruned], Some(raise.start_height + EPOCH_BLOCKS)).unwrap();
    assert_eq!(sequence(&heads), Some(2));
    assert_eq!(heads.last_current, raise.start_height + EPOCH_BLOCKS);
}

#[test]
fn the_mark_the_refusal_and_the_version_2_head_survive_a_restart() {
    let dir = TempDir::new();
    let mut heads = open(&dir);
    heads.accept(v2(1), Some(500)).unwrap();
    heads.keep(vec![head(2, &[])], Some(3_000)).unwrap();
    heads.keep(vec![head(3, &[entry(1_000, 30_000)])], Some(3_000)).unwrap();
    let again = open(&dir);
    assert_eq!((sequence(&again), again.last_current), (Some(2), 3_000));
    assert_eq!(again.refused().map(|(sequence, _)| *sequence), Some(3));
    assert_eq!(SeedDocument::decode(&again.seed_bytes().unwrap()).unwrap(), v2(1));
}

#[test]
fn a_peer_asks_for_version_3_and_falls_back_to_version_2_on_an_old_introduction() {
    use crate::directory::{Replica, Source};

    struct OldIntroduction(Vec<u8>);
    impl Source for OldIntroduction {
        fn seed_bytes(&self) -> Result<Vec<u8>, String> {
            Ok(self.0.clone())
        }
        fn store_bytes(&self, _: &[u8; 32]) -> Result<Option<Vec<u8>>, String> {
            Ok(None)
        }
        fn listing_bytes(&self, _: &[u8; 32]) -> Result<Vec<Vec<u8>>, String> {
            Ok(Vec::new())
        }
    }

    let intro_dir = TempDir::new();
    let mut intro = Replica::open(&intro_dir.0).unwrap();
    intro.accept_document(&v2(4).encode().unwrap(), &key()).unwrap();
    intro.accept_document(&head(4, &[]).encode().unwrap(), &key()).unwrap();
    let served = |kind| SeedDocument::decode(&intro.answer(kind, &[0u8; 32]).unwrap()).unwrap().version;
    assert_eq!((served(tormail::QUERY_SEED), served(tormail::QUERY_HEAD)), (2, 3));

    let peer_dir = TempDir::new();
    let mut peer = Replica::open(&peer_dir.0).unwrap();
    peer.pull_at(&[&intro], &key(), [0u8; 32], Some(100)).unwrap();
    assert_eq!(peer.head().unwrap().version, 3);
    let old_dir = TempDir::new();
    let mut behind = Replica::open(&old_dir.0).unwrap();
    behind.pull_at(&[&OldIntroduction(v2(4).encode().unwrap())], &key(), [0u8; 32], Some(100)).unwrap();
    assert_eq!(behind.head().unwrap().version, 2);
}
