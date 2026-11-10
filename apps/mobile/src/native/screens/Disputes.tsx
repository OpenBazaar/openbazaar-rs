import { useCallback, useEffect, useState } from "react";
import { Alert, Text, View } from "react-native";

import { disputeState, fetchDisputes, partyName, ruleDispute, type Dispute } from "../../moderator.ts";
import { isLocked } from "../../walletLock.ts";
import { Button, Card, Notice, Row, ui } from "../ui.tsx";

const shortId = (text: string) => (text.length > 20 ? `${text.slice(0, 8)}…${text.slice(-6)}` : text);
const keyOf = (dispute: Dispute) => `${dispute.id}-${dispute.favors}`;
const failure = (err: unknown, fallback: string) => (err instanceof Error && err.message ? err.message : fallback);

/** A moderator node's desk: the dispute letters it has checked, and the operator's ruling on each. */
export function Disputes({ unlocked }: { unlocked: boolean }) {
  const [disputes, setDisputes] = useState<Dispute[] | null>(null);
  const [ruling, setRuling] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setDisputes(await fetchDisputes());
    } catch (err) {
      setMessage(failure(err, "The disputes could not be read."));
    }
  }, []);
  useEffect(() => void load(), [load]);

  const rule = async (dispute: Dispute) => {
    setRuling(keyOf(dispute));
    setMessage(null);
    try {
      await ruleDispute(dispute);
    } catch (err) {
      setMessage(isLocked(err) ? "Unlock the node's wallet, then rule." : failure(err, "The ruling was not sent."));
    }
    await load();
    setRuling(null);
  };
  const confirm = (dispute: Dispute) => {
    const party = partyName(dispute.favors);
    const effect = dispute.favors === "buyer" ? "The buyer can then take the escrowed coins back." : "The seller can then take the escrowed coins.";
    Alert.alert(`Rule for the ${party}?`, `${effect} A ruling cannot be changed, and it closes the other party's letter about this escrow.`, [
      { text: "Cancel", style: "cancel" },
      { text: `Rule for the ${party}`, style: "destructive", onPress: () => void rule(dispute) },
    ]);
  };

  return (
    <Card title="Disputes">
      <Text style={ui.muted}>This node is a moderator. Each letter below passed its checks: the escrow terms, the writer's signature, and the funding on chain.</Text>
      {message ? <Notice tone="error" text={message} /> : null}
      {disputes && !disputes.length ? <Notice text="No dispute has reached this node." /> : null}
      {disputes?.map(dispute => (
        <DisputeItem key={keyOf(dispute)} dispute={dispute} all={disputes} unlocked={unlocked} busy={ruling === keyOf(dispute)} onRule={() => confirm(dispute)} />
      ))}
      {disputes?.some(dispute => disputeState(dispute, disputes) === "open") && !unlocked ? <Text style={ui.muted}>Unlock the node's wallet above to rule.</Text> : null}
      <Button label="Check for new disputes" kind="plain" onPress={() => void load()} />
    </Card>
  );
}

function DisputeItem({ dispute, all, unlocked, busy, onRule }: { dispute: Dispute; all: Dispute[]; unlocked: boolean; busy: boolean; onRule: () => void }) {
  const party = partyName(dispute.favors);
  const state = disputeState(dispute, all);
  return (
    <View style={{ gap: 10 }}>
      <Text style={ui.cardTitle}>{dispute.favors === "buyer" ? "The buyer asks for a refund" : "The seller asks for the payment"}</Text>
      <Text style={ui.subtitle} selectable>
        {dispute.memo || "The letter has no message."}
      </Text>
      <Row label="Escrow" value={shortId(dispute.id)} />
      <Row label="Funding" value={shortId(dispute.outpoint)} />
      <Row label="Arrived at block" value={String(dispute.receivedHeight)} />
      {state === "granted" ? <Notice text={`Ruled for the ${party}.`} /> : null}
      {state === "lost" ? <Notice text="Closed. The ruling on this escrow went to the other party." /> : null}
      {state === "open" ? <Button label={`Rule for the ${party}`} busy={busy} disabled={!unlocked} onPress={onRule} /> : null}
    </View>
  );
}
