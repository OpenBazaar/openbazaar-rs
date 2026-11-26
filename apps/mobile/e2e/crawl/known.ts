// Crawl findings that are not bugs. Each entry says why.
export const KNOWN: { kind: string; screen: RegExp; control: RegExp; why: string }[] = [
  { kind: 'dead control', screen: /^All products/, control: /^OpenBazaar home$/, why: 'The logo goes to All products, which is already showing.' },
  // Breadcrumb roots: clicking them while that workspace is already open is a no-op.
  { kind: 'dead control', screen: /./, control: /^Your store$/, why: 'The Your store breadcrumb stays on the store workspace when it is already open.' },
  { kind: 'dead control', screen: /./, control: /^Marketplace$/, why: 'The Marketplace breadcrumb stays on All products when it is already open.' },
]
