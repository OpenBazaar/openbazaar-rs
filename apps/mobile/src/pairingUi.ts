export type PairingKind = "loopback" | "code";

export function pairingKind(onThisComputer: boolean): PairingKind {
  return onThisComputer ? "loopback" : "code";
}

export function pairingDialog(kind: PairingKind): { title: string; body: string; action: string } {
  if (kind === "loopback") {
    return {
      title: "Continue on this computer",
      body: "This shop is running on this computer. Continue to place orders and open your wallet from this browser.",
      action: "Continue on this computer",
    };
  }
  return {
    title: "Pair this browser with your node",
    body: "Type the pairing code from your node to place orders and open your wallet from this browser. The code works once and expires after 5 minutes.",
    action: "Pair browser",
  };
}

export function pairingBanner(kind: PairingKind): { text: string; action: string } {
  if (kind === "loopback") {
    return {
      text: "This browser is not paired with your node. Orders, wallet and settings can continue on this computer.",
      action: "Continue on this computer",
    };
  }
  return {
    text: "This browser is not paired with your node. Orders, wallet and settings need a pairing code.",
    action: "Enter pairing code",
  };
}

export function pairingOrdersText(kind: PairingKind): string {
  return kind === "loopback"
    ? "Your orders are kept on your node. Continue on this computer to see them here."
    : "Your orders are kept on your node. Type its pairing code to see them here.";
}

export function pairingWalletPrompt(kind: PairingKind): { text: string; action: string } {
  return kind === "loopback"
    ? { text: "Continue on this computer to view your wallet.", action: "Continue on this computer" }
    : { text: "Pair this app with your node to view its wallet.", action: "Enter pairing code" };
}

export function browsersCardCopy(): {
  pageTitle: string;
  pageIntro: string;
  cardTitle: string;
  cardIntro: string;
  action: string;
  shown: string;
} {
  return {
    pageTitle: "Browsers",
    pageIntro: "Pair another browser with this node.",
    cardTitle: "Pairing code",
    cardIntro: "Create a code, then type it in the other browser.",
    action: "Create pairing code",
    shown: "This code works once and expires after 5 minutes. Type it in the other browser.",
  };
}
