/**
 * What the phone app is. It pairs with a home node and asks that node to
 * shop, pay, and keep orders. It is not a node, and handing someone the
 * phone does not hand them a shop.
 */
export function remoteRoleNotice(synced: boolean): string {
  if (!synced) {
    return "This phone is a remote for your home node. The shop stays closed until that node answers.";
  }
  return "This phone is a remote for your home node. Browsing and paying happen there, and they stop when that node is off.";
}
