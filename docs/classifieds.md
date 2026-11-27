# Classifieds: WTB / WTS board and contact mail

Status: **design, for discussion** (issue links at the end). Nothing here is built yet.

## Why

Messaging already exists **inside an order**: sealed `chat` letters, per-order keys, outbox retry (`docs/STATUS.md`, order messages). What is missing is anything **before** checkout — a public board where someone can post want-to-buy (WTB) or want-to-sell (WTS), and a sealed way to contact the poster without placing an order.

Goal for the first cut: a **global classifieds board** (coarsely categorized), open to **store keys and buyer-only light identities**, with **Contact** opening a new sealed letter kind. General “message any store without a post” comes later.

This is the same discovery tension as directories (`docs/directories.md`, #218): a board needs somewhere posts are listed, but contact mail must not depend on a central API that can disappear.

## What stays the same

- **Order chat** stays the thread for an open sale. Classified contact is a different conversation.
- **Records are self-authenticating.** A directory can withhold or serve a stale post; it cannot alter one.
- **Sealed letters:** HPKE to the poster’s mailbox key, BIP 340 by a per-thread key from the sender’s letter root, plain text only, padded sizes, outbox retry — same machinery as order mail.
- **Following by address / key** and orders, escrow, payments: untouched.
- **Buyer-only light mode** already has a seed, letter root, and can derive keys; it needs a reachable reply path (onion or a later pull mailbox) to receive contact.

## Proposal

### 1. A classified is a signed, short-lived record

```
Classified {
  id:          [u8; 16],           // random, stable for the post's life
  kind:        wtb | wts,
  category:    enum (fixed small set),
  title:       plain text,
  body:        plain text,
  region:      optional coarse region (same rules as store/listing region),
  price_hint:  optional amount + currency (not a quote),
  created:     unix day or height,
  expires:     unix day or height (hard cap in the signed body),
  poster:      BIP 340 x-only pubkey (store key or buyer-light identity),
  mailbox:     X25519 mailbox key,
  onion:       optional v3 onion for contact mail,
  signature:   BIP 340 over the body
}
```

- Magic / tag TBD (`OBCL` or similar), versioned like listings.
- Title, body, and region pass `plain_text` / `check_region` (no street, email, phone in the public record). Meeting details stay in sealed contact mail.
- **Categories** are a fixed enum at first (e.g. goods, services, housing, jobs, other) so indexes do not fragment. The client filters; directories index by category.
- **Expiry** is required. Suggested default 7–14 days; signing refuses a longer window than a hard max (e.g. 30 days). Expired posts drop from indexes; nodes may delete the bytes later.

### 2. Who can post

- **Store key:** post is linked to the store’s signed profile when the node has that store record; otherwise it still verifies by poster key alone.
- **Buyer-light identity:** same record shape; poster key comes from the buyer-only seed (path TBD, parallel to letter root / mailbox). No store required.
- A node can always keep and show **its own** posts with no directory fee. Replication into a directory’s classifieds index is what costs (below).

### 3. Discovery: directories carry a classifieds index

- Each directory may serve a **classifieds index** (admitted post ids + fee proofs), next to store admissions. Multi-directory (`docs/directories.md`) applies: the shop shows the **union** of posts from configured directories, plus the user’s own and any followed by id/url.
- Client shows **listed by** which directories, same idea as stores.
- A post is content-addressed / keyed by id; replicas share one copy across directories.

### 4. Spam: directory fee, not free megaphone

Mirror store admission, lighter:

| Rule | Why |
|---|---|
| Posts expire | Board stays fresh; abandoned spam dies |
| Listing in a directory costs a **small fee** to that directory’s fee address (shorter epoch / lower sats than store admission) | Same Sybil price as discovery; no free global flood |
| Already-admitted stores: free or cheaper posts under that admission | Don’t double-tax real shops |
| Buyer-light: must pay the classified fee (or use a directory that lists them under its own policy) | Throwaway seeds otherwise flood WTB |
| Own node: local posts free | Private / LAN use stays free |
| Contact letters: rate-limit + size cap on the poster’s mail drop | Board spam ≠ inbox spam |

A free directory can list anything; users see that in “listed by”. Portable one-payment-many-directories is out of scope for v1 (same open question as store admissions).

### 5. Contact: a new sealed letter kind

- Kind name TBD (`contact` / `classified-reply`): sealed to the post’s mailbox key, addressed to the onion in the post (or the poster’s known sale/mailbox onion).
- **Thread id** binds to the classified id + poster key. First contact letter opens the thread; later messages stay on it. Sender uses a per-thread key from the letter root (never the long-term identity in the clear), pinned like order mail.
- Receiver-stamped times, 2 000 char cap, plain text only, outbox retry — reuse order-message limits where they fit.
- UI: post detail → **Contact** → compose. Inbox of contact threads is separate from order chat.
- Buyer-light without a reachable onion cannot receive replies until a pull-mailbox path exists; the client must say so before they post (or only allow WTB that expects the poster to check a pull path — decide in implementation).

### 6. What this does not do (yet)

- Message a store with no classified (general DM).
- Escrow or moderated disputes on a contact thread (that stays with orders).
- Rich categories, images on posts, or search beyond title/body/category/region.
- A global “how many peers” count (classifieds are not a peer swarm).

## Open questions

1. Expiry clock: calendar days vs chain height? (Height ties to the fee epoch; days are clearer in the UI.)
2. Category enum for v1 — keep at five, or start with goods / services / other only?
3. Buyer-light mailbox path: require a published onion before allowing a post that expects replies, or allow WTB with pull-only contact from day one?
4. Store-admission holders: free posts, discounted fee, or same fee with a higher rate limit?
5. Should a directory be able to delist a post before expiry (abuse), or only refuse to renew?
6. Cap how many live posts one key may have per directory?

## Plan

1. **Protocol + node:** classified record, fee/admission into a directory index, local store of own posts, union in `/classifieds` (or equivalent), contact letter kind + thread + outbox. (Issue A)
2. **App:** Classifieds browse/filter/post, post detail, Contact compose + inbox; buyer-light and store entry points; clear copy when Tor/mailbox is not ready. (Issue B)
3. Depends on / extends multi-directory work for “listed by” and per-directory fees (`docs/directories.md`).
