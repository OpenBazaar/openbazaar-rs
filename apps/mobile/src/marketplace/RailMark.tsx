// Original marks for the payment rails. These are not the Bitcoin, Monero,
// Lightning, or Zcash trademark artwork, and they are not an icon font.

import type { ReactNode } from "react";

export type Rail = "bitcoin" | "monero" | "lightning" | "zcash";

const marks: Record<Rail, ReactNode> = {
  bitcoin: (
    <path
      fill="currentColor"
      fillRule="evenodd"
      d="M12 1.6a10.4 10.4 0 1 0 0 20.8 10.4 10.4 0 0 0 0-20.8zM7.15 5.05h1.25v13.9H7.15zm2.45 0h1.25v13.9H9.6zM10.85 7.15h1.15c1.7 0 2.65.75 2.65 1.9s-.95 1.95-2.65 1.95h-1.15zm0 4.85h1.45c1.95 0 3 .8 3 2.1s-1.05 2.15-3 2.15h-1.45z"
    />
  ),
  monero: (
    <path
      fill="currentColor"
      fillRule="evenodd"
      d="M12 1.6a10.4 10.4 0 1 0 0 20.8 10.4 10.4 0 0 0 0-20.8zM6.6 17.2V8.5h2.2l3.2 3.9 3.2-3.9h2.2v8.7h-2.2V11.6L12 15.2 8.8 11.6v5.6z"
    />
  ),
  lightning: <path fill="currentColor" d="M13.55 1.35 4.7 12.85h5.35l-1.85 9.8 10.15-13.15h-5.35l2.55-8.15z" />,
  zcash: (
    <path
      fill="currentColor"
      fillRule="evenodd"
      d="M12 1.6a10.4 10.4 0 1 0 0 20.8 10.4 10.4 0 0 0 0-20.8zM6.35 6.85h11.3v2.35H11.2l6.45 5.45v2.4H6.35v-2.35h6.45L6.35 9.25z"
    />
  ),
};

export function RailMark({ rail, size = 20 }: { rail: Rail; size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
      {marks[rail]}
    </svg>
  );
}

/** A balance, not a coin. Moderated escrow is bitcoin, so it does not wear a rail mark. */
export function EscrowMark({ size = 20 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M12 3.2v3M5.2 6.2h13.6M12 6.2V18M8 18h8" />
      <path d="M5.2 6.2 7.7 12.5H2.7Z" />
      <path d="M18.8 6.2 21.3 12.5h-5Z" />
    </svg>
  );
}

/** The rails a listing can be paid on, each with its mark. Bitcoin is always one of them. */
export function ListingRails({ listing }: { listing: { acceptsModerator: boolean; acceptsLightning: boolean; acceptsXmr: boolean; acceptsZec?: boolean } }) {
  return (
    <div className="ob-payment-note">
      <div>
        <strong>Payment options</strong>
        <p className="ob-rail-row">
          <span className="ob-rail"><RailMark rail="bitcoin" /> Bitcoin</span>
          {listing.acceptsLightning && <span className="ob-rail"><RailMark rail="lightning" /> Lightning</span>}
          {listing.acceptsXmr && <span className="ob-rail"><RailMark rail="monero" /> Monero</span>}
          {listing.acceptsZec && <span className="ob-rail"><RailMark rail="zcash" /> Zcash</span>}
        </p>
        <p>{listing.acceptsModerator ? "Pay the seller directly, or choose moderated escrow at checkout." : "Pay the seller directly with bitcoin."}</p>
      </div>
    </div>
  );
}
