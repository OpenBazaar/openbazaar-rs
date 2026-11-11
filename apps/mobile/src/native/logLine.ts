// One line for the phone's device log, which a Release build shows in
// Console.app and under `xcrun devicectl device process launch --console`.
// Never pass a pairing code, link, token or key. Reasons that come from Tor or
// the network can name an onion address, so those are shortened here.

export function logLine(text: string): string {
  const line = text.replace(/\s+/g, " ").replace(/[a-z2-7]+/g, run => (run.length === 56 ? `${run.slice(0, 6)}…${run.slice(-4)}` : run));
  return `ob-app: ${line.slice(0, 300)}`;
}
