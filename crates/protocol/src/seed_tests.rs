use super::*;
use crate::sign::secret_from_label;
use crate::terms::{EPOCH_BLOCKS, FEE_CONFS, FEE_SATS, NOTICE_BLOCKS, TERMS_ENTRY_LEN};

fn onion(label: char) -> String {
    format!("{}.onion", label.to_string().repeat(ONION_HOST_LEN))
}

fn secret() -> SecretKey {
    secret_from_label("seed-document-test")
}

fn sample(sequence: u64) -> (SeedDocument, SecretKey) {
    let secret = secret();
    let doc = SeedDocument::sign(
        sequence,
        vec![onion('b'), onion('a')],
        DirectoryTerms::standard(sign::xonly(&secret)),
        Some(onion('c')),
        &secret,
    )
    .unwrap();
    (doc, secret)
}

fn raised(secret: &SecretKey) -> TermsSchedule {
    let standard = DirectoryTerms::standard(sign::xonly(secret));
    TermsSchedule::new(vec![
        TermsEntry::genesis(standard),
        TermsEntry { start_height: 5_000, announced_height: 5_000 - NOTICE_BLOCKS, terms: DirectoryTerms { fee_sats: 30_000, ..standard } },
    ])
    .unwrap()
}

/// The version 2 layout as alpha.6 writes it.
fn v2_bytes(secret: &SecretKey, sequence: u64, introductions: &[String], terms: &DirectoryTerms) -> Vec<u8> {
    let mut writer = Writer::new();
    writer.bytes(b"OBSD");
    writer.u8(2);
    writer.bytes(&sign::xonly(secret));
    writer.u64(sequence);
    writer.u16(introductions.len() as u16);
    for onion in introductions {
        writer.str(onion).unwrap();
    }
    writer.bytes(&terms.fee_key);
    writer.u64(terms.fee_sats);
    writer.u32(terms.fee_confs);
    writer.u32(terms.epoch_blocks);
    writer.u8(0);
    let mut body = writer.finish();
    let signature = sign::sign("ob/seed/v2", &body, secret);
    body.extend_from_slice(&signature);
    body
}

#[test]
fn a_head_round_trips_and_names_no_stores() {
    let (doc, secret) = sample(4);
    assert_eq!(doc.version, 3);
    assert_eq!(doc.signer, sign::xonly(&secret));
    assert_eq!(doc.introductions, vec![onion('a'), onion('b')]);
    let [only] = doc.schedule.entries() else { panic!("one entry") };
    assert_eq!((only.start_height, only.terms.fee_sats, only.terms.fee_confs, only.terms.epoch_blocks), (0, FEE_SATS, FEE_CONFS, EPOCH_BLOCKS));
    doc.verify().unwrap();
    let bytes = doc.encode().unwrap();
    assert_eq!(SeedDocument::decode(&bytes).unwrap(), doc);
    let mut tampered = bytes.clone();
    tampered[10] ^= 0x01;
    assert!(SeedDocument::decode(&tampered).unwrap().verify().is_err());
    let mut version1 = bytes.clone();
    version1[4] = 1;
    assert!(SeedDocument::decode(&version1).is_err(), "a version 1 document is not a head");
}

#[test]
fn a_schedule_round_trips_at_its_encoded_size() {
    let secret = secret();
    let doc = SeedDocument::sign(7, vec![onion('a')], raised(&secret), None, &secret).unwrap();
    let bytes = doc.encode().unwrap();
    let single = SeedDocument::sign(7, vec![onion('a')], DirectoryTerms::standard(sign::xonly(&secret)), None, &secret).unwrap();
    assert_eq!(bytes.len() - single.encode().unwrap().len(), TERMS_ENTRY_LEN);
    assert_eq!(SeedDocument::decode(&bytes).unwrap(), doc);
}

#[test]
fn a_version_2_head_reads_as_one_entry_and_re_encodes_the_same_bytes() {
    let secret = secret();
    let terms = DirectoryTerms::standard(sign::xonly(&secret));
    let bytes = v2_bytes(&secret, 9, &[onion('a'), onion('b')], &terms);
    let doc = SeedDocument::decode(&bytes).unwrap();
    doc.verify_signer(&sign::xonly(&secret)).unwrap();
    assert_eq!(doc.version, 2);
    assert!(doc.schedule.is_single());
    assert_eq!(doc.schedule.at(0).unwrap().terms, terms);
    assert_eq!(doc.encode().unwrap(), bytes);
    let signed = SeedDocument::sign_v2(9, vec![onion('b'), onion('a')], terms, None, &secret).unwrap();
    assert_eq!(signed.encode().unwrap(), bytes, "sign_v2 writes what alpha.6 reads");
}

