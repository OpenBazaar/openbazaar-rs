// Original stroke marks for the payment rails. These are not the Bitcoin,
// Monero, Lightning, or Zcash trademark artwork.

import type { ReactNode } from "react";

export type Rail = "bitcoin" | "monero" | "lightning" | "zcash";

const marks: Record<Rail, ReactNode> = {
  bitcoin: (
    <>
      <circle cx="12" cy="12" r="8.25" />
      <path d="M9.6 7.6v8.8M9.6 7.6h2.7a2 2 0 0 1 0 4H9.6M9.6 11.6h3.2a2.2 2.2 0 0 1 0 4.4H9.6" />
    </>
  ),
  monero: (
    <>
      <circle cx="12" cy="12" r="8.25" />
      <path d="m8.4 8.4 7.2 7.2M15.6 8.4l-7.2 7.2" />
    </>
  ),
  lightning: <path d="M13 3 8 12h4l-2 9 7-11h-4l2-7" />,
  zcash: (
    <>
      <circle cx="12" cy="12" r="8.25" />
      <path d="M8.6 8.4h6.8l-6.8 7.2h6.8" />
    </>
  ),
};

export function RailMark({ rail, size = 20 }: { rail: Rail; size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      {marks[rail]}
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
