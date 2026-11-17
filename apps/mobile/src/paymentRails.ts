// Which direct-pay rails the own store can turn on from store settings.
// Bitcoin is always accepted. Escrow stays on bitcoin and is not a choice here.
// The sentences match the node's refusal when a route only reports `enabled: false`.

export type RailOffer = {
  on: boolean;
  /** The seller can flip the switch. */
  choosable: boolean;
  /** Why the switch is disabled. Null when it can be used. */
  reason: string | null;
  /** The node would decline orders on this rail right now. The switch still works. */
  problem: string | null;
  /** The first Lightning opt-in through an LSP has to be acknowledged. */
  needsLspAck: boolean;
};

export type DirectRail = {
  enabled: boolean;
  accepts: boolean;
  problem: string | null;
};

export type LightningRail = DirectRail & {
  blocked: string | null;
  lspNotice: string | null;
  lspAcknowledged: boolean;
};

export const MONERO_OFF = "Monero is off on this node. Start it with --xmr-daemon.";
export const ZCASH_OFF = "Zcash is off on this node. Start it with --zec-indexer.";
export const LIGHTNING_OFF = "Lightning is off on this node. Start it with --lightning.";
export const STORE_NOT_OPEN = "Save the store once before choosing what it accepts. The choice is signed into the store record.";

const idle: RailOffer = { on: false, choosable: false, reason: null, problem: null, needsLspAck: false };

function checking(name: string): RailOffer {
  return { ...idle, reason: `Checking whether this node can take ${name}.` };
}

function closed(on: boolean): RailOffer {
  return { ...idle, on, reason: STORE_NOT_OPEN };
}

/** A rail the node can sign on or off. Turning it off stays available after the daemon or indexer is gone. */
function direct(name: string, off: string, published: boolean, rail: DirectRail | null): RailOffer {
  if (!published) return closed(rail?.accepts === true);
  if (!rail) return checking(name);
  if (!rail.enabled && !rail.accepts) return { ...idle, reason: rail.problem ?? off };
  if (!rail.enabled) return { ...idle, on: true, choosable: true, problem: rail.problem ?? off };
  return { ...idle, on: rail.accepts, choosable: true, problem: rail.problem };
}

export function moneroOffer(published: boolean, rail: DirectRail | null): RailOffer {
  return direct("Monero", MONERO_OFF, published, rail);
}

export function zcashOffer(published: boolean, rail: DirectRail | null): RailOffer {
  return direct("Zcash", ZCASH_OFF, published, rail);
}

export function lightningOffer(published: boolean, rail: LightningRail | null): RailOffer {
  if (!published) return closed(rail?.accepts === true);
  if (!rail) return checking("Lightning");
  if (rail.blocked) return { ...idle, on: rail.accepts, reason: rail.blocked };
  const offer = direct("Lightning", LIGHTNING_OFF, true, rail);
  return { ...offer, needsLspAck: offer.choosable && !offer.on && !!rail.lspNotice && !rail.lspAcknowledged };
}
