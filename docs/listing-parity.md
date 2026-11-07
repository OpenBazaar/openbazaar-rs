# Listing and purchasing parity

What sellers and buyers get on Amazon, eBay and Mercari, what OpenBazaar has, and what it builds now. The second half is the design for the records and node behaviour that the "build now" set needs.

Checked against `main` at `e109aed`, with the seller work in progress on `seller-e2e` (listing v6: stable id, images, fiat price; own-store listing create/edit/delete; inbound orders and the Sales screen). This document builds on that work and does not repeat it.

## Ground rules

These come first because they decide several rows below.

- **No one is identifiable, locatable or traceable.** Public records (store, listing, coupon, review) are replicated forever. They carry no street, postal code, coordinates, email or phone (`public_text`), and no key that links a buyer across orders.
- **Trustless.** Anything a buyer relies on (a price, a discount, an accepted offer, a "verified purchase") is a record signed by the store key and checked on the buyer's device. The marketplace operator does not exist; there is no one to appeal to, so the rules must be checkable from the records alone.
- **The seller's node is offline much of the time.** The catalog is served by replicas and by the seller's last publish. Anything that needs a live answer (stock reservation, accepting an offer, redeeming a limited coupon) is decided by the seller's node when it reads its mailbox, and the buyer's client must say so.
- **Prices are in fiat** (minor units of an ISO 4217 currency). Sats are a conversion at order time, as in listing v6.
- **Testnet4 by default, mainnet switch, mainnet escrow refused.** Nothing here changes that.

## Parity matrix

