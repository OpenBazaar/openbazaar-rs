# Marketplace record (design)

Status: design only. Nothing in this document is implemented.

## Why

The network already unbundles everything a classic marketplace operator did:
a signed directory head names introductions and an admission fee
(`ob/seed/v2`), stores admit themselves with a fee receipt (`ob/admit/v2`),
moderators publish signed profiles with public rulings, dispute outcomes, and
bonds, labelers publish signed label sets (`ob/labels/v1`), and any node can
replicate a store it follows. Each role is a key, each product is a signed
record, and no role sees order traffic.

What is missing is the bundle as a thing a person can point at. "Shop at
Marlow's Market" today means: paste this directory seed, accept these
moderators, follow these labelers. A marketplace record binds those choices
into one signed, renderable entity, so a curator can publish a marketplace
the way a store publishes a store record - without running a server, without
carrying traffic, and without the power to take anyone off the network.

## What a marketplace is

A marketplace record is a signed statement by a curator key:

- a name, a tagline, and an avatar/banner (SHA-256 hashes, fetched like
  store media), so a client can render it;
- the directory signer whose heads the client should follow, so the
  marketplace inherits that directory's admission fee and store set;
- the moderator keys the curator endorses, which become the client's default
  accepted-moderator set while browsing this marketplace;
- the labeler keys the curator endorses, which the client follows for
  hide/warn decisions while browsing this marketplace;
- introduction onions that serve the marketplace's records, reusing the
  introduction mechanics of `ob/seed/v2`.

Joining a marketplace changes only what the client shows and which defaults
it offers. Orders, escrow, payments, and letters are exactly what they are
today, peer to peer. Leaving a marketplace is deleting a record from the
client.

## Proposed wire format

Same conventions as every other record (integers little-endian, `str` =
u16 byte length then UTF-8, BIP-340 over a tagged hash of the body):

```text
body = "OBMP" | 0x01 | curator [32] | sequence u64
     | name str | tagline str | avatar [32] | banner [32]
     | directory signer [32]
     | moderator_count u8  | moderator key [32] * count
     | labeler_count u8    | labeler key [32] * count
     | intro_count u16     | onion str * count
doc  = body | signature [64]
```

Tag `ob/marketplace/v1`. A later sequence by the same curator replaces an
earlier one, like a directory head. Introductions are sorted v3 onion hosts
with no repeats, like `ob/seed/v2`. Counts are capped (16 moderators, 16
labelers, 64 introductions) so the record stays small enough to paste.

## What the curator can and cannot do

Can:

- choose which directory's store set the marketplace browses;
- endorse moderators, so buyers get a vetted default instead of the shipped
  fixture, and the moderators' published outcomes and bonds carry the
  curator's implied vouching;
- endorse labelers, so the marketplace has a content policy that is visible,
  signed, and client-side;
- rotate any of these with a new sequence.

Cannot:

- remove a store from the network: a store the curator dislikes is still
  reachable by address, by other marketplaces, and by the directory itself;
- see or route orders: the record carries keys, not traffic;
- change escrow: a buyer still refuses terms naming a moderator the store's
  own record does not list, so a curator cannot inject a moderator into an
  escrow - endorsement only affects which stores the client surfaces.

This is the answer to the old OB1 problem where the company's trending list
and app store were a censorship chokepoint: a marketplace here is one signed
record among many, and switching costs one paste.

## Client behavior (sketch)

- "Add a marketplace" takes a pasted record or an onion that serves one,
  verifies the signature, and stores it like a followed store.
- While a marketplace is selected, search and browse draw from the
  marketplace's directory, the accepted-moderator default is the endorsed
  set (intersected with each store's own list at checkout, as today), and
  the endorsed labelers' hide/warn rules apply.
- The client shows which marketplace a default came from, so an endorsement
  is never silent.

## Open questions

- Should a marketplace be able to pin specific stores (a curated storefront)
  in addition to naming a directory? A `store_count | store id [32] * count`
  section is cheap, but a large pinned list recreates the stale-copy problem
  the [catalog index](catalog-index.md) addresses.
- Should curators pay the directory fee to be listed in a directory of
  marketplaces, so discovery of marketplaces is spam-priced the same way
  admission of stores is?
- Whether a marketplace names a preferred tracking policy or escrow policy
  version, or stays purely about discovery and defaults.
