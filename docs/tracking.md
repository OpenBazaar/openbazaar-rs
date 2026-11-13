# Shipment tracking

Shipment tracking is optional and seller-side. It is off unless the seller's
node is started with both a tracking provider and Tor:

```sh
OB_TRACKING_API_KEY=… ob-node serve --tracking aftership --proxy socks5h://127.0.0.1:9050
```

## What happens

1. The seller marks a physical sale shipped and chooses USPS, UPS, FedEx, or
   DHL and the tracking number, as before.
2. The seller's node asks AfterShip about that carrier and tracking number at
   most once every six hours, and only while the sale is shipped but not
   completed.
3. The node reduces the answer to a status (`in_transit`, `delivered`, and so
   on), checkpoint times, and city/state/country.
4. Any progress is sent to the buyer's node in the same sealed, sender-pinned
   reply channel as the seller's `shipped` letter. The buyer sees the timeline
   on the order; the seller sees the same timeline on the sale.
5. A delivered checkpoint is recorded as evidence. It does **not** complete
   the order or move escrow by itself. The buyer still confirms receipt, or a
   future escrow policy can use the delivered time to start an objection
   window.

## Privacy and security rules

- **Buyers never contact AfterShip or a carrier.** Their IP address, node, and
  order are not sent to a tracking service.
- **The seller's node is not contacted directly without Tor.** Without
  `--proxy`, AfterShip is not contacted at all. This is fail-closed, matching
  exchange rates and public chain servers.
- **Only carrier + tracking number leaves the seller's node.** No order id,
  buyer identity, ship-to address, item, amount, or note is sent. Creating an
  AfterShip tracker sends those same two fields and nothing else.
- **The API key stays in memory.** It is read from `OB_TRACKING_API_KEY`, never
  accepted on the command line, stored with a sale, sent to a buyer, returned
  by an HTTP route, or logged.
- **Sensitive checkpoint fields are discarded before storage.** AfterShip can
  return street-level locations, coordinates, recipient/signature names, and
  proof-of-delivery data. The node never reads those fields. Stored and
  forwarded checkpoints keep only time, normalized status, city, state, and
  country.
- **Provider answers are bounded and checked.** Responses are size-limited,
  redirects are not followed, an answer for another carrier or tracking number
  is refused, and the buyer's node accepts only known statuses, bounded coarse
  locations, and updates matching the shipment its seller already announced.
- **Updates are authenticated by the existing order channel.** A tracking
  update is useful only from the order's pinned per-sale seller key, inside
  the sealed reply. A stranger cannot inject one through the tracking API.

## What AfterShip still learns

Privacy here means minimizing and containing disclosure, not pretending a
third-party lookup is free. AfterShip learns the seller's tracking numbers,
lookup times, and that this AfterShip account tracks shipments. Requests leave
through the seller's Tor proxy, so AfterShip does not learn the seller node's
IP address from this integration, but the API account itself is identified by
its key. Anyone who needs to avoid that disclosure should not enable tracking;
manual carrier and tracking entry continues to work exactly as before.

Tracking evidence can also be wrong or misleading: carriers reuse numbers,
mark parcels delivered early, or deliver to the wrong place. That is why this
step only displays progress and records `deliveredAt`; it does not settle a
sale or decide a dispute.
