// Decorative line art follows the approved logo's canopy, valance, and counter.
// Theme colors come from marketplace.css; these graphics carry no interface meaning.
export function CanopyDivider() {
  return (
    <svg className="ob-canopy-divider" viewBox="0 0 144 10" preserveAspectRatio="none" aria-hidden="true" focusable="false">
      <path d="M0 2Q12 14 24 2Q36 14 48 2Q60 14 72 2Q84 14 96 2Q108 14 120 2Q132 14 144 2" fill="none" stroke="currentColor" strokeWidth="1" vectorEffect="non-scaling-stroke"/>
    </svg>
  );
}

export function MarketEmptyArtwork({ variant }: { variant: "bag" | "saved" }) {
  return (
    <svg className="ob-market-artwork" viewBox="0 0 176 144" fill="none" aria-hidden="true" focusable="false">
      <path className="ob-art-ground" d="M22 130h132"/>
      <path className="ob-art-structure" d="M35 71v55m106-55v55"/>
      <path className="ob-art-roof" d="m24 59 64-34 64 34H24Z"/>
      <path className="ob-art-roof-fold" d="m88 25-24 34h48L88 25Z"/>
      <path className="ob-art-outline" d="m24 59 64-34 64 34H24Zm64-34v-9l13 5-13 4"/>
      <path className="ob-art-valance" d="M24 59h128c0 9-6 15-16 15s-16-6-16-15c0 9-6 15-16 15s-16-6-16-15c0 9-6 15-16 15s-16-6-16-15c0 9-6 15-16 15S24 68 24 59Z"/>
      <path className="ob-art-outline" d="M24 59c0 9 6 15 16 15s16-6 16-15c0 9 6 15 16 15s16-6 16-15c0 9 6 15 16 15s16-6 16-15c0 9 6 15 16 15s16-6 16-15"/>
      {variant === "bag" ? (
        <g className="ob-art-detail">
          <path d="m77 88-2 21h26l-2-21H77Z"/>
          <path d="M82 89v-5a6 6 0 0 1 12 0v5"/>
        </g>
      ) : (
        <path className="ob-art-detail" d="m88 104-10-9a6 6 0 0 1 10-7 6 6 0 0 1 10 7l-10 9Z"/>
      )}
      <rect className="ob-art-counter" x="35" y="111" width="106" height="15" rx="2"/>
      <path className="ob-art-structure" d="M35 111h106"/>
    </svg>
  );
}
