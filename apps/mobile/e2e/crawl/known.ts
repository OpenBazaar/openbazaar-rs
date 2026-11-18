// Crawl findings that are not bugs. Each entry says why.
export const KNOWN: { kind: string; screen: RegExp; control: RegExp; why: string }[] = [
  { kind: 'dead control', screen: /^All products/, control: /^OpenBazaar home$/, why: 'The logo goes to All products, which is already showing.' }
]
