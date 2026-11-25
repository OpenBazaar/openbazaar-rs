# Directories: more than one, chosen by the user

Status: **design, for discussion** (issue links at the end). Nothing here is built yet.

## Why

Today the node knows exactly one directory. Its document key is compiled into `ob-node` (`crates/node/src/seeds.rs`, `DOCUMENT_KEY`), with two bootstrap introduction onions (`INTRODUCTIONS`). Whoever holds that key signs the directory head: which introductions nodes dial, and the listing-fee schedule. Listing fees pay that operator's fee key. The paper lists this as a limitation ("One directory").

The directory cannot forge or alter anything: store and listing records are signed by the store's own key, and admissions are judged by each node against the chain. What it controls is **discovery**: which stores a new node finds without being handed an address. If the operator disappears, nodes keep their head, but once its introductions go dark no new node can find stores through it, and its fee terms can never change. That is the same shape of dependency that ended the last OpenBazaar network, narrower but real.

Goal: no single directory is the backbone. A node can use several, anyone can run one, and the user can add or remove them without recompiling.

## What stays the same

- **Following by key or by address** (`POST /follow`, `POST /follows`) needs no directory, and stays the first-class way to reach a store.
- **Records are self-authenticating.** A directory, introduction, or replica can withhold or serve stale records, never change them.
- **Admissions are judged locally:** a store is listed by a directory only while its fee payment to *that* directory's fee address is confirmed on the node's own chain view, under *that* directory's schedule.
- Orders, escrow, payments, reviews: untouched.

## Proposal

### 1. A directory is data, not a constant

```
Directory { key: [u8; 32] (BIP 340, x-only), bootstrap: Vec<onion>, label: String }
```

- The build ships a **default list** (today: the one existing directory). It is a starting point, not a requirement.
- `ob-node serve --directory KEY@ONION[,ONION...]` (repeatable) adds one; `--no-default-directories` drops the shipped list.
- The list is also kept in `<data dir>/directories.json` and edited from the app (Settings → Directories): add by `KEY@onion`, remove, see each one's head sequence, introductions, fee terms, and how many stores it lists.
- `head_plan` / `sign-seed` take the key to sign for as an argument instead of comparing against the constant, so anyone can run a directory with the shipped tooling.

### 2. One replica per directory

- `data/directory/` becomes `data/directories/<key hex>/`, each with its own heads, admissions, admission snapshot, and catalog indexes. On first start an existing `data/directory/` moves under the default key.
- Store records and listings are content-addressed and signed by the store, so they can be shared across directories (one store kept once) to save disk.
- `followed` and `opened` move from the replica to the node: they are the user's, not a directory's.
- Each directory is pulled on its own schedule, from its own introductions, so a slow or hostile one cannot hold up another.

### 3. The shop is the union

- A store is shown if **any** configured directory shows it (its admission is live under that directory's terms, or that directory's snapshot names it), or if the user follows it, as today.
- The client shows which directories list a store ("listed by: A, B"), so a store only one directory vouches for is visible as such.
- Search and categories run over the union. Removing a directory removes only what it alone listed.

### 4. Stores choose where to list

- A store pays each directory it wants to be listed in, under that directory's fee terms, and submits its admission to that directory's introductions. The seller UI shows the directories the node knows, their fees, and where the store is listed.
- Nothing stops a store listing in none: a pasted address still works.

### 5. Key rotation and loss

- A head can name a **successor key**, signed by the current key. Nodes follow the chain of successors, so an operator can rotate without a new build.
- If a directory's key is lost or compromised, users remove it and add its replacement. With more than one directory this costs a little discovery, not the network.

## What this does not solve

- **Choosing whom to trust is still a choice.** Defaults matter: most users will keep whatever ships. The default list should name directories run by different, unrelated people.
- **Eclipse:** a node whose every directory and introduction is hostile still sees a partial network. More independent directories make that harder, not impossible.
- **Fees stay per directory.** A store that wants to be found by everyone pays more than one. Directories may compete on price.
- **Spam is per directory too.** The fee is each directory's Sybil price; a free directory would list anything, and users would see that in "listed by".

## Open questions (pinheadmz, others)

1. Union, or a ranked list where the user's first directory wins on conflicts? (There is nothing to conflict on for records; only on whether a store is shown at all.)
2. Should a directory head be able to *delist* a store it admitted (e.g. for abuse), or only stop renewing it at the next epoch?
3. Successor keys: one level, or a full chain? What does a node do when it was offline across two rotations?
4. Should admissions be portable (one payment, several directories), or is per-directory payment the point?
5. What should the default list be at mainnet, and who should run the entries?
6. Is a directory needed at all for most users if stores share addresses socially, the way people share links?

## Plan

1. Runtime-configurable directory list, per-directory replicas, union in the shop. Migration of `data/directory/`. (Issue A)
2. Settings → Directories in the app; seller-side "where my store is listed" and paying more than one directory. (Issue B)
3. Successor keys in the head format. (Issue B)
4. A second, independently run directory before mainnet.
