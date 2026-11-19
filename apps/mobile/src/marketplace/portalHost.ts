/** Where portaled UI mounts: app styles are scoped to `.ob-app`, so document.body would leave it unstyled. */
export function portalHost(from: Element | null | undefined): Element {
  return from?.closest('dialog, .ob-app') ?? document.querySelector('.ob-app') ?? document.body
}
