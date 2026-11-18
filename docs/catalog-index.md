# Signed catalog index (design)

Status: design only. Nothing in this document is implemented.

## The problem

A store's record and listings are individually signed, so a replica cannot
forge them. But a replica can still lie by omission and by staleness:

- serve yesterday's catalog after the store repriced or retracted a listing;
- serve half the catalog, hiding listings it dislikes;
- keep serving a store that retracted itself.

A buyer fetching from a replica today has no way to tell a complete, fresh
copy from a curated or stale one. The directory has the mirror-image
problem: its head (`ob/seed/v3`) names introductions and a fee schedule but no store
set, so nothing signed says which stores are currently admitted, and a
buyer cannot distinguish "the directory never heard of this store" from "the
introduction is hiding it".

## Per-store index

The store key signs a small index over its own catalog:

```text
body  = "OBCX" | 0x01 | store id [32] | sequence u64 | signed_on u32
      | record digest [32] | listing_count u16 | listing hash [32] * count
doc   = body | signature [64]
```

Tag `ob/catalog-index/v1`. Listing hashes are the existing content addresses
(SHA-256 of the signed listing body), sorted, no repeats, so one catalog has
one encoding. `record digest` is the hash of the store's current signed
record, so record rotation (new mailbox key, new policies) is covered too.
`signed_on` is a day stamp like a listing's; `sequence` increases with every
change. A cap of 4,096 listings matches the labeler set cap and keeps the
index one small fetch.

A client that has the index verifies, for free:

- completeness: every listing the replica serves is in the index, and every
  index entry the client asks for is served or the replica is marked bad;
- freshness: the highest sequence seen for a store wins, and a replica that
  serves a lower one than the client has seen is stale;
- retraction: a listing absent from the newest index is gone, even if a
  replica still has the bytes.

The index does not prove the newest sequence is the newest that exists - a
replica can withhold a newer index. Fetching the index from two or three
introductions (the directory pass already dials three) makes silent
withholding require collusion, which is the same bar the directory heads
already accept.

## Admission snapshot (directory side)

A small companion record lets the directory signer publish what is admitted
without returning to a v1-style store list in every head:

```text
body  = "OBAX" | 0x01 | directory signer [32] | sequence u64 | signed_on u32
      | head sequence u64 | admitted_count u16 | store id [32] * count
doc   = body | signature [64]
```

Tag `ob/admission-index/v1`. It names the head sequence it accompanies, so a
client can pair it with the head it already verified. With it:

- "not listed" becomes checkable: a store with a valid admission record
  (`ob/admit/v2`, fee receipt on chain) that is missing from the snapshot is
  visible evidence the directory is excluding it, which is the transparency
  the old OB1 model never had;
- a replica of the directory can be audited the same way a replica of a
  store can.

The snapshot is advisory for rendering and auditing; a buyer's existing
rule stays - an admission record whose chain check fails is dropped no
matter what any index says.

## What this buys against spam

Admission already prices a listing appearance (20,000 sats to the store's fee address,
one confirmation, 2,016-block epoch, as scheduled by the head), and reputation bonds price moderator
trust. The index adds the piece neither can: a stable, signed, enumerable
catalog per store, which gives labelers (`ob/labels/v1`) exact targets, lets
a client rate-limit by store rather than by listing, and makes a spam
store's churn visible as a fast-climbing sequence number instead of an
unaccountable flood.

## Non-goals

- No global search index. Search stays on-device over the catalogs the
  client follows.
- No third-party attestation of inventory truth. The index proves what the
  store signed, not that the goods exist; that remains the moderators' and
  buyers' domain.
- No new transport. Indexes are bytes served by introductions and replicas,
  fetched exactly like store records.

## Open questions

- Whether the per-store index should carry a Merkle root instead of the
  flat hash list once catalogs grow past a few thousand listings; the flat
  list is simpler and 4,096 x 32 bytes is only 128 KiB.
- Whether `GET /public/store` should serve the index inline or at its own
  path; inline is one round trip, separate lets a client poll freshness
  cheaply.
- How a marketplace record ([marketplace-record.md](marketplace-record.md))
  interacts with snapshots when it pins stores from several directories.