#[test]
fn a_version_2_reader_refuses_version_3_and_the_tags_do_not_cross() {
    let secret = secret();
    let doc = SeedDocument::sign(3, vec![onion('a')], raised(&secret), None, &secret).unwrap();
    let bytes = doc.encode().unwrap();
    assert_eq!(Reader::new(&bytes).expect_magic(b"OBSD", 2), Err(Error::BadVersion), "alpha.6 keeps its old head");
    let body = &bytes[..bytes.len() - SIG_LEN];
    assert!(sign::verify("ob/seed/v2", body, &doc.signer, &doc.signature).is_err());
    let old = SeedDocument::sign_v2(3, vec![onion('a')], DirectoryTerms::standard(sign::xonly(&secret)), None, &secret).unwrap();
    let mut relabeled = old.clone();
    relabeled.version = 3;
    assert!(relabeled.verify().is_err(), "a version 2 signature does not pass as version 3");
    let mut many = old;
    many.schedule = raised(&secret);
    assert!(many.encode().is_err(), "version 2 carries one set of terms");
}

#[test]
fn a_different_signer_and_an_unsorted_list_are_refused() {
    let (doc, _) = sample(1);
    let other = sign::xonly(&secret_from_label("other-seed-key"));
    assert!(doc.verify_signer(&other).is_err());
    doc.verify_signer(&doc.signer).unwrap();
    let mut unsorted = doc.clone();
    unsorted.introductions.reverse();
    assert!(unsorted.verify().is_err());
    assert!(SeedDocument::sign(1, vec!["example.com".into()], DirectoryTerms::standard(doc.signer), None, &secret()).is_err());
}

#[test]
fn a_schedule_out_of_shape_is_refused() {
    let secret = secret();
    let standard = DirectoryTerms::standard(sign::xonly(&secret));
    let later = |start: u32, announced: u32, terms: DirectoryTerms| TermsEntry { start_height: start, announced_height: announced, terms };
    let cases: Vec<(&str, Vec<TermsEntry>)> = vec![
        ("empty", vec![]),
        ("nine entries", (0..9).map(|index| later(index * 2_000, index * 2_000, standard)).collect()),
        ("unsorted", vec![later(3_000, 1_000, standard), later(2_000, 1_000, standard)]),
        ("announced after start", vec![later(10, 20, standard)]),
        ("short notice for a raise", vec![TermsEntry::genesis(standard), later(2_000, 1_500, DirectoryTerms { fee_sats: 30_000, ..standard })]),
        ("short notice for a drop", vec![TermsEntry::genesis(standard), later(2_000, 1_900, DirectoryTerms { fee_sats: 10_000, ..standard })]),
        ("announced out of order", vec![later(5_000, 3_000, standard), later(7_000, 2_000, standard)]),
        ("dust fee", vec![TermsEntry::genesis(DirectoryTerms { fee_sats: 329, ..standard })]),
        ("no confirmations", vec![TermsEntry::genesis(DirectoryTerms { fee_confs: 0, ..standard })]),
        ("epoch under confirmations", vec![TermsEntry::genesis(DirectoryTerms { fee_confs: 7, epoch_blocks: 6, ..standard })]),
        ("epoch past the cap", vec![TermsEntry::genesis(DirectoryTerms { epoch_blocks: 52_417, ..standard })]),
        ("not a key", vec![TermsEntry::genesis(DirectoryTerms { fee_key: [0u8; 32], ..standard })]),
    ];
    for (name, entries) in cases {
        assert!(TermsSchedule::new(entries.clone()).is_err(), "{name}");
        let mut writer = Writer::new();
        writer.bytes(b"OBSD");
        writer.u8(3);
        writer.bytes(&sign::xonly(&secret));
        writer.u64(1);
        writer.u16(0);
        writer.u8(entries.len() as u8);
        for entry in &entries {
            entry.write(&mut writer);
        }
        writer.u8(0);
        writer.bytes(&[0u8; SIG_LEN]);
        assert!(SeedDocument::decode(&writer.finish()).is_err(), "{name} does not decode");
    }
    let drop = TermsSchedule::new(vec![TermsEntry::genesis(standard), later(2_000, 1_712, DirectoryTerms { fee_sats: 10_000, ..standard })]);
    assert!(drop.is_ok(), "288 blocks is notice enough to ask less");
}
