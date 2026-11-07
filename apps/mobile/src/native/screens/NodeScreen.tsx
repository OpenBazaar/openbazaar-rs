import { useState } from "react";
import { ScrollView, Text, TextInput } from "react-native";

import { collectionValue } from "../../homeNode/replica.ts";
import { lockWallet, passphraseError, unlockWallet } from "../../walletLock.ts";
import { torStatus } from "../channels.ts";
import { sync, useSession } from "../session.ts";
import { ago, Button, Card, Notice, Row, ui } from "../ui.tsx";

export function NodeScreen() {
  const session = useSession();
  const [passphrase, setPassphrase] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const network = collectionValue(session.replica, "network") as Record<string, unknown> | null;
  const lock = collectionValue(session.replica, "lock") as Record<string, unknown> | null;
  const node = session.replica.node ?? {};
  const version = (node.version && typeof node.version === "object" ? node.version : {}) as Record<string, unknown>;
  const tor = torStatus();
  const link = session.pairing?.link;

  const act = async (action: () => Promise<unknown>, fallback: string) => {
    setBusy(true);
    setMessage(null);
    try {
      await action();
      setPassphrase("");
      await sync();
    } catch (err) {
      setMessage(passphraseError(err, fallback));
    } finally {
      setBusy(false);
    }
  };

  return (
    <ScrollView contentContainerStyle={ui.screen} keyboardShouldPersistTaps="handled">
      <Text style={ui.title}>Node</Text>
      <Card title="Connection">
        <Row label="Reached over" value={session.via === "lan" ? "home Wi-Fi" : session.via === "tor" ? "Tor" : "not yet"} />
        <Row label="Tor" value={tor.state === "ready" ? "ready" : `${tor.state} ${Math.round(tor.progress * 100)}%`} />
        <Row label="Last sync" value={session.lastSyncAt ? ago(Math.floor(session.lastSyncAt / 1000)) : "never"} />
        <Row label="Address" value={link ? `${link.onion.slice(0, 8)}…${link.onion.slice(-6)}.onion` : ""} />
        <Row label="Node id" value={link?.nodeId.slice(0, 16) ?? ""} />
        {typeof version.version === "string" ? <Row label="Version" value={String(version.version)} /> : null}
      </Card>
      <Card title="Bitcoin network">
        <Row label="Network" value={String(network?.network ?? "unknown")} />
        <Row label="Confirmations" value={String(network?.confirmations ?? "–")} />
        <Row label="Lightning" value={String(network?.lightning ?? "–")} />
      </Card>
      <Card title="Wallet lock">
        <Row label="State" value={String(lock?.state ?? "unknown")} />
        {message ? <Notice tone="error" text={message} /> : null}
        {lock?.state === "locked" ? (
          <>
            <TextInput style={ui.input} value={passphrase} onChangeText={setPassphrase} secureTextEntry placeholder="Wallet passphrase" accessibilityLabel="Wallet passphrase" />
            <Button label="Unlock the node's wallet" busy={busy} disabled={!passphrase} onPress={() => void act(() => unlockWallet(passphrase), "The wallet did not unlock.")} />
          </>
        ) : null}
        {lock?.state === "unlocked" ? <Button label="Lock the wallet now" kind="plain" busy={busy} onPress={() => void act(lockWallet, "The wallet did not lock.")} /> : null}
        <Text style={ui.muted}>The lock is your node's: unlocking here unlocks it for every device until it locks itself again.</Text>
      </Card>
    </ScrollView>
  );
}