Status: **have** = on main or in the seller PR (#27); **built** = in the listing-parity PRs (#28 protocol, #30 node, #31 web shop); **built (editor later)** = signed, priced and shown to buyers, but the seller editor cannot set it yet; **later** = designed here, built after; **won't** = deliberately not built. The recorded product decisions are in [Decisions](#decisions).

### Quantities and stock

| Feature | Amazon | eBay | Mercari | OpenBazaar | Notes |
|---|---|---|---|---|---|
| Quantity on hand | yes | yes | single item | have / built | v5 has one `stock`; v7 moves stock per variant. |
| Low stock / sold out state | "Only 3 left" | "Last one", "Out of stock" | "Sold" | built | Client derives it from signed stock: 0 = sold out, at most 3 = low. The record carries no publish date, so it reads "as of the seller's last update". |
| Per-order maximum | quantity limits | per-buyer limit | n/a | built | `max_per_order` in the signed listing, set in the editor; the seller node enforces it again across every line of the listing in one order. A per-buyer limit is **won't** (decided): it would need a buyer identity across orders, which this design does not have. |
| Quantity selector in cart and checkout | yes | yes | n/a | built | Capped at min(stock, max_per_order). |
| Stock reserved at order time | yes | yes | yes | built | Seller node is the authority; see [Overselling](#overselling-policy). |

### Variants

| Feature | Amazon | eBay | Mercari | OpenBazaar | Notes |
|---|---|---|---|---|---|
| Multi-dimension options | variation themes (SizeColor…) | up to 5 aspects, 250 variations | no | built | Up to 3 axes, 16 values each, 100 variants. |
| Per-variant SKU, price, stock | yes | yes | n/a | built | Price override in the listing's currency; SKU is seller text. |
| Per-variant image | yes | per-aspect photos | n/a | built | Index into the listing's images. |
| Per-variant weight | yes | yes | n/a | built (editor later) | Grams; falls back to the package weight. |
| Picker greys out unavailable combinations | yes | yes | n/a | built | Missing combination or zero stock. |
| Stable variant ids across edits | child ASIN | variation SKU | n/a | built | Random 8-byte id, kept by the editor; orders name it. |

### Item description

| Feature | Amazon | eBay | Mercari | OpenBazaar | Notes |
|---|---|---|---|---|---|
| Condition grades | New, Renewed, Used (4) | New, Open box, Used, Refurbished grades, For parts | New, Like new, Good, Fair, Poor | built | Six grades: New, Open box, Like new, Good, Fair, For parts or not working. Mercari's "Poor" maps to For parts. |
| Condition notes | yes | yes | in description | built | Short free text, `public_text`-checked. |
| Category taxonomy | browse tree | category tree | category tree | have / built (editor later) | Free-text category exists. v7 adds a fixed top-level taxonomy id; the free text stays as a subcategory. |
| Item specifics (brand, model, size, color, MPN, GTIN/UPC/ISBN) | yes | yes, per category | brand, size | built (editor later) | Up to 24 key/value pairs; well-known keys get typed checks (GTIN check digit). Shown on the listing page. |
| Search and filter on specifics, condition, price | yes | yes | yes | later | Client-side over the verified catalog. Search covers title, category, description and seller today. |
| Product catalog matching (one page, many sellers) | yes | product IDs | auto-fill | have | `product` key and "also sold by". |

### Media

| Feature | Amazon | eBay | Mercari | OpenBazaar | Notes |
|---|---|---|---|---|---|
| Multiple photos | 9 | 24 | 12 | have | v6: 8 images by hash. Raising it to 12 is a one-line change once media chunking lands. |
| Reorder, cover image | yes | yes | yes | have / later | The first image is the cover; reordering in the editor is later. |
| Zoom | yes | yes | yes | later | Lightbox in the buyer view. |
| Video | yes | 1 video | 1 video | later | Media cap is 2 MiB per object and the loopback HTTP body cap is 72 KiB; video needs chunked, content-addressed media first. |

### Pricing and promotions

| Feature | Amazon | eBay | Mercari | OpenBazaar | Notes |
|---|---|---|---|---|---|
| Compare-at / list price | reference price strikethrough | "Was" price (STP) | n/a | built (editor later) | Signed `compare_at`; must be above the price. The client shows the strikethrough without a percentage claim beyond what is signed. |
| Scheduled sale price | price discounts up to 30 days | markdown sale | time-limited sale | built (editor later) | Signed percentage off every variant, with start and end days. The client applies it by date; the seller node applies it by its own clock at order time. |
| Multi-buy / volume discount | quantity discounts (B2B) | volume pricing | bundle offers | built (editor later) | Up to 4 signed tiers: buy at least N, get P% off. |
| Coupons / promo codes | coupons | coded coupons | promo codes | built | `OBCP` record signed by the store key and kept on the seller's node; the buyer types the code at checkout and sees the discount in the seller's quote before paying. Sellers manage codes through `/coupons`; a coupon screen in the app is later. |
| Make an offer, accept / counter / decline | n/a | Best Offer | Offers | built (records and order checks), later (sending bids and answers) | Buyer sends a bid letter; seller answers with a store-signed offer the buyer can check offline. Auto-accept and auto-decline floors stay private on the seller node. |
| Price drop to watchers | n/a | send offers to watchers | offer to likers, promote | later (device-local notice), won't (targeted) | Not built. When built, the watchlist stays on the buyer's device and the buyer's own client notices a lower signed price on refresh; the seller is never told who watches. Notices sent by the seller to watchers would point at those buyers: **won't** (decided). |
| Smart pricing (automatic drops) | automate pricing | n/a | smart pricing | later | A seller-node schedule that re-signs the listing. Small once republish exists. |

### Shipping

| Feature | Amazon | eBay | Mercari | OpenBazaar | Notes |
|---|---|---|---|---|---|
| Package weight and dimensions | yes | yes | weight tiers | built (editor later) | Grams and millimetres, signed. Used for display and future rate tables. |
| Multiple shipping services with prices | yes | up to 4 domestic + 5 intl | prepaid labels | built | Up to 6 services: name, price, extra-item price, delivery estimate, ships-to regions. |
| Free shipping threshold | yes | promoted | free shipping option | have / built (editor later) | v5 had a sats threshold; v7 adds it in fiat. |
| Handling time | yes | yes | ships within N days | built (editor later) | Days, signed. |
| Ships-to regions | yes | yes | domestic | have / built (editor later) | Store-level country list exists; services name regions (country codes or "worldwide"). |
| Combined shipping (one seller, many items) | yes | combined shipping rules | bundles | built | Per service: first item at `price`, each extra item at `extra_item`; one package per seller in the cart. |
| Digital delivery | Kindle etc. | digital goods | n/a | have | `ListingKind::Digital`; no shipping. |
| Local pickup | n/a | local pickup | n/a | built (opt-in, coarse) | See [Local pickup](#local-pickup). A city or region only, never a street address (decided); meeting arranged in private messages. |
| Shipping labels, tracking integration | yes | yes | yes | won't | Carrier APIs would learn both parties. A tracking number can go in the private order reply. |

### Policies

| Feature | Amazon | eBay | Mercari | OpenBazaar | Notes |
|---|---|---|---|---|---|
| Store return policy | A-to-z | business policies | platform-wide | have | Store record text. |
| Per-listing returns override | yes | per listing | n/a | built (editor later) | Signed: no returns, or N days with who pays return shipping. Absent = store policy. |
| Buyer protection | A-to-z guarantee | Money Back Guarantee | platform | have | Moderated escrow (testnet4). |

### Buyer side

| Feature | Amazon | eBay | Mercari | OpenBazaar | Notes |
|---|---|---|---|---|---|
| Watchlist / favorites | lists | watchlist | likes | have / later | "Save for later" is device-local and never sent anywhere. Watching for changes is later. |
| Saved searches | n/a | saved searches | saved searches | later | Device-local; matches are counted on refresh. |
| Similar items | yes | yes | yes | later | Client-side: same category and overlapping specifics. |
| Notify me when back in stock | yes | n/a | n/a | later | Device-local, same mechanism as price-drop notices. |
| Reviews tied to completed orders | verified purchase | feedback | ratings | built | Pseudonymous per-order review keys; the seller signs a receipt on payment; the buyer's node signs and publishes the review, and other nodes keep it. The seller can attach one reply. See [Reviews](#reviews). |
| Seller ratings | seller feedback | feedback score | seller rating | built | Count, average and star distribution per store and per listing, computed by each node from the reviews it holds. No global score. |
| Q&A on listings | yes | ask a question | comments | later | Private questions already work through order mail once messaging lands; public Q&A needs a moderation story. |

### Listing lifecycle

| Feature | Amazon | eBay | Mercari | OpenBazaar | Notes |
|---|---|---|---|---|---|
| Create, edit, delete | yes | yes | yes | have | Seller PR. |
| Drafts | yes | yes | drafts | later | Kept on the seller node, never signed or published. |
| Duplicate listing | "copy" | sell similar | n/a | later | New listing id, new variant ids. |
| Bulk edit (price, stock, pause) | flat files | bulk edit | n/a | later | One request, one re-sign per listing. |
| Scheduled listing, relist | n/a | schedule, relist | relist | later | Needs a node scheduler; relist is just "set stock" today. |
| Vacation mode | yes | time away | vacation | later | Not built. Planned as a store-level flag in the node; listings are served as paused and orders refused with a message. Signed in the next store record version (later); until then a paused listing is republished with stock 0 and `away` availability. |
| Pre-order / made-to-order | pre-order | n/a | n/a | built (editor later) | Availability: in stock, pre-order (ships on a day), made to order (N days). |
| Bundles / lots | bundles | lots | bundles | built (lots, editor later), later (cross-listing bundles) | `lot_size`: one unit is N pieces. Bundles of different listings at one price are a coupon scoped to listings with a minimum subtotal. |
| Auctions | n/a | auction format | n/a | later | Decided: no seller-run auctions for now. See [Auctions](#auctions). |

## Design

### Records

All new records use the existing canonical encoding (`encode.rs`: little-endian integers, `u16`-length strings) and BIP 340 signatures over a tagged hash. Every new public text field goes through `public_text::check` at signing.

#### Listing v7 (`OBLS`, version 7)

v7 is v6 (`ListingDetails`: stable 16-byte `ListingId`, images, fiat price) followed by a `Commerce` block. v4, v5 and v6 still decode and verify. A v7 listing must carry a fiat price. Its base `stock` field is the sum of variant stock (or the single stock if there are no variants), so older clients still show something sensible.

```
Commerce {
  condition:       u8            // 0 new, 1 open box, 2 like new, 3 good, 4 fair, 5 for parts
  condition_note:  str (≤ 280)
  taxonomy:        u16           // top-level category id, 0 = uncategorised
  specifics:       [(key str ≤ 32, value str ≤ 120)] ≤ 24, sorted by key, unique
  axes:            [Axis { name str ≤ 32, values [str ≤ 40] ≤ 16 }] ≤ 3
  variants:        [Variant] ≤ 100 (empty when axes is empty)
  pricing: {
    compare_at:    u64           // 0 = none; else > the listing's fiat price
    sale:          optional { off_bps u16, starts_on u32, ends_on u32 }  // percent off every variant; YYYYMMDD, inclusive
    tiers:         [(min_qty u16, off_bps u16)] ≤ 4, strictly increasing in both
    accepts_offers: bool
  }
  purchase: {
    max_per_order: u16           // 0 = no limit
    lot_size:      u16           // pieces per unit, ≥ 1
    availability:  u8 + u32      // 0 in stock; 1 pre-order (ships on YYYYMMDD); 2 made to order (days); 3 away
  }
  shipping: {
    weight_g:      u32
    dims_mm:       [u16; 3]
    handling_days: u8
    free_over:     u64           // fiat minor units; 0 = never
    services:      [Service { name str ≤ 40, price u64, extra_item u64,
                              days_min u8, days_max u8, regions [CountryCode] ≤ 32 (empty = worldwide) }] ≤ 6
    pickup:        optional { area str (region check) }
  }
  returns:         u8 + u16      // 0 store policy; 1 no returns; 2 N days buyer pays; 3 N days seller pays
}

Variant {
  id:       [u8; 8]              // random, stable across edits
  choice:   [u8; axes.len()]     // index into each axis's values
  sku:      str ≤ 40
  price:    u64                  // 0 = the listing's price
  stock:    u32
  image:    u8                   // 0 = listing cover; else 1-based index into images
  weight_g: u32                  // 0 = package weight
}
```

Validation at sign and decode: every choice index is in range; no two variants share a choice or an id; axis and value names are unique; tiers, sale window and compare-at are consistent; a digital listing has no services, pickup, weight or dimensions; region strings pass `check_region`; GTIN-like specifics (`gtin`, `upc`, `ean`, `isbn`) must pass their check digit. A missing combination is a combination the seller doesn't offer.

Why variants live inside the listing and not as separate child records: one signature covers the whole offer, so a replica can't serve a mismatched set of children, and the buyer's picker can grey out combinations from one record. The cost is that a stock change re-signs and moves the listing hash; the seller node already keeps superseded hashes so older orders still resolve, and orders name the stable listing id and variant id.

#### Coupon (`OBCP`, version 1, tag `ob/coupon/v1`)

```
store_id, code str (A-Z 0-9 -, 3..24), off: (kind u8: percent bps | fixed minor), currency str,
min_subtotal u64, listings [ListingId] ≤ 32 (empty = whole store), starts_on u32, ends_on u32,
max_uses u32 (0 = unlimited), signature
```

A private coupon never leaves the seller's node: the buyer types the code, the order letter carries the code, and the seller's quote shows the discount before anything is paid, so codes can't be listed or scraped from replicas. A public coupon (later) is published with the store's catalog as its signed record, and the buyer's client verifies the store signature, the window and the scope itself. `max_uses` can only be enforced by the seller node; the client says "while uses last".

#### Offer (`OBOF`, version 1, tag `ob/offer/v1`)

Negotiation is two letters over the existing sealed order mail:

1. **Bid** (buyer → seller, in a sealed letter): listing id, variant id, quantity, amount, currency. Signed by the order's conversation key like any letter, so it does not link to the buyer's other orders.
2. **Answer** (seller → buyer, in the sealed reply): decline with a reason, or an `OBOF` record signed by the store key:

```
store_id, offer_id [u8;16], listing_id, variant_id (zero = no variants), quantity u16,
amount u64 (per unit), currency, buyer_key [u8;32], expires_at u64 (unix seconds), counter bool, signature
```

`buyer_key` is the conversation key the bid came from, so the offer is good for that buyer only. Accepting is ordering: the order letter carries the `OBOF` bytes; the seller node checks its own signature, the expiry, that `buyer_key` matches the order's sender key, and that `offer_id` has not been used. A counter from the buyer is a new bid. The seller's floor and auto-rules never leave the seller's node.

#### Order letter v3 (`OBOD`, version 3)

v2 plus:

```
line count u8 (≤ 20), then per line:
  listing_hash [u8;32], variant_id [u8;8] (zero = none), quantity u32, offer blob (≤ 512, encoded OBOF or empty)
service u8 (index into each listing's shipping.services, 0xff = pickup or digital),
coupon blob (≤ 512, the code as typed, or empty),
review_key [u8;32]
```

The lines are the whole package, so one letter can hold two variants of the same listing; the first line repeats the letter's own listing hash and quantity, and a v2 node still reads the first line. A listing hash may be a superseded version: the seller node resolves it to the current version and prices from that. An order with no terms is still written as v2.

#### Reviews (`OBRC` receipt, `OBRV` review, `OBRY` reply)

A review key is derived per order from the buyer's letter root (`m/1984'/c'/4'/0'`, the same root that gives each order its conversation key, under a separate label), so two reviews by the same buyer don't link. The order letter carries the review public key.

- **Receipt** (seller → buyer in the "paid" reply, or the next status reply once the store key is unlocked if it was locked when the payment confirmed): `store_id, listing_id, review_key, paid_on (YYYYMMDD), signature by store key`. The seller's node writes the first of the month as `paid_on`, so the receipt narrows the payment to a month on chain. It says "this key bought this listing". It names no order id, amount or address.
- **Review** (buyer → anyone): `receipt bytes, rating 1..5, text ≤ 1,000 (public_text-checked), written_on (YYYYMMDD, not before paid_on), signature by review_key`.
- **Reply** (seller → anyone, tag `ob/review-reply/v1`): `store_id, review hash, text 1..1,000 (public_text-checked), written_on, signature by store key`. One per review; a newer reply from the store replaces its older one. It cannot change the review, which the review key signs.

A client counts a review as a verified purchase when the receipt verifies under the store key, the review verifies under the receipt's review key, and no other review uses the same key (one review per receipt; a later one by the same key replaces it). Dates are days, not timestamps. The buyer's node waits a random 0 to 3 days after the buyer saves the review, then signs it dated that day, so the review can't be matched to a payment or to the moment the buyer wrote it.

**Publication and replication** (built). Every node keeps a review pool (`reviews.obv`, sealed by the vault) and serves all of it at `GET /public/reviews`, mixed, with nothing marking which reviews its own user wrote. When the buyer's node signs a review it adds it to its own pool and `POST`s it to `/public/reviews` on the seller's node and on every node it follows. Each replication round also pulls `/public/reviews` from every followed node. A pool keeps one review per review key and replaces it only with a newer one signed by the same key; it keeps one reply per review, accepted only if the store signed it and the review is in the pool. So a seller can leave a review out of its own pool but not out of the pools that already hold it, and anyone who follows one of those nodes gets it. A node the buyer's node pushes to does learn that the review came from that node, since pushes are first-hand; sending only to the seller and followed nodes limits who learns it. Since anyone can mint receipts for their own store key, a pool holds at most 2,000 reviews per store the node carries and 2,000 in total for stores it doesn't, and takes at most 500 per request. Public review routes refuse browser requests (an `Origin` header).

The buyer writes or changes a review on a paid, shipped or completed order; a change on the day it went out is refused, so two versions don't go out the same day.

What this does not stop, stated plainly: the seller learns which of their orders wrote which review (they issued the receipt to that order), which is no more than they already know about their own buyers. A seller can mint receipts for keys they control and review themselves; a client can't tell that apart from real buyers without payment proofs, which would link reviews to the chain. A seller that refuses receipts gets no reviews at all, which the client shows as "no verified reviews". A review reaches only nodes connected, through follows, to a node that holds it; there is no third-party review relay or global index.

### Reputation plan

Reviews are the first part of a larger plan. Only the first item is built.

1. **Receipt-locked, replicated reviews** (built, above): one review per paid order, a per-order key committed before the outcome, checked by every node from the signed receipt, kept by other nodes so the seller can't drop it, with one signed seller reply.
2. **Moderator dispute rulings as signed public outcomes** (later): which side the funds were released to, with no addresses or identities, so a store's disputes can be counted.
3. **Optional seller bond** (later): a time-locked output the buyer can verify on chain before ordering. It is collateral that shows commitment, not a fund a third party can slash.
4. **New-seller state** (later): no stars until a store has verified sales, and its first sales shown as moderated escrow with the moderator named.
5. **Self-deals** (later): direct payments between a seller and itself must not count like completed escrow sales once ratings weigh them.

There is no reviewer profile, no third-party review service and no score across stores in any of these.

### Overselling policy

The catalog is replicated and readable while the seller is offline, so stock shown to buyers is "as of last publish", and the client says so ("8 in stock as of the seller's last update"; the record carries no publish date to show).

1. **The seller's node is the only authority on stock.** It keeps a shelf count per variant (`stock.obv`): what the seller last set, less what has sold. The signed listing publishes the shelf count; units held for unpaid quotes are not taken off it, because a hold lasts an hour.
2. **Reserve on order receipt.** When the node reads an order letter (in arrival order), it checks the listing id, the variant id, `max_per_order`, availability and live stock. If the units are there, it reserves them for the payment window (the quote's expiry, 1 hour by default) and replies with the quote. If not, it replies "unavailable" before anything is paid.
3. **Commit on payment, release on expiry.** A confirmed payment turns the reservation into a sale. An unpaid quote releases its units when it expires.
4. **Late payment for released stock.** If a payment arrives after its reservation was released and the units were sold to someone else, the sale is marked oversold and the "paid" reply says which lines. Direct pay: the seller refunds by hand for now (an automatic refund needs the buyer's refund address in the order and the wallet unlocked; later). Escrow: the node offers the refund branch. The buyer's client shows "seller could not fill: refund pending".
5. **Republish.** Each watcher round signs again every own listing whose shelf count moved (only while the store key is unlocked; otherwise on a later round). When the seller edits a variant's stock, that count replaces the shelf count; a variant they did not touch keeps the ledger's count. A sold-out variant shows as sold out once the new record reaches replicas.
6. **Pre-order and made-to-order** listings reserve against their stock the same way; "made to order" with stock 0 means unlimited and is never oversold.

### Prices

The seller sets every amount in one fiat currency per listing. At order time the seller's node computes the line price in this order: the offer amount if a valid offer is attached; otherwise the variant price (or the listing price), less the sale percentage if today is inside the window, then the volume tier; then the coupon on the lines it covers (offer lines excluded). Discounts round down. Shipping uses the chosen service with combined shipping across lines in one checkout, then the free-shipping threshold on the discounted subtotal. The client computes the same total from the signed records with the same rules, checked against golden vectors generated by the Rust implementation (`crates/protocol/vectors/pricing.json`: signed records as hex, lines, expected quote), so the quote the seller returns can be checked. Sats are converted at the seller's rate when quoting, as in v6.

### Local pickup

Off by default, per listing. The only public data is a region name that passes `check_region` (a state, a country, or a city name on its own), never a neighbourhood, a street or a map pin. The client shows "Local pickup available in Oregon" and nothing finer. Choosing pickup puts no address in the order; the time and place are agreed in private messages, which are sealed per order. The order never carries a street address. The editor asks for a city or region only, and the buyer's checkout says that meeting in person shows each of them to the other.

### Auctions

**Later.** An auction needs a trusted clock, a single bid order, and binding bids, and each one is hard here:

- The seller's node is the only possible auctioneer, so it sees every bid and can shill-bid with fresh keys; no one else can check.
- The seller's node is often offline, so bids arrive late or in batches, and "ends at 20:00" can't be enforced by anyone else.
- A binding bid needs a deposit or escrow per bidder, and on-chain deposits link bidders to the chain.

A sealed-bid (second-price) auction, where bids are commitments revealed after the close and the seller signs the result, avoids sniping and most clock issues, but it still lets the seller shill-bid and needs deposits. Best Offer plus a scheduled sale price covers most of what sellers use auctions for. Decided: later, with no seller-run auctions for now; see [Decisions](#decisions).

### Node APIs (seller, paired client only)

Building on the seller PR's `own_listings`:

- `POST /store/listings` and `PUT /store/listings/{id}` take the v7 fields under `commerce`; variants keep their ids across edits; new variants get fresh ids. (Built.)
- `GET/POST /coupons`, `DELETE /coupons/{code}`. (Built.)
- `POST /checkout` lines take `variant`, packages take `service` (an index or `"pickup"`) and `coupon`. (Built.)
- Later: `POST /store/listings/{id}/duplicate`, bulk edit, drafts, `GET /offers`, `POST /offers/{id}/answer`.
- Reviews (built): `GET /stores/{key}/reviews` (count, average, stars, and each review with its listing title, paid month and reply); buyer `POST /orders/{id}/review` (`{rating, text}`); seller `POST /reviews/{id}/reply` (`{text}`). Between nodes, with no pairing: `GET` and `POST /public/reviews` (`{reviews: [hex]}` and `{replies: [hex]}`).
- Later: vacation mode (a store-level pause).

### Client

- **Seller editor** (built): condition and condition note, axes and values, a variant grid (SKU, price, stock, image per row) generated from the axes, per-order limit, shipping services (price, extra-item price, delivery days), pickup area. Later: specifics, taxonomy, per-variant weight, package weight and dimensions, handling time, ships-to regions, free-shipping threshold, returns override, compare-at, sale and tiers, availability, lot size, offers toggle, a coupon screen; drafts, duplicate, bulk edit, vacation mode.
- **Buyer** (built): variant picker that greys out missing combinations and strikes through sold-out values, with price and stock for the chosen variant; quantity selector capped by stock and the per-order limit; bag lines per variant; shipping service or local pickup and a coupon field at checkout; condition, specifics, shipping and returns on the listing page; strikethrough and sale badge; low-stock and sold-out notes "as of the seller's last update"; variant labels and oversold refund notice on orders; stars and a review form on paid orders, showing when the review will publish; reviews with seller replies and the store's rating on the listing page; the store's reviews with a reply box on the seller's own store page. Later: variant image swap, make-an-offer sheet, zoom, watching saved items, saved searches, back-in-stock and price-drop notices on the device, similar items, the native app checkout.
- **Fiat display**: amounts shown in the listing's currency, converted to the viewer's chosen currency with the node's rates when they differ, and sats only as the payment amount.

## Build order

1. **Protocol** (`crates/protocol`, `crates/tormail`): listing v7 commerce block, coupon, offer, receipt and review records, order letter v3, the shared price function and its test vectors. No dependency on the seller PR beyond listing v6.
2. **Node**: v7 authoring on top of `own_listings`, live stock with reservations, coupons, offers, receipts and reviews, combined pricing on order receipt.
3. **Seller UI**: the editor and listing management additions.
4. **Buyer UI**: picker, cart, checkout, coupons, offers, watchlist and reviews.

Steps 1 and 2 are in #28 and #30, except sending offers. Steps 3 and 4 are in #31 as listed under [Client](#client). The paper (`docs/paper`, section "Commerce Terms, Stock, and Reviews") describes what these PRs build.

## Decisions

1. **Auctions**: later. Best Offer and scheduled sale prices cover what sellers use auctions for. No seller-run auctions, sealed-bid or otherwise, for now.
2. **Reviews**: every node checks each review against its store-signed receipt. The buyer publishes the review like a listing and other nodes keep it, so it stays visible when the seller leaves it out of its own catalog; the seller may attach one signed reply. No third-party review relay. Revised from "reviews live with the seller's catalog" so that a seller can't drop a review.
3. **Local pickup**: kept, opt-in, as a city or region only. The app never collects a street address for pickup.
4. **Per-buyer limits**: won't build. `max_per_order` caps each order; a per-buyer limit would need a buyer identity.
5. **Vacation mode and price-drop notices**: later. Notices sent by the seller to watchers would point at those buyers, so a price-drop notice, when built, comes from the buyer's own client.
