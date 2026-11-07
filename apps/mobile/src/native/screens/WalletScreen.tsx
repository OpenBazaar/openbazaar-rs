import { useState } from "react";
import { ScrollView, Text } from "react-native";
import * as Clipboard from "expo-clipboard";

import { bitcoinUri, newWalletAddress } from "../../nodeWallet.ts";
import { Qr } from "../../NativePayments.tsx";
import { collectionValue } from "../../homeNode/replica.ts";
import { sync, useSession } from "../session.ts";
import { Button, Card, formatBtc, Notice, Row, ui } from "../ui.tsx";

type WalletDoc = { network?: string; balance?: Record<string, number>; receiveAddress?: string; lastSync?: number | null; syncError?: string | null };

export function WalletScreen() {
  const session = useSession();
  const wallet = collectionValue(session.replica, "wallet") as WalletDoc | null;
  const error = session.replica.collections.wallet?.error;
  const [fresh, setFresh] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const address = fresh ?? wallet?.receiveAddress ?? null;
  const balance = wallet?.balance ?? {};
  return (
    <ScrollView contentContainerStyle={ui.screen}>
      <Text style={ui.title}>Wallet</Text>
      {wallet?.network === "testnet4" ? <Notice text="Testnet4: test coins with no value." /> : null}
      {error ? <Notice tone="error" text={error.code === "locked" ? "The wallet is locked on your node. Unlock it under Node to see a fresh balance." : `The wallet did not sync (${error.code}).`} /> : null}
      {!wallet ? <Notice text="The wallet has not synced to this phone yet." /> : null}
      {wallet ? (
        <Card title="Balance">
          <Row label="Spendable" value={formatBtc(balance.spendableSats ?? 0)} />
          <Row label="Pending" value={formatBtc(balance.pendingSats ?? 0)} />
          <Row label="Total" value={formatBtc(balance.totalSats ?? 0)} />
          <Row label="Chain sync" value={wallet.lastSync ? new Date(wallet.lastSync * 1000).toLocaleString() : "not yet"} />
        </Card>
      ) : null}
      {address ? (
        <Card title="Receive">
          <Qr value={bitcoinUri(address)} />
          <Text selectable style={ui.subtitle}>
            {address}
          </Text>
          <Button label={message ?? "Copy address"} kind="plain" onPress={() => void Clipboard.setStringAsync(address).then(() => setMessage("Copied"))} />
          <Button
            label="New address"
            kind="plain"
            busy={busy}
            onPress={async () => {
              setBusy(true);
              try {
                setFresh((await newWalletAddress()).address);
                void sync();
              } catch (err) {
                setMessage(err instanceof Error ? err.message : "No new address");
              } finally {
                setBusy(false);
              }
            }}
          />
        </Card>
      ) : null}
      <Text style={ui.muted}>Paying from this phone always asks for your wallet passphrase. A paired phone alone cannot spend.</Text>
    </ScrollView>
  );
}
