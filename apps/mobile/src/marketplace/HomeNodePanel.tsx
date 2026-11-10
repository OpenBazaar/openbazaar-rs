import { useEffect, useState } from "react";
import { canRunAsHomeNode, homeNodeMode, setHomeNodeMode } from "../homeNode/api";
import { nodeTarget, servedByHomeNode } from "../homeNode/transport";
import { HomeConnectCard } from "./HomeConnectCard";
import { HomeDevicesCard } from "./HomeDevicesCard";
import { PasskeysCard } from "./PasskeysCard";

const failed = (e: unknown, fallback: string) => (e instanceof Error && e.message ? e.message : fallback);

/** Settings > Home node: run this computer as one, connect to one, and manage the devices of the node in use. */
export function HomeNodePanel() {
  const served = servedByHomeNode();
  const [generation, setGeneration] = useState(0);
  const ownNode = !served && nodeTarget() === "local";
  return <section className="ob-home-node" aria-label="Home node">
    {ownNode && canRunAsHomeNode() && <HomeModeCard onChange={() => setGeneration(n => n + 1)}/>}
    {!served && <HomeConnectCard/>}
    <HomeDevicesCard key={generation}/>
    {served && <PasskeysCard/>}
  </section>;
}

function HomeModeCard({ onChange }: { onChange: () => void }) {
  const [on, setOn] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  useEffect(() => { void homeNodeMode().then(setOn, () => setOn(false)); }, []);

  async function toggle() {
    if (on === null || busy) return;
    setBusy(true);
    setError("");
    try {
      setOn(await setHomeNodeMode(!on));
      onChange();
    } catch (err) {
      setError(failed(err, "The node could not be restarted."));
    } finally {
      setBusy(false);
    }
  }

  return <div className="ob-settings-card">
    <div className="ob-settings-card-heading">
      <h2>Run this computer as a home node</h2>
      <p>Let your phone and other computers manage this store from anywhere, over Tor, and directly on your home network. It works while this app is open; for a computer that stays on, install the background service (see the home node guide).</p>
    </div>
    <label className="ob-check">
      <input type="checkbox" checked={on === true} disabled={on === null || busy} onChange={() => void toggle()}/>
      {busy ? "Restarting the node…" : on ? "On: paired devices can reach this computer" : "Off"}
    </label>
    {error && <p className="ob-form-error" role="alert">{error}</p>}
  </div>;
}
